//! Read-only topology planning for VS Code's named guidance locations.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::{LinkAction, LinkEntry, LinkOperation};

const KINDS: &[&str] = &["agents", "instructions", "prompts"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub harness: super::Harness,
    pub repo: PathBuf,
    pub operation: LinkOperation,
    pub sources: Vec<String>,
    pub migrate: bool,
    pub entries: Vec<LinkEntry>,
    pub skipped_sources: Vec<String>,
    pub diagnostics: Vec<String>,
    pub snapshots: BTreeMap<String, super::vscode_links::PathState>,
    pub source_targets: BTreeMap<String, PathBuf>,
    pub migrations: BTreeMap<String, PathBuf>,
}

/// Build a deterministic, non-mutating VS Code link topology plan.
pub fn build_plan(
    repo: &Path,
    values: &[String],
    operation: LinkOperation,
    migrate: bool,
) -> Result<Plan, String> {
    if operation == LinkOperation::Unlink && migrate {
        return Err("--migrate is only valid with guidance link".to_owned());
    }
    let repo = fs::canonicalize(repo)
        .map_err(|error| format!("cannot resolve repository '{}': {error}", repo.display()))?;
    if !repo.is_dir() {
        return Err(format!(
            "repository '{}' is not a directory",
            repo.display()
        ));
    }
    let sources = parse_sources(values)?;
    let mut plan = Plan {
        harness: super::Harness::Vscode,
        repo,
        operation,
        sources: sources
            .iter()
            .map(|(name, path)| format!("{name}={}", path.display()))
            .collect(),
        migrate,
        entries: Vec::new(),
        skipped_sources: Vec::new(),
        diagnostics: Vec::new(),
        snapshots: BTreeMap::new(),
        source_targets: BTreeMap::new(),
        migrations: BTreeMap::new(),
    };

    for (name, source) in sources {
        let source_root = plan.repo.join(&source);
        if operation == LinkOperation::Link {
            match fs::canonicalize(&source_root) {
                Ok(real) if real.starts_with(&plan.repo) && real.is_dir() => {
                    plan.source_targets
                        .insert(source.to_string_lossy().replace('\\', "/"), real);
                }
                Ok(_) => {
                    plan.diagnostics.push(format!(
                        "source '{name}' must be a directory inside the repository"
                    ));
                    continue;
                }
                Err(error) => {
                    plan.diagnostics
                        .push(format!("requested source '{name}' is unavailable: {error}"));
                    continue;
                }
            }
        }
        let mut present_kinds = 0;
        for kind in KINDS {
            let source_kind = source.join(kind);
            let kind_path = plan.repo.join(&source_kind);
            let present = match fs::canonicalize(&kind_path) {
                Ok(real) if real.starts_with(&plan.repo) && real.is_dir() => {
                    plan.source_targets
                        .insert(source_kind.to_string_lossy().replace('\\', "/"), real);
                    true
                }
                Ok(_) => {
                    plan.diagnostics.push(format!(
                        "source kind '{}:{}' must be a directory inside the repository",
                        name, kind
                    ));
                    false
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => {
                    plan.diagnostics.push(format!(
                        "cannot resolve source kind '{}:{}': {error}",
                        name, kind
                    ));
                    false
                }
            };
            if operation == LinkOperation::Link && !present {
                plan.skipped_sources.push(format!("{name}:{kind}"));
                continue;
            }
            if present {
                present_kinds += 1;
            }
            let destination = format!(".github/{kind}/{name}");
            let destination_path = plan.repo.join(native_path(&destination));
            let parent_is_link = fs::symlink_metadata(destination_path.parent().unwrap())
                .is_ok_and(|metadata| !super::ordinary_directory(&metadata));
            let metadata = if parent_is_link {
                Err(io::Error::from(io::ErrorKind::NotFound))
            } else {
                fs::symlink_metadata(&destination_path)
            };
            let action = match metadata {
                Ok(_) if operation == LinkOperation::Link => LinkAction::Unchanged,
                Ok(_) => LinkAction::Remove,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if operation == LinkOperation::Link {
                        LinkAction::Create
                    } else {
                        LinkAction::Missing
                    }
                }
                Err(error) => {
                    plan.diagnostics.push(format!(
                        "cannot inspect destination '{destination}': {error}"
                    ));
                    continue;
                }
            };
            plan.entries.push(LinkEntry {
                source: source_kind.to_string_lossy().replace('\\', "/"),
                destination,
                target: target_from_kind_parent(&source_kind),
                action,
            });
        }
        if operation == LinkOperation::Link && present_kinds == 0 {
            plan.diagnostics.push(format!(
                "requested source '{name}' contains none of agents, instructions, or prompts"
            ));
        }
    }
    super::vscode_links::add_plan_diagnostics(&mut plan);
    Ok(plan)
}

pub(super) fn parse_sources(values: &[String]) -> Result<BTreeMap<String, PathBuf>, String> {
    if values.is_empty() {
        return Err("--source NAME=PATH is required for --harness vscode".to_owned());
    }
    let mut result = BTreeMap::new();
    for value in values {
        let (name, raw_path) = value
            .split_once('=')
            .ok_or_else(|| format!("--source must be NAME=PATH, got '{value}'"))?;
        if !valid_name(name) {
            return Err(format!("invalid VS Code source name '{name}'"));
        }
        let path = native_path(raw_path);
        if raw_path.is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!(
                "source path for '{name}' must be a non-empty repository-relative path without traversal"
            ));
        }
        if result.insert(name.to_owned(), path).is_some() {
            return Err(format!("duplicate VS Code source name '{name}'"));
        }
    }
    Ok(result)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.bytes();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    if !chars.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
    }) {
        return false;
    }
    !matches!(
        name,
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    )
}

fn target_from_kind_parent(source_kind: &Path) -> PathBuf {
    let mut target = PathBuf::from("..").join("..");
    target.push(source_kind);
    target
}

pub(super) fn native_path(value: &str) -> PathBuf {
    Path::new(value).components().collect()
}
