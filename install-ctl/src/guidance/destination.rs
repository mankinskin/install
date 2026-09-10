//! Destination scope resolution: repo/user/system/explicit roots, submodule
//! preference with canonical fallback, and traversal/escape validation.

use std::path::{Path, PathBuf};

use crate::guidance::graph::{Diagnostic, DiagnosticCode};
use crate::guidance::profile::DestinationScopeKind;

/// Injectable roots for the user/system destination scopes so tests never
/// touch a real home directory or system path.
pub struct DestinationPaths {
    pub user_root: PathBuf,
    pub system_root: PathBuf,
}

impl DestinationPaths {
    /// Resolve real platform defaults. Used only by the CLI entry points;
    /// tests construct `DestinationPaths` directly with a temp root.
    pub fn platform_default() -> Self {
        let user_root = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("install-ctl")
            .join("guidance");
        let system_root = if cfg!(windows) {
            PathBuf::from("C:/ProgramData/install-ctl/guidance")
        } else {
            PathBuf::from("/etc/install-ctl/guidance")
        };
        Self {
            user_root,
            system_root,
        }
    }
}

/// Lexically normalize `..`/`.` components without touching the filesystem.
/// Returns `None` if the path escapes above its own root (a bare leading
/// `..`), which callers report as a traversal diagnostic.
pub fn normalize_lexical(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    let mut depth: i64 = 0;
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            Prefix(p) => out.push(p.as_os_str()),
            RootDir => out.push(std::path::MAIN_SEPARATOR.to_string()),
            CurDir => {}
            ParentDir => {
                if depth == 0 {
                    return None;
                }
                out.pop();
                depth -= 1;
            }
            Normal(seg) => {
                out.push(seg);
                depth += 1;
            }
        }
    }
    Some(out)
}

/// Resolve the destination root for a given scope. `target_root` is only
/// used for the repo scope. Returns a blocking diagnostic instead of a path
/// when the scope's inputs are invalid (e.g. an explicit path that escapes
/// itself via `..`, or a missing explicit path).
pub fn resolve_destination_root(
    scope: DestinationScopeKind,
    explicit_path: Option<&Path>,
    target_root: &Path,
    paths: &DestinationPaths,
) -> Result<PathBuf, Diagnostic> {
    match scope {
        DestinationScopeKind::Repo => Ok(target_root.to_path_buf()),
        DestinationScopeKind::User => Ok(paths.user_root.clone()),
        DestinationScopeKind::System => Ok(paths.system_root.clone()),
        DestinationScopeKind::Explicit => {
            let raw = explicit_path.ok_or_else(|| Diagnostic {
                code: DiagnosticCode::PathTraversal,
                subject: "destination.path".to_string(),
                message: "explicit destination scope requires a destination path".to_string(),
            })?;
            normalize_lexical(raw).ok_or_else(|| Diagnostic {
                code: DiagnosticCode::PathTraversal,
                subject: raw.display().to_string(),
                message: format!(
                    "explicit destination '{}' escapes itself via '..'",
                    raw.display()
                ),
            })
        }
    }
}

/// Split a repo-relative id at its `.agents/` segment, returning
/// `(owning_dir, canonical_relative)`. `owning_dir` is `None` when the id
/// has no `.agents/` segment (the fallback and submodule paths coincide).
pub fn split_at_agents_root(id: &str) -> (Option<&str>, &str) {
    if let Some(pos) = id.find(".agents/") {
        let owning_dir = id[..pos].trim_end_matches('/');
        let owning_dir = if owning_dir.is_empty() {
            None
        } else {
            Some(owning_dir)
        };
        (owning_dir, &id[pos + ".agents/".len()..])
    } else {
        (None, id)
    }
}

/// Whether `target_root/candidate` is an initialized Git submodule, i.e. has
/// a `.git` *file* (gitlink) rather than a `.git` directory or nothing.
pub fn is_initialized_submodule(target_root: &Path, candidate: &str) -> bool {
    let git_marker = target_root.join(candidate).join(".git");
    git_marker.is_file()
}

/// Compute the destination-root-relative final path for a corpus id under
/// the repo scope: prefer an existing target submodule that owns the id,
/// falling back to the canonical `.agents/` root otherwise.
pub fn map_repo_scope_path(id: &str, target_root: &Path) -> String {
    let (owning_dir, canonical_relative) = split_at_agents_root(id);
    if let Some(owning_dir) = owning_dir
        && is_initialized_submodule(target_root, owning_dir)
    {
        return format!("{owning_dir}/.agents/{canonical_relative}");
    }
    format!(".agents/{canonical_relative}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lexical_collapses_dot_segments() {
        let p = normalize_lexical(Path::new("a/./b/../c")).unwrap();
        assert_eq!(p, PathBuf::from("a/c"));
    }

    #[test]
    fn normalize_lexical_rejects_escape() {
        assert!(normalize_lexical(Path::new("../etc")).is_none());
    }

    #[test]
    fn split_at_agents_root_extracts_owning_dir() {
        let (owner, rel) = split_at_agents_root("workflow-tools/.agents/instructions/foo.md");
        assert_eq!(owner, Some("workflow-tools"));
        assert_eq!(rel, "instructions/foo.md");
    }

    #[test]
    fn split_at_agents_root_falls_back_without_marker() {
        let (owner, rel) = split_at_agents_root("prompt.md");
        assert_eq!(owner, None);
        assert_eq!(rel, "prompt.md");
    }
}
