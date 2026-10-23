use super::{CachedRepository, RepositoryLock};
use crate::guidance::remote::transport::{
    config,
    tests::{Repository, git, http_server, http_server_mode},
};
use std::{
    fs,
    io::Write,
    time::{Duration, Instant},
};

fn blob_wants(log: &std::path::Path, oid: &str) -> usize {
    fs::read_to_string(log)
        .unwrap()
        .matches(&format!("want {oid}"))
        .count()
}

#[test]
fn canonical_identity_discards_credentials_query_and_default_port() {
    let secret = config::repository_key("https://secret-user:secret-pass@EXAMPLE.invalid:443/repo/?token=secret-query#secret-fragment").unwrap();
    let clean = config::repository_key("https://example.invalid/repo").unwrap();
    assert_eq!(secret, clean);
    assert_eq!(secret.len(), 64);
    assert!(secret.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn unchanged_head_rechecks_remote_but_receives_zero_cached_blobs() {
    let fixture = Repository::new(true);
    let (server, url, log) = http_server(&fixture, false);
    let root = fixture.root.path().join("cache");
    let mut first = CachedRepository::open_in(&url, &root).unwrap();
    let metadata = first.metadata().unwrap();
    assert!(metadata.iter().all(|object| object.kind != "blob"));
    assert_eq!(first.blob(&fixture.selected).unwrap().data, b"# Selected\n");
    let directory = first.directory().to_owned();
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            directory
                .join("commits")
                .join(format!("{}.json", fixture.head)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["commit"], fixture.head);
    assert!(
        !directory
            .join("objects")
            .join(format!("{}.obj", fixture.unrelated))
            .exists()
    );
    let initial_requests = fs::read_to_string(&log).unwrap();
    assert_eq!(blob_wants(&log, &fixture.selected), 1);
    drop(first);
    let mut second = CachedRepository::open_in(&url, &root).unwrap();
    assert_eq!(second.head_oid(), fixture.head);
    second.metadata().unwrap();
    assert_eq!(
        second.blob(&fixture.selected).unwrap().data,
        b"# Selected\n"
    );
    assert_eq!(blob_wants(&log, &fixture.selected), 1);
    let requests = fs::read_to_string(&log).unwrap();
    assert!(
        requests.len() > initial_requests.len(),
        "cached success skipped the live remote"
    );
    let suffix = &requests[initial_requests.len()..];
    assert!(suffix.contains("command=ls-refs"));
    assert!(!suffix.contains("command=fetch"));
    if let Some(evidence) = std::env::var_os("GUIDANCE_CACHE_EVIDENCE_ROOT") {
        let evidence = std::path::Path::new(&evidence);
        fs::create_dir_all(evidence).unwrap();
        fs::copy(
            directory
                .join("commits")
                .join(format!("{}.json", fixture.head)),
            evidence.join("commit.json"),
        )
        .unwrap();
        fs::copy(
            directory
                .join("objects")
                .join(format!("{}.obj", fixture.selected)),
            evidence.join("selected.obj"),
        )
        .unwrap();
        fs::copy(&log, evidence.join("requests.jsonl")).unwrap();
        fs::write(
            evidence.join("inventory.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "head": fixture.head,
                "selected": fixture.selected,
                "unrelated": fixture.unrelated,
                "selected_blob_arrivals_before_retry": 1,
                "selected_blob_arrivals_after_retry": blob_wants(&log, &fixture.selected),
                "retry_fetch_commands": suffix.matches("command=fetch").count(),
                "retry_head_commands": suffix.matches("command=ls-refs").count(),
            }))
            .unwrap(),
        )
        .unwrap();
    }
    drop(server);
    assert!(
        CachedRepository::open_in(&url, &root).is_err(),
        "offline cache must not become success"
    );
}

#[test]
fn changed_head_reuses_unchanged_blob_oids() {
    let fixture = Repository::new(true);
    let (_server, url, log) = http_server(&fixture, false);
    let root = fixture.root.path().join("cache");
    let mut first = CachedRepository::open_in(&url, &root).unwrap();
    first.metadata().unwrap();
    first.blob(&fixture.selected).unwrap();
    fs::write(fixture.repo.join("new.md"), "# New\n").unwrap();
    git(&fixture.repo, &["add", "new.md"]);
    git(&fixture.repo, &["commit", "--quiet", "-m", "changed head"]);
    let changed = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let mut second = CachedRepository::open_in(&url, &root).unwrap();
    assert_ne!(second.head_oid(), fixture.head);
    assert_eq!(second.head_oid(), changed);
    second.metadata().unwrap();
    second.blob(&fixture.selected).unwrap();
    assert_eq!(blob_wants(&log, &fixture.selected), 1);
}

#[test]
fn corruption_is_diagnosed_and_refetched_without_discarding_valid_objects() {
    let fixture = Repository::new(true);
    let (_server, url, log) = http_server(&fixture, false);
    let root = fixture.root.path().join("cache");
    let mut remote = CachedRepository::open_in(&url, &root).unwrap();
    remote.metadata().unwrap();
    remote.blob(&fixture.selected).unwrap();
    let object = remote
        .directory()
        .join("objects")
        .join(format!("{}.obj", fixture.selected));
    fs::write(&object, b"blob\ncorrupted content").unwrap();
    assert_eq!(
        remote.blob(&fixture.selected).unwrap().data,
        b"# Selected\n"
    );
    assert_eq!(blob_wants(&log, &fixture.selected), 2);
    assert_eq!(fs::read(&object).unwrap(), b"blob\n# Selected\n");
    assert!(
        remote
            .blob("1111111111111111111111111111111111111111")
            .is_err()
    );
    assert_eq!(
        remote.blob(&fixture.selected).unwrap().data,
        b"# Selected\n"
    );
    assert_eq!(blob_wants(&log, &fixture.selected), 2);
}

#[test]
fn incomplete_temporary_files_are_not_objects_or_manifests() {
    let fixture = Repository::new(true);
    let root = fixture.root.path().join("cache");
    let mut remote = CachedRepository::open_in(fixture.repo.to_str().unwrap(), &root).unwrap();
    fs::create_dir_all(remote.directory().join("objects")).unwrap();
    let mut interrupted =
        tempfile::NamedTempFile::new_in(remote.directory().join("objects")).unwrap();
    interrupted.write_all(b"blob\npartial").unwrap();
    let _interrupted = interrupted.keep().unwrap();
    assert_eq!(
        remote.blob(&fixture.selected).unwrap().data,
        b"# Selected\n"
    );
    let metadata = remote.metadata().unwrap();
    assert!(metadata.iter().any(|object| object.oid == fixture.head));
}

#[test]
fn concurrent_writers_publish_one_verified_blob() {
    let fixture = Repository::new(true);
    let (_server, url, log) = http_server(&fixture, false);
    let root = fixture.root.path().join("cache");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let threads: Vec<_> = (0..3)
        .map(|_| {
            let (url, root, oid, barrier) = (
                url.clone(),
                root.clone(),
                fixture.selected.clone(),
                barrier.clone(),
            );
            std::thread::spawn(move || {
                let mut remote = CachedRepository::open_in(&url, &root).unwrap();
                barrier.wait();
                remote.metadata().unwrap();
                assert_eq!(remote.blob(&oid).unwrap().data, b"# Selected\n");
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(blob_wants(&log, &fixture.selected), 1);
}

#[test]
fn revoked_access_does_not_use_previously_cached_content() {
    let fixture = Repository::new(true);
    let (_server, url, _log) = http_server_mode(&fixture, "mutable-access");
    let root = fixture.root.path().join("cache");
    let mut remote = CachedRepository::open_in(&url, &root).unwrap();
    remote.blob(&fixture.selected).unwrap();
    let object = remote
        .directory()
        .join("objects")
        .join(format!("{}.obj", fixture.selected));
    let before = fs::read(&object).unwrap();
    fs::write(fixture.root.path().join("deny-access"), []).unwrap();
    assert!(CachedRepository::open_in(&url, &root).is_err());
    assert_eq!(fs::read(&object).unwrap(), before);
}

#[test]
fn pinned_revisions_do_not_lookup_child_head_and_are_checked_live() {
    let fixture = Repository::new(true);
    let (_server, url, log) = http_server(&fixture, false);
    let root = fixture.root.path().join("cache");
    let mut remote = CachedRepository::pinned_in(&url, &fixture.head, &root).unwrap();
    remote.blob(&fixture.selected).unwrap();
    let requests = fs::read_to_string(&log).unwrap();
    assert!(!requests.contains("command=ls-refs"));
    assert!(requests.contains(&format!("want {}", fixture.head)));
    assert!(
        CachedRepository::pinned_in(&url, "1111111111111111111111111111111111111111", &root)
            .is_err()
    );
}

#[test]
fn repository_lock_is_bounded_and_released_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repository.lock");
    let lock = RepositoryLock::acquire(&path, Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    let error = RepositoryLock::acquire(&path, Duration::from_millis(50))
        .err()
        .unwrap();
    assert!(error.contains("contention"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(lock);
    RepositoryLock::acquire(&path, Duration::from_secs(1)).unwrap();
}
