//! Dependency edge classification, Markdown link extraction, and transitive
//! closure computation over a guidance corpus.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    GuidanceReference,
    ProfileReference,
    GeneratedInput,
    SubmoduleOwnership,
    Unsupported,
    Unsafe,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeStatus {
    Resolved,
    Missing,
    Blocked,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DependencyEdge {
    pub kind: EdgeKind,
    pub source: String,
    pub target: String,
    pub status: EdgeStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    MissingDependency,
    Cycle,
    DuplicateDestination,
    AmbiguousOwnership,
    PathTraversal,
    AbsoluteSourcePath,
    SymlinkEscape,
    UnsupportedRecipeStep,
    UnsafeTarget,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub subject: String,
    pub message: String,
}

/// Extract inline Markdown link targets (`[label](target)`), in source
/// order. Titles (`(target "title")`) are stripped from the target.
pub fn extract_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close_bracket_rel) = text[i..].find(']') {
                let after_label = i + close_bracket_rel + 1;
                if text[after_label..].starts_with('(') {
                    let paren_start = after_label + 1;
                    if let Some(close_paren_rel) = text[paren_start..].find(')') {
                        let raw_target = &text[paren_start..paren_start + close_paren_rel];
                        let target = raw_target.split_whitespace().next().unwrap_or("");
                        if !target.is_empty() {
                            links.push(target.to_string());
                        }
                        i = paren_start + close_paren_rel + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    links
}

/// Classification of a single extracted link target relative to the source
/// root the referencing artifact lives in. Guidance/Profile carry no escape
/// information themselves — root-escape detection needs the referencing
/// file's directory and is done separately by [`resolve_relative`].
pub enum LinkClass {
    /// `http(s)://`, `mailto:`, or a fragment-only `#anchor` reference.
    External,
    /// Ends in `.md`.
    Guidance,
    /// Ends in `.toml`.
    Profile,
    /// A relative link that is not `.md`/`.toml`; not part of the guidance
    /// corpus, recorded as skipped rather than added to the closure.
    NonGuidance(String),
    /// An absolute path or drive-qualified path.
    AbsoluteSource(String),
}

pub fn classify_link(raw: &str) -> LinkClass {
    if raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("mailto:")
        || raw.starts_with('#')
    {
        return LinkClass::External;
    }
    // Strip a trailing fragment before resolving the target path.
    let path_part = raw.split('#').next().unwrap_or(raw);
    if path_part.is_empty() {
        return LinkClass::External;
    }
    if path_part.starts_with('/')
        || path_part.starts_with('\\')
        || (path_part.len() > 1 && path_part.as_bytes()[1] == b':')
    {
        return LinkClass::AbsoluteSource(path_part.to_string());
    }
    if path_part.ends_with(".md") {
        LinkClass::Guidance
    } else if path_part.ends_with(".toml") {
        LinkClass::Profile
    } else {
        LinkClass::NonGuidance(path_part.to_string())
    }
}

/// Resolve `raw` (possibly containing `.`/`..`) against an implicit root,
/// returning `None` if it would escape the root. `base_dir` is the
/// repo-relative directory of the file containing the link.
pub fn resolve_relative(base_dir: &str, raw: &str) -> Option<String> {
    let path_part = raw.split('#').next().unwrap_or(raw);
    let mut stack: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path_part.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}

/// A node the closure can visit: either an existing on-disk source file or a
/// recipe-produced artifact whose bytes are read from an input file.
pub trait ContentSource {
    /// Read UTF-8 text content for `id` if the node both exists and is
    /// readable as text (non-`.md`/`.toml` nodes are never queried).
    fn read_text(&self, id: &str) -> Option<String>;
    /// Whether `id` exists at all (used for missing-dependency detection).
    fn exists(&self, id: &str) -> bool;
    /// Whether `id` resolves (e.g. via a symlink) to a path outside the
    /// source root. Checked before `exists` so an escaping symlink is
    /// reported distinctly rather than silently followed.
    fn escapes_root(&self, id: &str) -> bool;
}

pub struct ClosureResult {
    pub artifacts: BTreeSet<String>,
    pub edges: Vec<DependencyEdge>,
    pub diagnostics: Vec<Diagnostic>,
    pub skipped_external: BTreeSet<String>,
}

/// Compute the deterministic transitive closure over `roots` by following
/// `.md` and `.toml` links discovered in each visited node's text.
pub fn compute_closure(roots: &[String], source: &dyn ContentSource) -> ClosureResult {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        InProgress,
        Done,
    }

    let mut state: BTreeMap<String, State> = BTreeMap::new();
    let mut edges: Vec<DependencyEdge> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut skipped_external: BTreeSet<String> = BTreeSet::new();
    let mut artifacts: BTreeSet<String> = BTreeSet::new();

    fn visit(
        id: &str,
        source: &dyn ContentSource,
        state: &mut BTreeMap<String, State>,
        edges: &mut Vec<DependencyEdge>,
        diagnostics: &mut Vec<Diagnostic>,
        skipped_external: &mut BTreeSet<String>,
        artifacts: &mut BTreeSet<String>,
    ) {
        state.insert(id.to_string(), State::InProgress);
        artifacts.insert(id.to_string());

        let base_dir = id.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        let text = if id.ends_with(".md") {
            source.read_text(id)
        } else {
            None
        };

        let mut links: Vec<String> = text.as_deref().map(extract_links).unwrap_or_default();
        links.sort();
        links.dedup();

        for raw in links {
            match classify_link(&raw) {
                LinkClass::External => {
                    skipped_external.insert(raw);
                }
                LinkClass::NonGuidance(resolved) => {
                    skipped_external.insert(resolved);
                }
                LinkClass::AbsoluteSource(target) => {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::AbsoluteSourcePath,
                        subject: id.to_string(),
                        message: format!("'{id}' links to absolute path '{target}'"),
                    });
                    edges.push(DependencyEdge {
                        kind: EdgeKind::Unsafe,
                        source: id.to_string(),
                        target,
                        status: EdgeStatus::Blocked,
                    });
                }
                LinkClass::Guidance | LinkClass::Profile => {
                    let resolved = resolve_relative(base_dir, &raw);
                    let Some(target) = resolved else {
                        diagnostics.push(Diagnostic {
                            code: DiagnosticCode::PathTraversal,
                            subject: id.to_string(),
                            message: format!(
                                "'{id}' links to '{raw}', which escapes the source root"
                            ),
                        });
                        continue;
                    };
                    let kind = if target.ends_with(".toml") {
                        EdgeKind::ProfileReference
                    } else {
                        EdgeKind::GuidanceReference
                    };

                    match state.get(&target).copied() {
                        Some(State::InProgress) => {
                            diagnostics.push(Diagnostic {
                                code: DiagnosticCode::Cycle,
                                subject: target.clone(),
                                message: format!(
                                    "dependency cycle: '{id}' -> '{target}' closes a cycle"
                                ),
                            });
                            edges.push(DependencyEdge {
                                kind,
                                source: id.to_string(),
                                target,
                                status: EdgeStatus::Blocked,
                            });
                        }
                        Some(State::Done) => {
                            edges.push(DependencyEdge {
                                kind,
                                source: id.to_string(),
                                target,
                                status: EdgeStatus::Resolved,
                            });
                        }
                        None => {
                            if source.escapes_root(&target) {
                                diagnostics.push(Diagnostic {
                                    code: DiagnosticCode::SymlinkEscape,
                                    subject: target.clone(),
                                    message: format!(
                                        "'{id}' references '{target}', which resolves outside the source root"
                                    ),
                                });
                                edges.push(DependencyEdge {
                                    kind,
                                    source: id.to_string(),
                                    target,
                                    status: EdgeStatus::Blocked,
                                });
                                continue;
                            }
                            if !source.exists(&target) {
                                diagnostics.push(Diagnostic {
                                    code: DiagnosticCode::MissingDependency,
                                    subject: target.clone(),
                                    message: format!(
                                        "'{id}' references '{target}', which does not exist"
                                    ),
                                });
                                edges.push(DependencyEdge {
                                    kind,
                                    source: id.to_string(),
                                    target,
                                    status: EdgeStatus::Missing,
                                });
                                continue;
                            }
                            edges.push(DependencyEdge {
                                kind,
                                source: id.to_string(),
                                target: target.clone(),
                                status: EdgeStatus::Resolved,
                            });
                            visit(
                                &target,
                                source,
                                state,
                                edges,
                                diagnostics,
                                skipped_external,
                                artifacts,
                            );
                        }
                    }
                }
            }
        }

        state.insert(id.to_string(), State::Done);
    }

    let mut sorted_roots: Vec<String> = roots.to_vec();
    sorted_roots.sort();
    sorted_roots.dedup();
    for root in &sorted_roots {
        if state.contains_key(root) {
            continue;
        }
        if source.escapes_root(root) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::SymlinkEscape,
                subject: root.clone(),
                message: format!("selected root '{root}' resolves outside the source root"),
            });
            continue;
        }
        if !source.exists(root) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::MissingDependency,
                subject: root.clone(),
                message: format!("selected root '{root}' does not exist"),
            });
            continue;
        }
        visit(
            root,
            source,
            &mut state,
            &mut edges,
            &mut diagnostics,
            &mut skipped_external,
            &mut artifacts,
        );
    }

    edges.sort();
    edges.dedup();
    diagnostics.sort();
    diagnostics.dedup();

    ClosureResult {
        artifacts,
        edges,
        diagnostics,
        skipped_external,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_links_finds_inline_targets() {
        let text = "See [base](../instructions/base.md) and [ext](https://example.com).";
        let links = extract_links(text);
        assert_eq!(
            links,
            vec!["../instructions/base.md", "https://example.com"]
        );
    }

    #[test]
    fn extract_links_strips_titles() {
        let text = r#"[a](foo.md "title text")"#;
        assert_eq!(extract_links(text), vec!["foo.md"]);
    }

    #[test]
    fn classify_link_detects_external_and_unsafe() {
        assert!(matches!(
            classify_link("https://example.com"),
            LinkClass::External
        ));
        assert!(matches!(classify_link("#anchor"), LinkClass::External));
        assert!(matches!(
            classify_link("/etc/passwd"),
            LinkClass::AbsoluteSource(_)
        ));
        assert!(matches!(
            classify_link("../../etc/notes.md"),
            LinkClass::Guidance
        ));
    }

    #[test]
    fn resolve_relative_rejects_escape_past_root() {
        assert_eq!(resolve_relative("a", "../b.md"), Some("b.md".to_string()));
        assert_eq!(resolve_relative("", "../b.md"), None);
    }
}
