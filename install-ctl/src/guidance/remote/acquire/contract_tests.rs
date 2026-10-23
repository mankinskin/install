use super::{
    acquire, modules,
    tests::{commit, write},
};
use crate::guidance::remote::transport::tests::{Repository, git, http_server};
use std::{collections::BTreeSet, fs, path::Path};

#[cfg(windows)]
#[test]
fn windows_device_and_aliased_names_are_rejected_before_materialization() {
    for path in [
        "NUL.md",
        "con.txt",
        "COM1.md",
        "LPT9.toml",
        "folder./root.md",
        "folder /root.md",
        "root?.md",
    ] {
        assert!(super::normalize(path).is_err(), "{path}");
    }
    assert_eq!(
        super::normalize("normal/root.md").unwrap(),
        "normal/root.md"
    );
}

#[cfg(windows)]
#[test]
fn case_colliding_git_sources_do_not_overwrite_snapshot_bytes() {
    let mut fixture = Repository::new(true);
    write(&fixture.repo, "input-content", "# Other\n");
    let oid = git(&fixture.repo, &["hash-object", "-w", "input-content"]);
    fs::remove_file(fixture.repo.join("input-content")).unwrap();
    git(
        &fixture.repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{oid},Selected.md"),
        ],
    );
    git(&fixture.repo, &["commit", "--quiet", "-m", "case alias"]);
    fixture.head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let snapshot = fixture.root.path().join("snapshot");
    let error = acquire(
        fixture.repo.to_str().unwrap(),
        &["selected.md".into(), "Selected.md".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap_err();
    assert!(error.contains("aliases"), "{error}");
}

pub(super) fn verify_closure_inventory(
    fixture: &Repository,
    snapshot: &Path,
    log: &Path,
    root: &str,
    dependency: &str,
) {
    let mut actual = BTreeSet::new();
    for repository in fs::read_dir(fixture.root.path().join("cache")).unwrap() {
        for format in fs::read_dir(repository.unwrap().path()).unwrap() {
            let objects = format.unwrap().path().join("objects");
            if !objects.is_dir() {
                continue;
            }
            for object in fs::read_dir(objects).unwrap() {
                actual.insert(
                    object
                        .unwrap()
                        .path()
                        .file_stem()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
            }
        }
    }
    let tree = git(&fixture.repo, &["rev-parse", "HEAD^{tree}"]);
    let dependency_tree = git(&fixture.repo, &["rev-parse", "HEAD:deps"]);
    let expected = BTreeSet::from([
        fixture.head.clone(),
        tree,
        dependency_tree,
        root.to_owned(),
        dependency.to_owned(),
    ]);
    assert_eq!(
        actual, expected,
        "received cache inventory differs from metadata plus the exact selected closure"
    );
    if let Some(evidence) = std::env::var_os("GUIDANCE_ACQUIRE_EVIDENCE_ROOT") {
        let evidence = Path::new(&evidence);
        fs::create_dir_all(evidence).unwrap();
        fs::copy(snapshot.join("selected.md"), evidence.join("selected.md")).unwrap();
        fs::copy(
            snapshot.join("deps/base.md"),
            evidence.join("dependency.md"),
        )
        .unwrap();
        fs::copy(log, evidence.join("requests.jsonl")).unwrap();
        fs::write(evidence.join("inventory.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "received_oids": actual, "expected_oids": expected, "unrelated_oid": fixture.unrelated,
        })).unwrap()).unwrap();
    }
}

#[test]
fn relative_git_urls_preserve_transport_and_parent_repository_directory_semantics() {
    for (parent, relative, expected) in [
        (
            "https://example.invalid/group/parent.git",
            "../child.git",
            "https://example.invalid/group/child.git",
        ),
        (
            "ssh://git@example.invalid/group/parent.git",
            "../child.git",
            "ssh://git@example.invalid/group/child.git",
        ),
        (
            "git@example.invalid:group/parent.git",
            "../child.git",
            "git@example.invalid:group/child.git",
        ),
        (
            "git://example.invalid/group/parent.git",
            "./nested.git",
            "git://example.invalid/group/parent.git/nested.git",
        ),
    ] {
        assert_eq!(modules::resolve_url(parent, relative).unwrap(), expected);
    }
}

#[test]
fn selected_directory_with_only_non_guidance_files_remains_empty() {
    let mut fixture = Repository::new(true);
    write(&fixture.repo, "empty/ignored.txt", "not selected");
    commit(&mut fixture);
    let (_server, url, log) = http_server(&fixture, false);
    let snapshot = fixture.root.path().join("snapshot");
    let profile = acquire(
        &url,
        &["empty".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap();
    let crate::guidance::profile::CorpusItem::Direct { paths, .. } = &profile.corpus[0] else {
        panic!("expected direct item")
    };
    assert!(paths.is_empty());
    assert!(snapshot.join("empty").is_dir());
    assert!(!snapshot.join("empty/ignored.txt").exists());
    let ignored = git(&fixture.repo, &["rev-parse", "HEAD:empty/ignored.txt"]);
    assert!(
        !fs::read_to_string(log)
            .unwrap()
            .contains(&format!("want {ignored}"))
    );
}

#[test]
fn invalid_profile_selection_preflights_without_selected_content() {
    let mut fixture = Repository::new(true);
    write(
        &fixture.repo,
        "profile.toml",
        r#"
[profile]
id = "preflight"
[[corpus]]
id = "valid"
paths = ["selected.md"]
[destination]
scope = "repo"
"#,
    );
    commit(&mut fixture);
    let (_server, url, log) = http_server(&fixture, false);
    let error = acquire(
        &url,
        &["valid".into(), "unknown".into()],
        Some(Path::new("profile.toml")),
        &fixture.root.path().join("snapshot"),
        &fixture.root.path().join("cache"),
    )
    .unwrap_err();
    assert!(error.contains("unknown"));
    assert!(
        !fs::read_to_string(log)
            .unwrap()
            .contains(&format!("want {}", fixture.selected))
    );
}

#[test]
fn fallible_graph_read_errors_are_not_missing_dependency_diagnostics() {
    use crate::guidance::graph::{FallibleContentSource, compute_closure_fallible};
    struct Failure;
    impl FallibleContentSource for Failure {
        type Error = &'static str;
        fn read_text(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
            Err("authentication denied")
        }
        fn exists(&mut self, _: &str) -> Result<bool, Self::Error> {
            Ok(true)
        }
        fn escapes_root(&mut self, _: &str) -> Result<bool, Self::Error> {
            Ok(false)
        }
    }
    assert_eq!(
        compute_closure_fallible(&["root.md".into()], &mut Failure).err(),
        Some("authentication denied")
    );
}
