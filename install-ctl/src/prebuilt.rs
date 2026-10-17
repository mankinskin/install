//! Verified local prebuilt bundle installation; never falls back to Cargo.
use std::{collections::HashSet, fs, io::Write, path::Path};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    apply_report::{ActionStatus, ApplyAction, ApplyReport},
    plan::{InstallPlan, exclusion_reason},
    registry::RegistryEntryV2,
};

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    target: String,
    tools: Vec<Tool>,
}

#[derive(Deserialize)]
struct Tool {
    id: String,
    path: String,
    sha256: String,
}

struct Verified {
    id: String,
    bin: String,
    hash: String,
    bytes: Vec<u8>,
}

pub fn apply(
    plan: &InstallPlan,
    bundle: &Path,
    manifest_path: &Path,
    registry: &[RegistryEntryV2],
    dry_run: bool,
) -> ApplyReport {
    let selected = plan
        .selected
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let mut report = match install_at_root(
        &selected,
        bundle,
        manifest_path,
        &plan.root,
        registry,
        dry_run,
    ) {
        Ok(report) => report,
        Err(error) => {
            let mut report = ApplyReport::default();
            report.push(ApplyAction {
                id: "prebuilt".into(),
                target: plan.root.clone(),
                status: ActionStatus::Failed,
                detail: error,
            });
            report
        }
    };
    for excluded in &plan.excluded {
        report.push(ApplyAction {
            id: excluded.id.clone(),
            target: plan.root.clone(),
            status: ActionStatus::Skipped,
            detail: excluded.reason.clone(),
        });
    }
    report
}

fn install_at_root(
    selection: &[String],
    bundle: &Path,
    manifest_path: &Path,
    tools_root: &Path,
    registry: &[RegistryEntryV2],
    dry_run: bool,
) -> Result<ApplyReport, String> {
    if selection.is_empty() {
        return Ok(ApplyReport::default());
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid prebuilt manifest: {error}"))?;
    if manifest.version != 1 {
        return Err(format!(
            "unsupported prebuilt manifest version: {}",
            manifest.version
        ));
    }
    let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    if manifest.target != target {
        return Err(format!(
            "prebuilt bundle target '{}' does not match '{target}'",
            manifest.target
        ));
    }
    let bundle =
        fs::canonicalize(bundle).map_err(|error| format!("invalid bundle directory: {error}"))?;
    let mut ids = HashSet::new();
    let mut manifest_ids = HashSet::new();
    for tool in &manifest.tools {
        if !manifest_ids.insert(&tool.id) {
            return Err(format!("duplicate manifest tool id: {}", tool.id));
        }
    }
    let cache = tools_root.join("cache");
    let bins = tools_root.join("bin");
    for directory in [tools_root, &cache, &bins] {
        reject_link(directory)?;
    }
    let mut destinations = HashSet::new();
    let mut verified = Vec::new();
    // Complete preflight: source and already-existing cache are checked before
    // creating cache/bin directories or writing any destination.
    for id in selection {
        if !ids.insert(id) {
            return Err(format!("duplicate selected tool id: {id}"));
        }
        let entry = registry
            .iter()
            .find(|entry| entry.id == *id)
            .ok_or_else(|| format!("unknown registry tool: {id}"))?;
        if let Some(reason) = exclusion_reason(entry) {
            return Err(format!("prebuilt tool {id} is not installable: {reason}"));
        }
        let bin = entry.bin.as_deref().unwrap_or(id);
        if bin.is_empty() || bin.contains(['/', '\\', ':']) || bin == "." || bin == ".." {
            return Err(format!("unsafe registry binary name for {id}"));
        }
        let destination = bins.join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
        if !destinations.insert(destination.clone()) {
            return Err(format!("duplicate prebuilt destination for {id}"));
        }
        reject_link(&destination)?;
        if let (Ok(destination), Ok(current)) = (
            fs::canonicalize(&destination),
            std::env::current_exe().and_then(fs::canonicalize),
        ) {
            if destination == current {
                return Err(format!(
                    "cannot replace running installer: {}",
                    destination.display()
                ));
            }
        }
        let tool = manifest
            .tools
            .iter()
            .find(|tool| tool.id == *id)
            .ok_or_else(|| format!("prebuilt manifest has no tool: {id}"))?;
        if tool.sha256.len() != 64 || !tool.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid SHA-256 for {id}"));
        }
        let relative = Path::new(&tool.path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(format!("unsafe bundle path for {id}"));
        }
        let source = fs::canonicalize(bundle.join(relative))
            .map_err(|error| format!("missing bundle file for {id}: {error}"))?;
        if !source.starts_with(&bundle)
            || !fs::metadata(&source).map_err(|e| e.to_string())?.is_file()
        {
            return Err(format!(
                "bundle source is not a regular in-bundle file for {id}"
            ));
        }
        let bytes = fs::read(&source).map_err(|error| error.to_string())?;
        let hash = tool.sha256.to_ascii_lowercase();
        if digest(&bytes) != hash {
            return Err(format!("SHA-256 mismatch for {id}"));
        }
        let cached = cache.join(&hash);
        reject_link(&cached)?;
        if cached.exists() {
            let metadata = fs::metadata(&cached).map_err(|error| error.to_string())?;
            if !metadata.is_file()
                || digest(&fs::read(&cached).map_err(|error| error.to_string())?) != hash
            {
                return Err(format!("cached SHA-256 mismatch for {id}"));
            }
        }
        verified.push(Verified {
            id: id.clone(),
            bin: bin.into(),
            hash,
            bytes,
        });
    }
    let mut report = ApplyReport::default();
    for item in verified {
        let destination = bins.join(format!("{}{}", item.bin, std::env::consts::EXE_SUFFIX));
        let (status, detail) = if report.has_failures() {
            (
                ActionStatus::Skipped,
                "not executed after an earlier failure".into(),
            )
        } else if dry_run {
            (
                ActionStatus::Skipped,
                "verified local bundle; dry-run, no writes".into(),
            )
        } else {
            match write_verified(&item, &cache, &destination) {
                Ok(unchanged) => (
                    if unchanged {
                        ActionStatus::Unchanged
                    } else {
                        ActionStatus::Written
                    },
                    "verified local bundle; no Cargo invoked".into(),
                ),
                Err(error) => (ActionStatus::Failed, error),
            }
        };
        report.push(ApplyAction {
            id: item.id,
            target: destination,
            status,
            detail,
        });
    }
    Ok(report)
}

fn reject_link(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "prebuilt destination/cache link is not allowed: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn write_verified(item: &Verified, cache: &Path, destination: &Path) -> Result<bool, String> {
    let cached = cache.join(&item.hash);
    if !cached.exists() {
        atomic_executable(&cached, &item.bytes)?;
    }
    let unchanged = match fs::read(destination) {
        Ok(bytes) => digest(&bytes) == item.hash,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("cannot read {}: {error}", destination.display())),
    };
    if unchanged {
        set_executable(destination)?;
    } else {
        atomic_executable(destination, &item.bytes)?;
    }
    Ok(unchanged)
}

fn atomic_executable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent for {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    staged.write_all(bytes).map_err(|error| error.to_string())?;
    set_executable(staged.path())?;
    staged
        .persist(path)
        .map_err(|error| format!("cannot install {}: {error}", path.display()))?;
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())
}
#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), String> {
    Ok(())
}
