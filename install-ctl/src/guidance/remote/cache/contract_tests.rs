use super::{CachedRepository, RepositoryLock};
use crate::guidance::remote::transport::tests::{Repository, Server, http_server};
use std::{
    fs,
    io::Write,
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn sha256_objects_are_verified_and_reused_in_their_hash_namespace() {
    let fixture = Repository::with_format(true, "sha256");
    let (_server, url, log) = http_server(&fixture, false);
    let root = fixture.root.path().join("cache");
    let mut first = CachedRepository::open_in(&url, &root).unwrap();
    first.metadata().unwrap();
    let blob = first.blob(&fixture.selected).unwrap();
    assert_eq!(blob.oid.len(), 64);
    blob.verify().unwrap();
    let mut second = CachedRepository::open_in(&url, &root).unwrap();
    second.metadata().unwrap();
    assert_eq!(
        second.blob(&fixture.selected).unwrap().data,
        b"# Selected\n"
    );
    let requests = fs::read_to_string(log).unwrap();
    assert_eq!(
        requests
            .matches(&format!("want {}", fixture.selected))
            .count(),
        1
    );
    assert_eq!(first.directory(), second.directory());
}

#[test]
fn corrupt_metadata_and_manifest_paths_are_recovered_by_verified_refetch() {
    let fixture = Repository::new(true);
    let root = fixture.root.path().join("cache");
    let mut remote = CachedRepository::open_in(fixture.repo.to_str().unwrap(), &root).unwrap();
    let metadata = remote.metadata().unwrap();
    let tree = metadata
        .iter()
        .find(|object| object.kind == "tree")
        .unwrap();
    fs::write(
        remote
            .directory()
            .join("objects")
            .join(format!("{}.obj", tree.oid)),
        b"tree\nbroken",
    )
    .unwrap();
    let repaired = remote.metadata().unwrap();
    for object in &repaired {
        object.verify().unwrap();
    }
    fs::write(
        remote.manifest_path(),
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "commit": fixture.head, "objects": ["../../outside"]
        }))
        .unwrap(),
    )
    .unwrap();
    let recovered = remote.metadata().unwrap();
    assert!(recovered.iter().any(|object| object.oid == fixture.head));
    let manifest: super::Manifest =
        serde_json::from_slice(&fs::read(remote.manifest_path()).unwrap()).unwrap();
    assert!(
        manifest
            .objects
            .iter()
            .all(|oid| oid.bytes().all(|byte| byte.is_ascii_hexdigit()))
    );
}

#[test]
fn blob_lookup_rejects_escapes_wrong_types_and_wrong_hash_formats() {
    let fixture = Repository::new(true);
    let mut remote = CachedRepository::open_in(
        fixture.repo.to_str().unwrap(),
        &fixture.root.path().join("cache"),
    )
    .unwrap();
    remote.metadata().unwrap();
    assert!(
        remote
            .blob("../../outside")
            .unwrap_err()
            .contains("invalid object ID")
    );
    assert!(
        remote
            .blob(&"1".repeat(64))
            .unwrap_err()
            .contains("hash format")
    );
    assert!(
        remote
            .blob(&fixture.head)
            .unwrap_err()
            .contains("not a blob")
    );
}

#[test]
fn cache_files_never_persist_source_credentials() {
    let fixture = Repository::new(true);
    let (_server, url, _log) = http_server(&fixture, false);
    let credential_url = url.replacen("http://", "http://secret-user:secret-pass@", 1);
    let mut remote =
        CachedRepository::open_in(&credential_url, &fixture.root.path().join("cache")).unwrap();
    remote.metadata().unwrap();
    remote.blob(&fixture.selected).unwrap();
    for directory in ["objects", "commits"] {
        for entry in fs::read_dir(remote.directory().join(directory)).unwrap() {
            let path = entry.unwrap().path();
            let bytes = fs::read(path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for forbidden in ["secret-user", "secret-pass", "http://", "Authorization"] {
                assert!(
                    !text.contains(forbidden),
                    "cache leaked source/authentication metadata"
                );
            }
        }
    }
}

#[test]
fn process_interruption_releases_lock_and_preserves_completed_objects() {
    let fixture = Repository::new(true);
    let root = fixture.root.path().join("cache");
    let ready = fixture.root.path().join("worker-ready");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "guidance::remote::cache::contract_tests::cache_crash_worker",
        ])
        .env("INSTALL_CTL_CACHE_CRASH_ROOT", &root)
        .env("INSTALL_CTL_CACHE_CRASH_SOURCE", &fixture.repo)
        .env("INSTALL_CTL_CACHE_CRASH_OID", &fixture.selected)
        .env("INSTALL_CTL_CACHE_CRASH_READY", &ready)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = Server(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && child.0.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ready.exists(),
        "cache crash worker did not become ready within 20 seconds"
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let mut remote = CachedRepository::open_in(fixture.repo.to_str().unwrap(), &root).unwrap();
    let object = remote
        .directory()
        .join("objects")
        .join(format!("{}.obj", fixture.selected));
    assert_eq!(fs::read(&object).unwrap(), b"blob\n# Selected\n");
    assert_eq!(
        remote.blob(&fixture.selected).unwrap().data,
        b"# Selected\n"
    );
    remote.metadata().unwrap();
}

#[test]
fn cache_crash_worker() {
    let Some(root) = std::env::var_os("INSTALL_CTL_CACHE_CRASH_ROOT") else {
        return;
    };
    let mut remote = CachedRepository::open_in(
        &std::env::var("INSTALL_CTL_CACHE_CRASH_SOURCE").unwrap(),
        std::path::Path::new(&root),
    )
    .unwrap();
    remote.metadata().unwrap();
    remote
        .blob(&std::env::var("INSTALL_CTL_CACHE_CRASH_OID").unwrap())
        .unwrap();
    let _lock = RepositoryLock::acquire(&remote.lock_path, Duration::from_secs(1)).unwrap();
    let mut partial = tempfile::NamedTempFile::new_in(remote.directory().join("commits")).unwrap();
    partial.write_all(b"{\"version\":").unwrap();
    fs::write(
        std::env::var_os("INSTALL_CTL_CACHE_CRASH_READY").unwrap(),
        b"ready",
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(60));
}
