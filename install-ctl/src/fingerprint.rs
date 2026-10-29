//! Conservative local build-input fingerprints, independent of Cargo's build cache.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{freshness, registry::Artifact};

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    path: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    source: String,
    binary: String,
}

pub struct Snapshot {
    pub sources: BTreeMap<String, String>,
}

impl Snapshot {
    pub fn capture(repo: &Path, artifacts: &[&Artifact]) -> Result<Self, String> {
        let mut packages = BTreeMap::new();
        let mut manifests = vec![repo.join("Cargo.toml")];
        let mut workspaces = BTreeSet::new();
        let mut common = Sha256::new();
        common.update(b"install-ctl-source-v1");
        while let Some(manifest) = manifests.pop() {
            let manifest = fs::canonicalize(&manifest)
                .map_err(|error| format!("cannot resolve {}: {error}", manifest.display()))?;
            if packages.contains_key(&manifest) {
                continue;
            }
            let metadata: Metadata = serde_json::from_slice(&output(
                Command::new("cargo")
                    .current_dir(repo)
                    .args([
                        "metadata",
                        "--format-version",
                        "1",
                        "--no-deps",
                        "--offline",
                        "--locked",
                    ])
                    .arg("--manifest-path")
                    .arg(&manifest),
            )?)
            .map_err(|error| {
                format!("invalid Cargo metadata for {}: {error}", manifest.display())
            })?;
            if !workspaces.insert(metadata.workspace_root.clone()) {
                continue;
            }
            let root_manifest = metadata.workspace_root.join("Cargo.toml");
            let document: toml::Value =
                toml::from_str(&fs::read_to_string(&root_manifest).map_err(|error| {
                    format!("cannot read {}: {error}", root_manifest.display())
                })?)
                .map_err(|error| format!("invalid {}: {error}", root_manifest.display()))?;
            if let Some(patches) = document.get("patch").and_then(toml::Value::as_table) {
                for source in patches.values().filter_map(toml::Value::as_table) {
                    for entry in source.values() {
                        if let Some(path) = entry.get("path").and_then(toml::Value::as_str) {
                            let manifest = fs::canonicalize(
                                metadata.workspace_root.join(path).join("Cargo.toml"),
                            )
                            .map_err(|error| format!("cannot resolve patch {path}: {error}"))?;
                            if !packages.contains_key(&manifest) {
                                manifests.push(manifest);
                            }
                        }
                    }
                }
            }
            for package in metadata.packages {
                for dependency in &package.dependencies {
                    if let Some(path) = &dependency.path {
                        manifests.push(path.join("Cargo.toml"));
                    }
                }
                let path = fs::canonicalize(&package.manifest_path).map_err(|error| {
                    format!(
                        "cannot resolve {}: {error}",
                        package.manifest_path.display()
                    )
                })?;
                packages.insert(path, package);
            }
        }
        for root in &workspaces {
            for file in ["Cargo.toml", "Cargo.lock"] {
                hash_optional(&mut common, &root.join(file))?;
            }
            for ancestor in root.ancestors() {
                for file in [
                    ".cargo/config",
                    ".cargo/config.toml",
                    "rust-toolchain",
                    "rust-toolchain.toml",
                ] {
                    hash_optional(&mut common, &ancestor.join(file))?;
                }
            }
        }
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")));
        if let Some(home) = cargo_home {
            for file in ["config", "config.toml"] {
                hash_optional(&mut common, &home.join(file))?;
            }
        }
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        common.update(output(Command::new(rustc).current_dir(repo).arg("-vV"))?);
        common.update(output(Command::new("cargo").current_dir(repo).arg("-V"))?);
        let environment: BTreeMap<_, _> = std::env::vars_os()
            .filter(|(key, _)| {
                let key = key.to_string_lossy();
                key.starts_with("RUST")
                    || key.starts_with("CARGO_")
                        && !matches!(
                            key.as_ref(),
                            "CARGO_TARGET_DIR"
                                | "CARGO_MAKEFLAGS"
                                | "CARGO_TERM_COLOR"
                                | "CARGO_TERM_PROGRESS_WHEN"
                                | "CARGO_TERM_PROGRESS_WIDTH"
                        )
                    || matches!(key.as_ref(), "CC" | "CXX" | "AR" | "CFLAGS" | "CXXFLAGS")
            })
            .collect();
        for (key, value) in environment {
            field(&mut common, key.to_string_lossy().as_bytes());
            field(&mut common, value.to_string_lossy().as_bytes());
        }
        let mut package_hashes = BTreeMap::new();
        let mut sources = BTreeMap::new();
        for artifact in artifacts {
            let manifest = fs::canonicalize(repo.join(&artifact.path).join("Cargo.toml"))
                .map_err(|error| format!("cannot resolve source for {}: {error}", artifact.id))?;
            let mut closure = BTreeSet::new();
            collect_packages(&manifest, &packages, &mut closure)?;
            let mut hash = common.clone();
            field(&mut hash, artifact.id.as_bytes());
            field(
                &mut hash,
                artifact.bin.as_deref().unwrap_or(&artifact.id).as_bytes(),
            );
            for feature in &artifact.features {
                field(&mut hash, feature.as_bytes());
            }
            for manifest in closure {
                let digest = match package_hashes.get(&manifest) {
                    Some(digest) => *digest,
                    None => {
                        let directory = manifest
                            .parent()
                            .ok_or_else(|| format!("no parent for {}", manifest.display()))?;
                        let mut tree_hash = Sha256::new();
                        hash_tree(&mut tree_hash, directory, &mut BTreeSet::new())?;
                        let digest: [u8; 32] = tree_hash.finalize().into();
                        package_hashes.insert(manifest.clone(), digest);
                        digest
                    }
                };
                field(&mut hash, manifest.to_string_lossy().as_bytes());
                hash.update(digest);
            }
            sources.insert(artifact.id.clone(), format!("{:x}", hash.finalize()));
        }
        Ok(Self { sources })
    }

    pub fn source(&self, artifact: &Artifact) -> &str {
        &self.sources[&artifact.id]
    }
}

fn collect_packages(
    manifest: &Path,
    packages: &BTreeMap<PathBuf, Package>,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    if !seen.insert(manifest.to_path_buf()) {
        return Ok(());
    }
    let package = packages.get(manifest).ok_or_else(|| {
        format!(
            "local package missing from Cargo metadata: {}",
            manifest.display()
        )
    })?;
    for dependency in &package.dependencies {
        if let Some(path) = &dependency.path {
            let path = fs::canonicalize(path.join("Cargo.toml")).map_err(|error| {
                format!("cannot resolve dependency {}: {error}", path.display())
            })?;
            collect_packages(&path, packages, seen)?;
        } else {
            // Include matching workspace/patch packages conservatively, including
            // optional, target-specific and development dependencies.
            for (path, candidate) in packages {
                if candidate.name == dependency.name {
                    collect_packages(path, packages, seen)?;
                }
            }
        }
    }
    Ok(())
}

fn output(command: &mut Command) -> Result<Vec<u8>, String> {
    let result = command
        .output()
        .map_err(|error| format!("cannot run {command:?}: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "{command:?} failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(result.stdout)
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash_file(&mut hash, path)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn hash_file(hash: &mut Sha256, path: &Path) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut buffer = [0; 65536];
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(())
}

fn hash_optional(hash: &mut Sha256, path: &Path) -> Result<(), String> {
    field(hash, path.to_string_lossy().as_bytes());
    match fs::metadata(path) {
        Ok(_) => {
            hash.update([1]);
            hash_file(hash, path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hash.update([0]);
            Ok(())
        }
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn hash_tree(
    hash: &mut Sha256,
    path: &Path,
    parents: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if !parents.insert(canonical.clone()) {
        return Err(format!(
            "source directory symlink cycle: {}",
            path.display()
        ));
    }
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("cannot list {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(
                ".git"
                    | "target"
                    | "node_modules"
                    | ".workflow-tools"
                    | ".install-ctl"
                    | ".ticket"
                    | ".spec"
                    | ".test"
                    | ".session"
                    | ".feedback"
            )
        ) {
            continue;
        }
        field(hash, name.to_string_lossy().as_bytes());
        let metadata = fs::metadata(entry.path())
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if metadata.is_dir() {
            hash.update([0]);
            hash_tree(hash, &entry.path(), parents)?;
        } else if metadata.is_file() {
            hash.update([1]);
            hash.update(metadata.len().to_le_bytes());
            hash_file(hash, &entry.path())?;
        } else {
            return Err(format!(
                "unsupported source file: {}",
                entry.path().display()
            ));
        }
    }
    parents.remove(&canonical);
    hash.update([2]);
    Ok(())
}

fn receipt_path(root: &Path, artifact: &Artifact) -> PathBuf {
    root.join(".install-ctl")
        .join(format!("{:x}.json", Sha256::digest(artifact.id.as_bytes())))
}

pub fn is_current(root: &Path, artifact: &Artifact, source: &str) -> Result<bool, String> {
    let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
    if !freshness::is_installed(root, bin)? {
        return Ok(false);
    }
    let path = receipt_path(root, artifact);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "{}: no source fingerprint; reinstall once to establish provenance",
                artifact.id
            );
            return Ok(false);
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid receipt {}: {error}", path.display()))?;
    if receipt.version != 1 {
        return Err(format!("unsupported receipt version in {}", path.display()));
    }
    Ok(receipt.source == source
        && receipt.binary == file_hash(&freshness::installed_path(root, bin))?)
}

pub fn record(root: &Path, artifact: &Artifact, source: &str) -> Result<(), String> {
    let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
    let receipt = Receipt {
        version: 1,
        source: source.into(),
        binary: file_hash(&freshness::installed_path(root, bin))?,
    };
    let path = receipt_path(root, artifact);
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent for {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut staged, &receipt).map_err(|error| error.to_string())?;
    staged.flush().map_err(|error| error.to_string())?;
    staged
        .persist(&path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(())
}
