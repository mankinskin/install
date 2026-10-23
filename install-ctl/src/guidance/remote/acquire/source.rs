use super::{
    Acquirer, normalize,
    repository::{Entry, Repository},
};
use crate::guidance::remote::cache::CachedRepository;
use std::collections::BTreeSet;

pub(super) struct Location {
    pub mount: String,
    pub path: String,
    pub entry: Entry,
}

impl Acquirer {
    pub(super) fn locate(&mut self, id: &str) -> Result<Option<Location>, String> {
        let mut path = normalize(id)?;
        let mut visited = BTreeSet::new();
        'resolve: for _ in 0..256 {
            if !visited.insert(path.clone()) {
                return Err(format!("guidance acquisition: symlink cycle at '{id}'"));
            }
            let mut mount = String::new();
            let mut relative = path.clone();
            'repository: loop {
                if relative.is_empty() {
                    let entry = self.repositories[&mount]
                        .entries
                        .get(b"".as_slice())
                        .ok_or("guidance acquisition: missing repository root tree")?
                        .clone();
                    return Ok(Some(Location {
                        mount,
                        path: relative,
                        entry,
                    }));
                }
                let segments: Vec<_> = relative.split('/').collect();
                for length in 1..=segments.len() {
                    let prefix = segments[..length].join("/");
                    let Some(entry) = self.repositories[&mount]
                        .entries
                        .get(prefix.as_bytes())
                        .cloned()
                    else {
                        return Ok(None);
                    };
                    let full = join(&mount, &prefix);
                    if entry.is_symlink() {
                        let target = self
                            .repositories
                            .get_mut(&mount)
                            .ok_or("guidance acquisition: missing symlink owner")?
                            .cache
                            .blob(&entry.oid)?;
                        let target = std::str::from_utf8(&target.data)
                            .map_err(|_| "guidance acquisition: symlink target is not UTF-8")?;
                        if target.starts_with(['/', '\\']) || target.contains([':', '\0', '\\']) {
                            return Err(format!(
                                "guidance acquisition: unsafe symlink target at '{full}'"
                            ));
                        }
                        let base = full.rsplit_once('/').map(|(base, _)| base).unwrap_or("");
                        path = normalize(&join(base, &join(target, &segments[length..].join("/"))))
                            .map_err(|error| format!("guidance acquisition: symlink '{full}' escapes source root: {error}"))?;
                        continue 'resolve;
                    }
                    if entry.is_gitlink() {
                        self.enter_child(&mount, &prefix, &full, &entry.oid)?;
                        mount = full;
                        relative = segments[length..].join("/");
                        continue 'repository;
                    }
                    if length == segments.len() {
                        return Ok(Some(Location {
                            mount,
                            path: prefix,
                            entry,
                        }));
                    }
                    if !entry.is_directory() {
                        return Ok(None);
                    }
                }
                return Ok(None);
            }
        }
        Err(format!(
            "guidance acquisition: symlink resolution exceeds 256 steps at '{id}'"
        ))
    }

    fn enter_child(
        &mut self,
        parent: &str,
        path: &str,
        mount: &str,
        oid: &str,
    ) -> Result<(), String> {
        if self.repositories.contains_key(mount) {
            return Ok(());
        }
        if mount.split('/').count() > 256 {
            return Err("guidance acquisition: submodule nesting exceeds 256 levels".into());
        }
        let source = super::modules::child_url(
            self.repositories
                .get_mut(parent)
                .ok_or("guidance acquisition: missing parent repository")?,
            path,
        )?;
        let cache = CachedRepository::pinned_in(&source, oid, &self.cache_root)
            .map_err(|error| format!("guidance acquisition: submodule '{mount}': {error}"))?;
        let repository = Repository::new(&source, cache)?;
        self.repositories.insert(mount.to_owned(), repository);
        Ok(())
    }

    pub(super) fn expand(
        &mut self,
        id: &str,
        ancestors: &mut BTreeSet<String>,
    ) -> Result<Vec<String>, String> {
        let location = self.locate(id)?.ok_or_else(|| {
            format!("guidance acquisition: selected source '{id}' does not exist")
        })?;
        if !location.entry.is_directory() {
            return Ok(vec![normalize(id)?]);
        }
        let canonical = join(&location.mount, &location.path);
        if !ancestors.insert(canonical.clone()) {
            return Err(format!(
                "guidance acquisition: directory symlink cycle at '{id}'"
            ));
        }
        self.empty_directories.insert(normalize(id)?);
        let prefix = if location.path.is_empty() {
            String::new()
        } else {
            format!("{}/", location.path)
        };
        let children: Vec<_> = self.repositories[&location.mount]
            .entries
            .keys()
            .filter_map(|path| path.strip_prefix(prefix.as_bytes()))
            .filter(|suffix| !suffix.is_empty() && !suffix.contains(&b'/'))
            .map(|suffix| {
                String::from_utf8(suffix.to_owned()).map_err(|_| {
                    "guidance acquisition: selected directory contains a non-UTF-8 name".to_owned()
                })
            })
            .collect::<Result<_, _>>()?;
        let mut expanded = Vec::new();
        for child in children {
            let path = join(id, &child);
            let location = self
                .locate(&path)?
                .ok_or("guidance acquisition: directory entry disappeared")?;
            if location.entry.is_directory() || child.ends_with(".md") || child.ends_with(".toml") {
                expanded.extend(self.expand(&path, ancestors)?);
            }
        }
        ancestors.remove(&canonical);
        expanded.sort();
        expanded.dedup();
        Ok(expanded)
    }

    pub(super) fn select_path(&mut self, id: &str) -> Result<Vec<String>, String> {
        if self.locate(id)?.is_some() {
            return self.expand(id, &mut BTreeSet::new());
        }
        if id.contains(['/', '\\']) {
            return Err(format!(
                "guidance acquisition: selected source '{id}' does not exist"
            ));
        }
        let mut pending = vec![String::new()];
        let mut matches = Vec::new();
        while let Some(mount) = pending.pop() {
            let entries: Vec<_> = self.repositories[&mount]
                .entries
                .iter()
                .map(|(path, entry)| (path.clone(), entry.clone()))
                .collect();
            for (path, entry) in entries {
                if path
                    .split(|byte| *byte == b'/')
                    .any(|part| matches!(part, b".git" | b"target" | b"node_modules"))
                {
                    continue;
                }
                if entry.is_gitlink() {
                    let path = String::from_utf8(path)
                        .map_err(|_| "guidance acquisition: non-UTF-8 candidate submodule path")?;
                    let full = join(&mount, &path);
                    self.enter_child(&mount, &path, &full, &entry.oid)?;
                    pending.push(full);
                } else if !entry.is_directory()
                    && path.rsplit(|byte| *byte == b'/').next() == Some(id.as_bytes())
                {
                    let path = String::from_utf8(path)
                        .map_err(|_| "guidance acquisition: non-UTF-8 candidate path")?;
                    matches.push(join(&mount, &path));
                }
            }
        }
        matches.sort();
        let found = matches.first().ok_or_else(|| {
            format!("guidance acquisition: selected source '{id}' does not exist")
        })?;
        self.expand(found, &mut BTreeSet::new())
    }
}

pub(super) fn join(base: &str, path: &str) -> String {
    match (base.is_empty(), path.is_empty()) {
        (true, _) => path.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!("{base}/{path}"),
    }
}
