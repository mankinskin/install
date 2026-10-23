//! Verified immutable storage; a live transport check always precedes cache access.

#[cfg(test)]
mod contract_tests;
mod storage;
#[cfg(test)]
mod tests;

use super::transport::{ReceivedObject, SelectiveTransport, validate_metadata};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct CachedRepository {
    transport: SelectiveTransport,
    directory: PathBuf,
    lock_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    commit: String,
    objects: Vec<String>,
}

struct RepositoryLock {
    _file: File,
}

impl RepositoryLock {
    fn acquire(path: &Path, budget: Duration) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| format!("guidance cache: cannot open repository lock: {error}"))?;
        let started = Instant::now();
        loop {
            match fs4::FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { _file: file }),
                Err(error)
                    if error.raw_os_error() == fs4::lock_contended_error().raw_os_error() =>
                {
                    if started.elapsed() >= budget {
                        return Err(format!(
                            "guidance cache contention: repository lock {} remained busy; retry after the other guidance operation finishes",
                            path.display()
                        ));
                    }
                    std::thread::sleep(
                        Duration::from_millis(10).min(budget.saturating_sub(started.elapsed())),
                    );
                }
                Err(error) => {
                    return Err(format!("guidance cache: cannot lock repository: {error}"));
                }
            }
        }
    }
}

pub fn default_root() -> Result<PathBuf, String> {
    dirs::cache_dir()
        .map(|root| {
            root.join("workflow-tools")
                .join("install-ctl")
                .join("guidance")
                .join("v1")
        })
        .ok_or_else(|| "guidance cache: operating-system cache directory is unavailable".into())
}

impl CachedRepository {
    pub fn open_in(source: &str, root: &Path) -> Result<Self, String> {
        Self::from_transport(SelectiveTransport::connect(source)?, root)
    }

    pub fn pinned_in(source: &str, revision: &str, root: &Path) -> Result<Self, String> {
        let mut repository =
            Self::from_transport(SelectiveTransport::pinned(source, revision)?, root)?;
        // A cached child commit does not prove the remote still makes that pinned revision available.
        repository.metadata_inner(true)?;
        Ok(repository)
    }

    fn from_transport(transport: SelectiveTransport, root: &Path) -> Result<Self, String> {
        let format = gix_hash::ObjectId::from_hex(transport.head_oid().as_bytes())
            .map_err(|_| "guidance cache: transport returned an invalid revision")?
            .kind();
        let repository = root.join(transport.repository_key());
        let directory = repository.join(format.to_string());
        for name in ["objects", "commits"] {
            fs::create_dir_all(directory.join(name)).map_err(|error| {
                format!("guidance cache: cannot create storage directory: {error}")
            })?;
        }
        Ok(Self {
            transport,
            directory,
            lock_path: repository.join("repository.lock"),
        })
    }

    pub fn head_oid(&self) -> &str {
        self.transport.head_oid()
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn lock(&self) -> Result<RepositoryLock, String> {
        RepositoryLock::acquire(&self.lock_path, Duration::from_secs(30))
    }

    pub fn metadata(&mut self) -> Result<Vec<ReceivedObject>, String> {
        self.metadata_inner(false)
    }

    fn metadata_inner(&mut self, force_remote: bool) -> Result<Vec<ReceivedObject>, String> {
        let _lock = self.lock()?;
        if !force_remote && let Some(objects) = self.read_metadata()? {
            return Ok(objects);
        }
        let revision = self.head_oid().to_owned();
        let objects = self.transport.fetch_metadata(&revision)?;
        validate_metadata(&objects, self.head_oid())?;
        for object in &objects {
            storage::write_object(&self.directory, object)?;
        }
        let manifest = Manifest {
            version: 1,
            commit: self.head_oid().to_owned(),
            objects: objects.iter().map(|object| object.oid.clone()).collect(),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|error| {
            format!("guidance cache: cannot serialize commit manifest: {error}")
        })?;
        storage::publish(&self.manifest_path(), &bytes)?;
        self.read_metadata()?
            .ok_or_else(|| "guidance cache: published metadata failed read-back".into())
    }

    fn manifest_path(&self) -> PathBuf {
        self.directory
            .join("commits")
            .join(format!("{}.json", self.head_oid()))
    }

    fn read_metadata(&self) -> Result<Option<Vec<ReceivedObject>>, String> {
        let Some(bytes) = storage::read(&self.manifest_path())? else {
            return Ok(None);
        };
        let manifest: Manifest = match serde_json::from_slice(&bytes) {
            Ok(manifest) => manifest,
            Err(_) => {
                return Ok(storage::corruption(
                    "malformed commit manifest; refetching metadata",
                ));
            }
        };
        if manifest.version != 1 || manifest.commit != self.head_oid() {
            return Ok(storage::corruption(
                "commit manifest identity mismatch; refetching metadata",
            ));
        }
        let mut objects = Vec::with_capacity(manifest.objects.len());
        for oid in &manifest.objects {
            if self.normalize_oid(oid).as_deref() != Ok(oid.as_str()) {
                return Ok(storage::corruption(
                    "invalid object ID in manifest; refetching metadata",
                ));
            }
            let Some(object) = storage::read_object(&self.directory, oid)? else {
                return Ok(storage::corruption(
                    "incomplete cached metadata; refetching metadata",
                ));
            };
            objects.push(object);
        }
        if let Err(error) = validate_metadata(&objects, self.head_oid()) {
            return Ok(storage::corruption(&format!(
                "{error}; refetching metadata"
            )));
        }
        Ok(Some(objects))
    }

    fn normalize_oid(&self, oid: &str) -> Result<String, String> {
        let parsed = gix_hash::ObjectId::from_hex(oid.as_bytes())
            .map_err(|_| "guidance cache: invalid object ID")?;
        if oid.len() != self.head_oid().len() {
            return Err("guidance cache: object hash format does not match repository".into());
        }
        Ok(parsed.to_string())
    }

    pub fn blob(&mut self, oid: &str) -> Result<ReceivedObject, String> {
        let oid = self.normalize_oid(oid)?;
        let _lock = self.lock()?;
        if let Some(object) = storage::read_object(&self.directory, &oid)? {
            if object.kind != "blob" {
                return Err("guidance cache: selected object is not a blob".into());
            }
            return Ok(object);
        }
        let object = self.transport.fetch_blob(&oid)?;
        storage::write_object(&self.directory, &object)?;
        storage::read_object(&self.directory, &oid)?
            .ok_or_else(|| "guidance cache: published blob failed read-back".into())
    }
}
