//! Reversible filesystem application for the read-only VS Code link plan.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum PathState {
    Missing,
    Directory,
    Link(PathBuf),
}

fn snapshot(path: &Path) -> Result<PathState, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if ordinary_directory(&metadata) => Ok(PathState::Directory),
        Ok(metadata) if real_symlink(path, &metadata)? => fs::read_link(path)
            .map(PathState::Link)
            .map_err(|error| format!("cannot read '{}': {error}", path.display())),
        Ok(_) => Err(format!("unsafe destination '{}'", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(PathState::Missing),
        Err(error) => Err(format!("cannot inspect '{}': {error}", path.display())),
    }
}

use super::vscode::{Plan, native_path};
use super::{
    LinkAction, LinkOperation, create_symlink, normalize, ordinary_directory, real_symlink,
    remove_symlink,
};

#[derive(Debug)]
pub struct Journal {
    created_links: Vec<(PathBuf, PathBuf)>,
    created_dirs: Vec<PathBuf>,
    removed_links: Vec<(PathBuf, PathBuf)>,
    migrated_aliases: Vec<(PathBuf, PathBuf)>,
}

impl Journal {
    /// Reverse this call only. It is intentionally usable by W3's transaction.
    pub fn rollback(self) -> Result<(), String> {
        let mut failures = Vec::new();
        for (path, expected) in self.created_links.into_iter().rev() {
            match snapshot(&path) {
                Ok(PathState::Missing) => {}
                Ok(PathState::Link(target)) if target == expected => match remove_symlink(&path) {
                    Ok(()) => {}
                    Err(error) => failures.push(format!("{}: {error}", path.display())),
                },
                Ok(_) => failures.push(format!("{} changed during rollback", path.display())),
                Err(error) => failures.push(error),
            }
        }
        for (path, target) in self.removed_links.into_iter().rev() {
            if let Err(error) = create_symlink(&target, &path) {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        for path in self.created_dirs.into_iter().rev() {
            if let Err(error) = fs::remove_dir(&path) {
                if error.kind() != io::ErrorKind::NotFound {
                    failures.push(format!("{}: {error}", path.display()));
                }
            }
        }
        for (path, target) in self.migrated_aliases.into_iter().rev() {
            if let Err(error) = create_symlink(&target, &path) {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

/// Add destination and ancestor safety diagnostics to a freshly built plan.
pub(super) fn add_plan_diagnostics(plan: &mut Plan) {
    if let Err(error) = validate(plan) {
        plan.diagnostics.push(error);
    }
    let mut paths = vec![".github".to_owned()];
    paths.extend(["agents", "instructions", "prompts"].map(|kind| format!(".github/{kind}")));
    paths.extend(
        plan.entries
            .iter()
            .filter(|entry| {
                let path = plan.repo.join(native_path(&entry.destination));
                !fs::symlink_metadata(path.parent().unwrap())
                    .is_ok_and(|metadata| !ordinary_directory(&metadata))
            })
            .map(|entry| entry.destination.clone()),
    );
    for relative in paths {
        match snapshot(&plan.repo.join(native_path(&relative))) {
            Ok(state) => {
                if relative.split('/').count() == 2 {
                    if let PathState::Link(target) = &state {
                        plan.migrations.insert(relative.clone(), target.clone());
                    }
                }
                plan.snapshots.insert(relative, state);
            }
            Err(error) => plan.diagnostics.push(error),
        }
    }
}

pub fn apply_plan(plan: &Plan) -> Result<Journal, String> {
    apply_with_creator(plan, create_symlink)
}

pub(super) fn apply_with_creator(
    plan: &Plan,
    mut create: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<Journal, String> {
    if !plan.diagnostics.is_empty() {
        return Err(format!(
            "refusing to apply blocking link plan: {}",
            plan.diagnostics.join("; ")
        ));
    }
    if super::vscode::build_plan(&plan.repo, &plan.sources, plan.operation, plan.migrate)? != *plan
    {
        return Err("link plan is stale; run --plan again before applying".to_owned());
    }
    validate(plan)?;
    let mut journal = Journal {
        created_links: Vec::new(),
        created_dirs: Vec::new(),
        removed_links: Vec::new(),
        migrated_aliases: Vec::new(),
    };
    let result = (|| {
        for kind in ["agents", "instructions", "prompts"] {
            let path = plan.repo.join(".github").join(kind);
            if is_migratable_alias(plan, kind, &path)? {
                let raw = fs::read_link(&path)
                    .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
                remove_symlink(&path)
                    .map_err(|e| format!("cannot remove legacy alias '{}': {e}", path.display()))?;
                journal.migrated_aliases.push((path.clone(), raw));
                fs::create_dir(&path)
                    .map_err(|e| format!("cannot create '{}': {e}", path.display()))?;
                journal.created_dirs.push(path);
            }
        }
        for entry in &plan.entries {
            let destination = plan.repo.join(native_path(&entry.destination));
            match entry.action {
                LinkAction::Create => {
                    if fs::symlink_metadata(&destination).is_ok() {
                        return Err(format!(
                            "destination '{}' changed since planning",
                            entry.destination
                        ));
                    }
                    let parent = destination
                        .parent()
                        .ok_or_else(|| "invalid destination".to_owned())?;
                    ensure_parents(parent, &plan.repo, &mut journal)?;
                    ordinary_or_missing(parent, "destination ancestor")?;
                    let source = fs::canonicalize(plan.repo.join(native_path(&entry.source)))
                        .map_err(|error| format!("source '{}' changed: {error}", entry.source))?;
                    if !source.starts_with(&plan.repo)
                        || !source.is_dir()
                        || plan.source_targets.get(&entry.source) != Some(&source)
                    {
                        return Err(format!(
                            "source '{}' changed or escaped the repository",
                            entry.source
                        ));
                    }
                    create(&entry.target, &destination).map_err(|e| {
                        format!(
                            "cannot create symbolic link '{}': {e}; no copy or junction fallback",
                            destination.display()
                        )
                    })?;
                    journal
                        .created_links
                        .push((destination, entry.target.clone()));
                }
                LinkAction::Remove => {
                    let raw = checked_link(plan, entry, &destination)?;
                    remove_symlink(&destination)
                        .map_err(|e| format!("cannot remove '{}': {e}", destination.display()))?;
                    journal.removed_links.push((destination, raw));
                }
                LinkAction::Unchanged => {
                    checked_link(plan, entry, &destination)?;
                }
                LinkAction::Missing => {
                    if fs::symlink_metadata(&destination).is_ok() {
                        return Err(format!(
                            "destination '{}' changed since planning",
                            entry.destination
                        ));
                    }
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(journal),
        Err(error) => match journal.rollback() {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!("{error}; rollback failures: {rollback}")),
        },
    }
}

fn validate(plan: &Plan) -> Result<(), String> {
    let github = plan.repo.join(".github");
    ordinary_or_missing(&github, "destination ancestor")?;
    for kind in ["agents", "instructions", "prompts"] {
        let directory = github.join(kind);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if ordinary_directory(&metadata) => {}
            Ok(_) if is_migratable_alias(plan, kind, &directory)? => {}
            Ok(_) => {
                return Err(format!(
                    "destination ancestor '{}' is not an ordinary directory; matching legacy aliases require --migrate",
                    directory.display()
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect '{}': {e}", directory.display())),
        }
    }
    for entry in &plan.entries {
        let path = plan.repo.join(native_path(&entry.destination));
        let kind = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        if is_migratable_alias(plan, &kind, path.parent().unwrap())? {
            continue;
        }
        let present = fs::symlink_metadata(&path).is_ok();
        match (plan.operation, entry.action, present) {
            (LinkOperation::Link, LinkAction::Create, false)
            | (LinkOperation::Unlink, LinkAction::Missing, false) => {}
            (_, LinkAction::Unchanged | LinkAction::Remove, true) => {
                checked_link(plan, entry, &path)?;
            }
            _ => {
                return Err(format!(
                    "destination '{}' changed since planning",
                    entry.destination
                ));
            }
        }
    }
    Ok(())
}

fn ordinary_or_missing(path: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if ordinary_directory(&metadata) => Ok(()),
        Ok(_) => Err(format!(
            "{label} '{}' is not an ordinary directory",
            path.display()
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot inspect '{}': {e}", path.display())),
    }
}

fn is_migratable_alias(plan: &Plan, kind: &str, path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("cannot inspect '{}': {e}", path.display())),
    };
    if !real_symlink(path, &metadata)? {
        return Ok(false);
    }
    if !plan.migrate || plan.operation != LinkOperation::Link {
        return Ok(false);
    }
    let raw = fs::read_link(path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let actual = fs::canonicalize(path.parent().unwrap().join(raw))
        .map_err(|_| format!("legacy alias '{}' is broken or unsafe", path.display()))?;
    Ok(plan.entries.iter().any(|entry| {
        entry.source.ends_with(&format!("/{kind}"))
            && fs::canonicalize(plan.repo.join(native_path(&entry.source)))
                .ok()
                .as_ref()
                == Some(&actual)
    }))
}

fn checked_link(plan: &Plan, entry: &super::LinkEntry, path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("cannot inspect '{}': {e}", entry.destination))?;
    if !real_symlink(path, &metadata)? {
        return Err(format!(
            "destination '{}' is not a symbolic link",
            entry.destination
        ));
    }
    let raw =
        fs::read_link(path).map_err(|e| format!("cannot read '{}': {e}", entry.destination))?;
    let expected = normalize(&plan.repo.join(native_path(&entry.source)));
    let actual = normalize(&path.parent().unwrap().join(&raw));
    let matches = actual == expected
        || fs::canonicalize(&actual)
            .ok()
            .zip(fs::canonicalize(&expected).ok())
            .is_some_and(|(a, e)| a == e && a.starts_with(&plan.repo));
    if !matches {
        return Err(format!(
            "destination '{}' points elsewhere",
            entry.destination
        ));
    }
    Ok(raw)
}

fn ensure_parents(parent: &Path, repo: &Path, journal: &mut Journal) -> Result<(), String> {
    for path in [repo.join(".github"), parent.to_path_buf()] {
        if !path.exists() {
            fs::create_dir(&path)
                .map_err(|e| format!("cannot create '{}': {e}", path.display()))?;
            journal.created_dirs.push(path);
        } else {
            ordinary_or_missing(&path, "destination ancestor")?;
        }
    }
    Ok(())
}
