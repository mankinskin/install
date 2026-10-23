use super::{normalize, repository::Repository};
use std::{collections::BTreeMap, io::Write, process::Command};

pub(super) fn child_url(repository: &mut Repository, path: &str) -> Result<String, String> {
    if repository.modules.is_none() {
        let entry = repository
            .entries
            .get(b".gitmodules".as_slice())
            .ok_or_else(|| {
                format!("guidance acquisition: submodule '{path}' has no .gitmodules mapping")
            })?
            .clone();
        if entry.is_symlink() || entry.is_directory() || entry.is_gitlink() {
            return Err("guidance acquisition: .gitmodules must be a regular file".into());
        }
        let object = repository.cache.blob(&entry.oid)?;
        let mut file = tempfile::NamedTempFile::new().map_err(|error| {
            format!("guidance acquisition: cannot prepare .gitmodules parser: {error}")
        })?;
        file.write_all(&object.data).map_err(|error| {
            format!("guidance acquisition: cannot write .gitmodules parser input: {error}")
        })?;
        let output = Command::new("git")
            .args(["config", "--no-includes", "--null", "--file"])
            .arg(file.path())
            .args(["--get-regexp", r"^submodule\..*\.(path|url)$"])
            .output()
            .map_err(|_| "guidance acquisition: cannot start Git .gitmodules parser")?;
        if !output.status.success() {
            return Err("guidance acquisition: .gitmodules is malformed or lacks mappings".into());
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| "guidance acquisition: non-UTF-8 submodule mapping")?;
        let mut fields = BTreeMap::new();
        for record in text.split('\0').filter(|value| !value.is_empty()) {
            let (key, value) = record
                .split_once('\n')
                .ok_or("guidance acquisition: invalid Git submodule parser response")?;
            if fields.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err("guidance acquisition: ambiguous duplicate submodule mapping".into());
            }
        }
        let mut modules = BTreeMap::new();
        for (key, value) in &fields {
            if let Some(base) = key.strip_suffix(".path") {
                let url = fields
                    .get(&format!("{base}.url"))
                    .ok_or("guidance acquisition: submodule mapping lacks a URL")?;
                if modules.insert(normalize(value)?, url.clone()).is_some() {
                    return Err("guidance acquisition: multiple submodules own one path".into());
                }
            }
        }
        repository.modules = Some(modules);
    }
    let relative = repository
        .modules
        .as_ref()
        .and_then(|modules| modules.get(path))
        .ok_or_else(|| {
            format!("guidance acquisition: missing URL mapping for submodule '{path}'")
        })?;
    resolve_url(&repository.source, relative)
}

pub(super) fn resolve_url(parent: &str, child: &str) -> Result<String, String> {
    if !child.starts_with("./") && !child.starts_with("../") {
        if child.is_empty() || child.contains(['\0', '\r', '\n']) {
            return Err("guidance acquisition: invalid submodule URL".into());
        }
        return Ok(child.to_owned());
    }
    let mut url = gix_url::parse(parent)
        .map_err(|_| "guidance acquisition: cannot resolve relative submodule URL")?;
    if url.scheme == gix_url::Scheme::File {
        url.canonicalize(
            &std::env::current_dir()
                .map_err(|_| "guidance acquisition: missing source directory")?,
        )
        .map_err(|_| "guidance acquisition: cannot canonicalize local parent URL")?;
    }
    let path = std::str::from_utf8(&url.path)
        .map_err(|_| "guidance acquisition: non-UTF-8 parent URL path")?
        .replace('\\', "/");
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    for part in child.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts
                    .pop()
                    .ok_or("guidance acquisition: relative submodule URL escapes its host")?;
            }
            value => parts.push(value),
        }
    }
    url.path = format!("{}{}", if absolute { "/" } else { "" }, parts.join("/")).into();
    String::from_utf8(url.to_bstring().into())
        .map_err(|_| "guidance acquisition: invalid resolved submodule URL")
        .map_err(str::to_owned)
}
