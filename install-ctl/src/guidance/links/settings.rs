//! One transaction for named links, JSONC discovery settings, and ownership.

use super::{LinkAction, LinkOperation, vscode, vscode_links};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[path = "settings_document.rs"]
mod document;
#[path = "settings_files.rs"]
mod files;
#[path = "settings_ownership.rs"]
mod ownership;

const SETTINGS: &str = "settings.json";
const RECORD: &str = "install-ctl-guidance-links.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub links: vscode::Plan,
    pub configuration_changes: Vec<String>,
    pub settings_path: PathBuf,
    pub ownership_path: PathBuf,
    #[serde(skip_serializing)]
    settings_before: Option<Vec<u8>>,
    #[serde(skip_serializing)]
    ownership_before: Option<Vec<u8>>,
    #[serde(skip_serializing)]
    settings_after: Option<Vec<u8>>,
    #[serde(skip_serializing)]
    ownership_after: Option<Vec<u8>>,
    vscode_existed: bool,
}

pub fn build_plan(
    repo: &Path,
    sources: &[String],
    operation: LinkOperation,
    migrate: bool,
) -> Result<Plan, String> {
    let links = vscode::build_plan(repo, sources, operation, migrate)?;
    if !links.diagnostics.is_empty() {
        return Err(links.diagnostics.join("; "));
    }
    let vscode_existed = files::safe_directory(&links.repo.join(".vscode"))?;
    let settings_path = links.repo.join(".vscode").join(SETTINGS);
    let ownership_path = links.repo.join(".vscode").join(RECORD);
    let settings_before = files::read_optional(&settings_path)?;
    let ownership_before = files::read_optional(&ownership_path)?;
    let mut record = ownership::read(ownership_before.as_deref(), settings_before.is_none())?;
    let requested: BTreeMap<_, _> = links
        .sources
        .iter()
        .map(|value| {
            let (name, path) = value.split_once('=').expect("validated source");
            (name.to_owned(), path.replace('\\', "/"))
        })
        .collect();
    let doc = document::Document::parse(settings_before.as_deref())?;
    record.validate_current(&doc)?;
    let mut changes = Vec::new();
    if operation == LinkOperation::Unlink {
        for entry in &links.entries {
            if entry.action != LinkAction::Remove {
                continue;
            }
            let name = entry
                .destination
                .rsplit('/')
                .next()
                .expect("validated name");
            let kind = entry.destination.split('/').nth(1).expect("validated kind");
            if !record
                .sources
                .get(name)
                .is_some_and(|source| source.links.contains_key(kind))
            {
                return Err(format!(
                    "selected link '{}' has no matching ownership mapping",
                    entry.destination
                ));
            }
        }
    }

    for (name, path) in &requested {
        let old = record.sources.get(name);
        if let Some(old) = old {
            if old.path != *path {
                return Err(format!(
                    "source '{name}' is already managed at '{}'; unlink it first",
                    old.path
                ));
            }
        }
        if operation == LinkOperation::Unlink && old.is_none() {
            if links.entries.iter().any(|entry| {
                entry.destination.ends_with(&format!("/{name}"))
                    && entry.action != LinkAction::Missing
            }) {
                return Err(format!(
                    "selected links for '{name}' have no matching ownership record"
                ));
            }
            continue;
        }
        match operation {
            LinkOperation::Link => {
                let mut source = old.cloned().unwrap_or_else(|| ownership::Source {
                    path: path.clone(),
                    links: BTreeMap::new(),
                });
                for entry in links
                    .entries
                    .iter()
                    .filter(|entry| entry.destination.ends_with(&format!("/{name}")))
                {
                    let kind = entry.destination.split('/').nth(1).expect("validated kind");
                    let inserted =
                        doc.insert(kind, &entry.destination, &mut record.created_settings)?;
                    if inserted {
                        changes.push(format!("insert {} = true", entry.destination));
                    }
                    match source.links.get_mut(kind) {
                        Some(owned) => {
                            if owned.destination != entry.destination
                                || owned.target != entry.target
                            {
                                return Err(format!("owned mapping for '{name}:{kind}' changed"));
                            }
                            owned.inserted |= inserted;
                        }
                        None => {
                            source.links.insert(
                                kind.to_owned(),
                                ownership::OwnedLink {
                                    destination: entry.destination.clone(),
                                    target: entry.target.clone(),
                                    inserted,
                                },
                            );
                        }
                    }
                }
                record.sources.insert(name.clone(), source);
            }
            LinkOperation::Unlink => {
                let source = record
                    .sources
                    .remove(name)
                    .expect("checked selected ownership");
                for (kind, owned) in source.links {
                    if owned.inserted {
                        doc.remove(&kind, &owned.destination)?;
                        changes.push(format!("remove owned {}", owned.destination));
                    }
                }
            }
        }
    }
    doc.remove_empty_created_settings(&record.created_settings);
    let settings_after = if settings_before.is_none() && changes.is_empty() {
        None
    } else if operation == LinkOperation::Unlink
        && record.sources.is_empty()
        && record.settings_was_absent
        && doc.is_empty_without_comments()
    {
        None
    } else {
        Some(doc.bytes())
    };
    let ownership_after = if record.sources.is_empty() {
        None
    } else {
        Some(
            serde_json::to_vec_pretty(&record)
                .map_err(|error| format!("cannot serialize ownership: {error}"))?,
        )
    };
    if settings_after != settings_before {
        changes.push(format!("update .vscode/{SETTINGS}"));
    }
    if ownership_after != ownership_before {
        changes.push(format!("update .vscode/{RECORD}"));
    }
    Ok(Plan {
        links,
        configuration_changes: changes,
        settings_path,
        ownership_path,
        settings_before,
        ownership_before,
        settings_after,
        ownership_after,
        vscode_existed,
    })
}

pub fn apply_plan(plan: &Plan) -> Result<(), String> {
    apply_with_writer(plan, files::replace)
}

pub(super) fn apply_with_writer(
    plan: &Plan,
    mut write: impl FnMut(&Path, &Option<Vec<u8>>) -> Result<(), String>,
) -> Result<(), String> {
    let fresh = build_plan(
        &plan.links.repo,
        &plan.links.sources,
        plan.links.operation,
        plan.links.migrate,
    )?;
    if fresh != *plan {
        return Err("settings plan is stale; run --plan again before applying".to_owned());
    }
    let journal = vscode_links::apply_plan(&plan.links)?;
    let mut written = Vec::new();
    let result = (|| {
        for (path, before, after) in [
            (
                &plan.settings_path,
                &plan.settings_before,
                &plan.settings_after,
            ),
            (
                &plan.ownership_path,
                &plan.ownership_before,
                &plan.ownership_after,
            ),
        ] {
            if before != after {
                files::safe_directory(&plan.links.repo.join(".vscode"))?;
                if files::read_optional(path)? != *before {
                    return Err(format!(
                        "configuration '{}' changed during apply",
                        path.display()
                    ));
                }
                // Register first: an I/O failure may have happened after replacement.
                written.push((path, before));
                write(path, after)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for (path, before) in written.into_iter().rev() {
            if let Err(failure) = files::replace(path, before) {
                failures.push(failure);
            }
        }
        if let Err(failure) = journal.rollback() {
            failures.push(failure);
        }
        if !plan.vscode_existed {
            if let Err(failure) = files::remove_empty_directory(&plan.links.repo.join(".vscode")) {
                failures.push(failure);
            }
        }
        return if failures.is_empty() {
            Err(error)
        } else {
            Err(format!(
                "{error}; rollback failures: {}",
                failures.join("; ")
            ))
        };
    }
    Ok(())
}
