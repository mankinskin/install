use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use clap::ValueEnum;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    CopilotCli,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkOperation {
    Link,
    Unlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkAction {
    Create,
    Unchanged,
    Remove,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkEntry {
    pub source: String,
    pub destination: String,
    pub target: PathBuf,
    pub action: LinkAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LinkPlan {
    pub repo: PathBuf,
    pub harness: Harness,
    pub operation: LinkOperation,
    pub entries: Vec<LinkEntry>,
    pub skipped_sources: Vec<String>,
    pub diagnostics: Vec<String>,
}

const COPILOT_MAPPINGS: &[(&str, &str)] = &[
    (".agents/agents", ".github/agents"),
    (".agents/instructions", ".github/instructions"),
    (".agents/prompts", ".claude/commands"),
];

pub fn build_link_plan(
    repo: &Path,
    harness: Harness,
    operation: LinkOperation,
) -> Result<LinkPlan, String> {
    let repo = fs::canonicalize(repo)
        .map_err(|error| format!("cannot resolve repository '{}': {error}", repo.display()))?;
    if !repo.is_dir() {
        return Err(format!(
            "repository '{}' is not a directory",
            repo.display()
        ));
    }
    let mut plan = LinkPlan {
        repo,
        harness,
        operation,
        entries: Vec::new(),
        skipped_sources: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mappings = match harness {
        Harness::CopilotCli => COPILOT_MAPPINGS,
    };
    for &(source, destination) in mappings {
        if operation == LinkOperation::Link {
            match fs::symlink_metadata(plan.repo.join(source)) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    plan.skipped_sources.push(source.to_owned());
                    continue;
                }
                Err(error) => {
                    plan.diagnostics
                        .push(format!("cannot inspect source '{source}': {error}"));
                    continue;
                }
                Ok(_) => {}
            }
            match fs::canonicalize(plan.repo.join(source)) {
                Ok(real) if real.starts_with(&plan.repo) && real.is_dir() => {}
                Ok(_) => {
                    plan.diagnostics.push(format!(
                        "source '{source}' must be a directory inside the repository"
                    ));
                    continue;
                }
                Err(error) => {
                    plan.diagnostics
                        .push(format!("cannot resolve source '{source}': {error}"));
                    continue;
                }
            }
        }
        let target = relative_target(source);
        match inspect_destination(&plan.repo, source, destination) {
            Ok(present) => {
                let action = match (operation, present) {
                    (LinkOperation::Link, false) => LinkAction::Create,
                    (LinkOperation::Link, true) => LinkAction::Unchanged,
                    (LinkOperation::Unlink, false) => LinkAction::Missing,
                    (LinkOperation::Unlink, true) => LinkAction::Remove,
                };
                plan.entries.push(LinkEntry {
                    source: source.to_owned(),
                    destination: destination.to_owned(),
                    target,
                    action,
                });
            }
            Err(error) => plan.diagnostics.push(error),
        }
    }
    if operation == LinkOperation::Link && plan.entries.is_empty() && plan.diagnostics.is_empty() {
        plan.diagnostics
            .push("no guidance source directories exist under .agents".to_owned());
    }
    Ok(plan)
}

fn relative_target(source: &str) -> PathBuf {
    let mut target = PathBuf::from("..");
    for component in Path::new(source).components() {
        target.push(component.as_os_str());
    }
    target
}

fn inspect_destination(repo: &Path, source: &str, destination: &str) -> Result<bool, String> {
    let path = repo.join(destination);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid destination '{destination}'"))?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if !ordinary_directory(&metadata) => {
            return Err(format!(
                "destination ancestor '{}' is not an ordinary directory",
                parent.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot inspect '{}': {error}", parent.display())),
    }
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot inspect '{destination}': {error}")),
    };
    if !real_symlink(&path, &metadata)? {
        return Err(format!(
            "destination '{destination}' is not a symbolic link; refusing to overwrite or remove it"
        ));
    }
    let target = fs::read_link(&path)
        .map_err(|error| format!("cannot read link '{destination}': {error}"))?;
    let expected = repo.join(source);
    let resolved = normalize(&parent.join(target));
    let matches = resolved == expected
        || fs::canonicalize(&resolved)
            .ok()
            .zip(fs::canonicalize(&expected).ok())
            .is_some_and(|(actual, intended)| actual == intended && actual.starts_with(repo));
    if !matches {
        return Err(format!(
            "destination '{destination}' points elsewhere; expected '{source}'"
        ));
    }
    Ok(true)
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn ordinary_directory(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_dir() && metadata.file_attributes() & 0x400 == 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_dir()
    }
}

fn real_symlink(path: &Path, metadata: &fs::Metadata) -> Result<bool, String> {
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FileAttributeTagInfo, GetFileInformationByHandleEx,
        };
        let file = fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(7)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .map_err(|error| format!("cannot inspect reparse tag '{}': {error}", path.display()))?;
        let mut information = FILE_ATTRIBUTE_TAG_INFO {
            FileAttributes: 0,
            ReparseTag: 0,
        };
        let success = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                (&mut information as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if success == 0 {
            return Err(format!(
                "cannot inspect reparse tag '{}': {}",
                path.display(),
                io::Error::last_os_error()
            ));
        }
        Ok(information.ReparseTag == 0xA000000C)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(true)
    }
}

pub fn apply_link_plan(plan: &LinkPlan) -> Result<(), String> {
    apply_with_creator(plan, create_symlink)
}

fn apply_with_creator(
    plan: &LinkPlan,
    mut create: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    if !plan.diagnostics.is_empty() {
        return Err(format!(
            "refusing to apply blocking link plan: {}",
            plan.diagnostics.join("; ")
        ));
    }
    if build_link_plan(&plan.repo, plan.harness, plan.operation)? != *plan {
        return Err("link plan is stale; run --plan again before applying".to_owned());
    }
    let mut created_links: Vec<&LinkEntry> = Vec::new();
    let mut created_dirs = Vec::new();
    let result = (|| {
        for entry in &plan.entries {
            let present = inspect_destination(&plan.repo, &entry.source, &entry.destination)?;
            let destination = plan.repo.join(&entry.destination);
            match entry.action {
                LinkAction::Create if !present => {
                    let real =
                        fs::canonicalize(plan.repo.join(&entry.source)).map_err(|error| {
                            format!("cannot resolve source '{}': {error}", entry.source)
                        })?;
                    if !real.starts_with(&plan.repo) || !real.is_dir() {
                        return Err(format!(
                            "source '{}' changed; refusing to create link",
                            entry.source
                        ));
                    }
                    let parent = destination
                        .parent()
                        .ok_or_else(|| "invalid destination".to_owned())?;
                    if !parent.exists() {
                        fs::create_dir(parent).map_err(|error| {
                            format!("cannot create '{}': {error}", parent.display())
                        })?;
                        created_dirs.push(parent.to_path_buf());
                    }
                    create(&entry.target, &destination).map_err(|error| {
                        let advice = if cfg!(windows) {
                            "; enable Windows Developer Mode or run the command yourself as administrator"
                        } else { "" };
                        format!("cannot create symbolic link '{}': {error}{advice}; no copy or junction fallback", destination.display())
                    })?;
                    created_links.push(entry);
                }
                LinkAction::Remove if present => remove_symlink(&destination).map_err(|error| {
                    format!(
                        "cannot remove symbolic link '{}': {error}",
                        destination.display()
                    )
                })?,
                LinkAction::Unchanged if present => {}
                LinkAction::Missing if !present => {}
                _ => {
                    return Err(format!(
                        "destination '{}' changed since planning",
                        entry.destination
                    ));
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for entry in created_links.into_iter().rev() {
            match inspect_destination(&plan.repo, &entry.source, &entry.destination) {
                Ok(true) => {
                    if let Err(failure) = remove_symlink(&plan.repo.join(&entry.destination)) {
                        rollback_errors.push(format!("{}: {failure}", entry.destination));
                    }
                }
                Ok(false) => {}
                Err(failure) => rollback_errors.push(failure),
            }
        }
        for directory in created_dirs.into_iter().rev() {
            if let Err(failure) = fs::remove_dir(&directory) {
                rollback_errors.push(format!("{}: {failure}", directory.display()));
            }
        }
        return if rollback_errors.is_empty() {
            Err(error)
        } else {
            Err(format!(
                "{error}; rollback failures: {}",
                rollback_errors.join("; ")
            ))
        };
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, destination)
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are not supported on this platform",
    ))
}

fn remove_symlink(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        fs::remove_dir(path)
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(path)
    }
}

#[cfg(test)]
#[path = "links/tests.rs"]
mod tests;
