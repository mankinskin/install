//! Network acquisition is explicit; the planner consumes only the resulting local snapshot.

#[cfg(test)]
mod contract_tests;
mod modules;
mod repository;
mod source;
#[cfg(test)]
mod tests;

use super::cache::CachedRepository;
use crate::guidance::{
    graph::{DiagnosticCode, FallibleContentSource, compute_closure_fallible},
    profile::{
        CorpusItem, Profile, RecipeStep, load_profile, synthesize_direct_profile,
        validate_repo_relative,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

struct Acquirer {
    repositories: BTreeMap<String, repository::Repository>,
    cache_root: PathBuf,
    snapshot: PathBuf,
    recipes: BTreeMap<String, String>,
    materialized: BTreeSet<String>,
    empty_directories: BTreeSet<String>,
}

fn normalize(raw: &str) -> Result<String, String> {
    validate_repo_relative(raw, raw)?;
    if raw.contains([':', '\0', '\\']) {
        return Err(format!("guidance acquisition: unsafe source path '{raw}'"));
    }
    let mut components = Vec::new();
    for component in raw.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components
                    .pop()
                    .ok_or_else(|| format!("source path '{raw}' escapes its root"))?;
            }
            ".git" => {
                return Err(
                    "guidance acquisition: Git administrative paths cannot be selected".into(),
                );
            }
            value => {
                #[cfg(windows)]
                validate_windows_component(value)?;
                components.push(value);
            }
        }
    }
    Ok(components.join("/"))
}

#[cfg(windows)]
fn validate_windows_component(value: &str) -> Result<(), String> {
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered_device = (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && matches!(stem.as_bytes()[3], b'1'..=b'9');
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || numbered_device
        || value.ends_with(['.', ' '])
        || value
            .chars()
            .any(|character| character.is_control() || "<>\"|?*".contains(character))
    {
        return Err(format!(
            "guidance acquisition: source component '{value}' cannot be materialized safely on Windows"
        ));
    }
    Ok(())
}

impl Acquirer {
    fn bytes(&mut self, id: &str) -> Result<Vec<u8>, String> {
        let input = self.recipes.get(id).map(String::as_str).unwrap_or(id);
        let input = normalize(input)?;
        if self.materialized.contains(&input) {
            return fs::read(self.snapshot.join(&input)).map_err(|error| {
                format!("guidance acquisition: cannot read materialized '{input}': {error}")
            });
        }
        let location = self
            .locate(&input)?
            .ok_or_else(|| format!("guidance acquisition: missing required source '{input}'"))?;
        if location.entry.is_directory() {
            return Err(format!(
                "guidance acquisition: required source '{input}' is a directory"
            ));
        }
        let object = self
            .repositories
            .get_mut(&location.mount)
            .ok_or("guidance acquisition: missing source owner")?
            .cache
            .blob(&location.entry.oid)?;
        let destination = self.snapshot.join(&input);
        if destination.try_exists().map_err(|error| {
            format!("guidance acquisition: cannot inspect snapshot path '{input}': {error}")
        })? {
            return Err(format!(
                "guidance acquisition: source '{input}' aliases an already materialized snapshot path"
            ));
        }
        fs::create_dir_all(
            destination
                .parent()
                .ok_or("guidance acquisition: missing snapshot parent")?,
        )
        .map_err(|error| {
            format!("guidance acquisition: cannot create source directory: {error}")
        })?;
        fs::write(destination, &object.data).map_err(|error| {
            format!("guidance acquisition: cannot materialize '{input}': {error}")
        })?;
        self.materialized.insert(input);
        Ok(object.data)
    }
}

impl FallibleContentSource for Acquirer {
    type Error = String;
    fn read_text(&mut self, id: &str) -> Result<Option<String>, String> {
        String::from_utf8(self.bytes(id)?)
            .map(Some)
            .map_err(|_| format!("guidance acquisition: required guidance '{id}' is not UTF-8"))
    }
    fn exists(&mut self, id: &str) -> Result<bool, String> {
        let input = self
            .recipes
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned());
        Ok(self
            .locate(&input)?
            .is_some_and(|location| !location.entry.is_directory()))
    }
    fn escapes_root(&mut self, _id: &str) -> Result<bool, String> {
        Ok(false) // locate() rejects escaping and cyclic Git symlinks without OS symlinks.
    }
}

pub fn acquire(
    source: &str,
    selection: &[String],
    profile_path: Option<&Path>,
    snapshot: &Path,
    cache_root: &Path,
) -> Result<Profile, String> {
    let cache = CachedRepository::open_in(source, cache_root)?;
    let revision = cache.head_oid().to_owned();
    let identity = cache
        .directory()
        .parent()
        .and_then(Path::file_name)
        .ok_or("guidance acquisition: missing repository identity")?
        .to_string_lossy()
        .into_owned();
    let root = repository::Repository::new(source, cache)?;
    acquire_snapshot(root, selection, profile_path, snapshot, cache_root)
        .map_err(|error| format!("repository {identity}, revision {revision}: {error}"))
}

fn acquire_snapshot(
    root: repository::Repository,
    selection: &[String],
    profile_path: Option<&Path>,
    snapshot: &Path,
    cache_root: &Path,
) -> Result<Profile, String> {
    let mut acquirer = Acquirer {
        repositories: BTreeMap::from([(String::new(), root)]),
        cache_root: cache_root.to_owned(),
        snapshot: snapshot.to_owned(),
        recipes: BTreeMap::new(),
        materialized: BTreeSet::new(),
        empty_directories: BTreeSet::new(),
    };
    let mut profile = if let Some(path) = profile_path {
        let path = path
            .to_str()
            .ok_or("guidance acquisition: profile path is not UTF-8")?;
        let path = normalize(path)?;
        acquirer.bytes(&path)?;
        load_profile(&snapshot.join(path))?
    } else {
        synthesize_direct_profile(selection)?
    };
    let selected: BTreeSet<_> = selection.iter().map(String::as_str).collect();
    for id in &selected {
        if !profile.corpus.iter().any(|item| item.id() == *id) {
            return Err(format!(
                "selection '{id}' is not declared by profile '{}'",
                profile.id
            ));
        }
    }
    let mut roots = Vec::new();
    for item in &mut profile.corpus {
        if !selected.contains(item.id()) {
            continue;
        }
        match item {
            CorpusItem::Direct { paths, .. } => {
                let mut expanded = Vec::new();
                for path in paths.iter() {
                    expanded.extend(acquirer.select_path(path)?);
                }
                expanded.sort();
                expanded.dedup();
                for path in &expanded {
                    if !path.ends_with(".md") && !path.ends_with(".toml") {
                        return Err(format!(
                            "guidance acquisition: selected non-guidance source '{path}'"
                        ));
                    }
                }
                *paths = expanded;
                roots.extend(paths.iter().cloned());
            }
            CorpusItem::Recipe { id, steps } => {
                for step in steps {
                    let RecipeStep::Copy { from, to } = step else {
                        return Err(format!(
                            "guidance acquisition: unsupported recipe step in '{id}'"
                        ));
                    };
                    normalize(to)?;
                    if !to.ends_with(".md") && !to.ends_with(".toml") {
                        return Err(format!("guidance acquisition: unsafe recipe output '{to}'"));
                    }
                    let location = acquirer.locate(from)?.ok_or_else(|| {
                        format!("guidance acquisition: missing recipe input '{from}'")
                    })?;
                    if location.entry.is_directory() {
                        return Err(format!(
                            "guidance acquisition: recipe input '{from}' is a directory"
                        ));
                    }
                    if let Some(previous) = acquirer.recipes.insert(to.clone(), from.clone())
                        && previous != *from
                    {
                        return Err(format!(
                            "guidance acquisition: ambiguous recipe output '{to}'"
                        ));
                    }
                    roots.push(to.clone());
                }
            }
        }
    }
    let closure = compute_closure_fallible(&roots, &mut acquirer)?;
    let failures: Vec<_> = closure
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code != DiagnosticCode::Cycle)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    if !failures.is_empty() {
        return Err(format!("guidance acquisition: {}", failures.join("; ")));
    }
    for id in &closure.artifacts {
        acquirer.bytes(id)?;
    }
    for directory in &acquirer.empty_directories {
        fs::create_dir_all(snapshot.join(directory)).map_err(|error| {
            format!("guidance acquisition: cannot materialize selected directory: {error}")
        })?;
    }
    Ok(profile)
}
