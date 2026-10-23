use super::acquire;
use crate::guidance::remote::transport::tests::{Repository, git, http_server};
use std::{fs, path::Path};

pub(super) fn commit(repository: &mut Repository) {
    git(&repository.repo, &["add", "-A"]);
    git(
        &repository.repo,
        &["commit", "--quiet", "-m", "acquisition fixture"],
    );
    repository.head = git(&repository.repo, &["rev-parse", "HEAD"]);
}

pub(super) fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn wants(log: &Path, oid: &str) -> usize {
    fs::read_to_string(log)
        .unwrap()
        .matches(&format!("want {oid}"))
        .count()
}

#[test]
fn closure_fetches_only_selected_references_and_terminates_cycles() {
    let mut fixture = Repository::new(true);
    write(
        &fixture.repo,
        "selected.md",
        "[dep](deps/base.md) [web](https://example.invalid) [image](unused.bin)",
    );
    write(&fixture.repo, "deps/base.md", "[root](../selected.md)");
    commit(&mut fixture);
    let selected = git(&fixture.repo, &["rev-parse", "HEAD:selected.md"]);
    let dependency = git(&fixture.repo, &["rev-parse", "HEAD:deps/base.md"]);
    let (_server, url, log) = http_server(&fixture, false);
    let snapshot = fixture.root.path().join("snapshot");
    let profile = acquire(
        &url,
        &["selected.md".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap();
    assert_eq!(profile.id, "get");
    assert!(
        fs::read_to_string(snapshot.join("deps/base.md"))
            .unwrap()
            .contains("../selected.md")
    );
    assert!(!snapshot.join("unrelated.bin").exists());
    assert!(!snapshot.join(".gitmodules").exists());
    assert_eq!(wants(&log, &selected), 1);
    assert_eq!(wants(&log, &dependency), 1);
    assert_eq!(wants(&log, &fixture.unrelated), 0);
    super::contract_tests::verify_closure_inventory(
        &fixture,
        &snapshot,
        &log,
        &selected,
        &dependency,
    );
}

#[test]
fn invalid_second_selection_fails_before_any_guidance_blob() {
    let fixture = Repository::new(true);
    let (_server, url, log) = http_server(&fixture, false);
    let snapshot = fixture.root.path().join("snapshot");
    let error = acquire(
        &url,
        &["selected.md".into(), "missing/absent.md".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap_err();
    assert!(error.contains("missing/absent.md"), "{error}");
    assert_eq!(wants(&log, &fixture.selected), 0);
    assert!(!snapshot.join("selected.md").exists());
}

#[test]
fn required_missing_reference_is_not_a_successful_partial_snapshot() {
    let mut fixture = Repository::new(true);
    write(&fixture.repo, "selected.md", "[required](absent.md)");
    commit(&mut fixture);
    let snapshot = fixture.root.path().join("snapshot");
    let error = acquire(
        fixture.repo.to_str().unwrap(),
        &["selected.md".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap_err();
    assert!(error.contains("absent.md"), "{error}");
}

#[test]
fn directory_and_bare_filename_selections_expand_deterministically() {
    let mut fixture = Repository::new(true);
    write(&fixture.repo, "docs/z.md", "# z");
    write(&fixture.repo, "docs/a.toml", "value = 1");
    write(&fixture.repo, "docs/ignored.txt", "not guidance");
    commit(&mut fixture);
    let snapshot = fixture.root.path().join("snapshot");
    let profile = acquire(
        fixture.repo.to_str().unwrap(),
        &["docs".into(), "z.md".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap();
    let crate::guidance::profile::CorpusItem::Direct { paths, .. } = &profile.corpus[0] else {
        panic!("expected direct item")
    };
    assert_eq!(paths, &["docs/a.toml", "docs/z.md"]);
    assert!(!snapshot.join("docs/ignored.txt").exists());
    assert_eq!(
        fs::read_to_string(snapshot.join("docs/z.md")).unwrap(),
        "# z"
    );
}

#[test]
fn recipe_links_use_output_identity_while_snapshot_retains_input_bytes() {
    let mut fixture = Repository::new(true);
    write(&fixture.repo, "inputs/raw.md", "[dep](dep.md)");
    write(&fixture.repo, ".agents/prompts/dep.md", "# dependency");
    write(
        &fixture.repo,
        "profile.toml",
        r#"
[profile]
id = "recipe"
[[corpus]]
id = "copy"
[[corpus.recipe.step]]
kind = "copy"
from = "inputs/raw.md"
to = ".agents/prompts/out.md"
[destination]
scope = "repo"
"#,
    );
    commit(&mut fixture);
    let snapshot = fixture.root.path().join("snapshot");
    let profile = acquire(
        fixture.repo.to_str().unwrap(),
        &["copy".into()],
        Some(Path::new("profile.toml")),
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap();
    assert_eq!(profile.id, "recipe");
    assert_eq!(
        fs::read_to_string(snapshot.join("inputs/raw.md")).unwrap(),
        "[dep](dep.md)"
    );
    assert!(snapshot.join(".agents/prompts/dep.md").is_file());
    assert!(!snapshot.join(".agents/prompts/out.md").exists());
}

fn symlink(repository: &Repository, name: &str, target: &str) {
    let input = repository.root.path().join("symlink-content");
    fs::write(&input, target).unwrap();
    let oid = git(
        &repository.repo,
        &["hash-object", "-w", input.to_str().unwrap()],
    );
    git(
        &repository.repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{oid},{name}"),
        ],
    );
}

#[test]
fn contained_symlinks_are_materialized_without_os_symlink_privileges() {
    let fixture = Repository::new(true);
    symlink(&fixture, "alias.md", "selected.md");
    git(&fixture.repo, &["commit", "--quiet", "-m", "symlink"]);
    let snapshot = fixture.root.path().join("snapshot");
    acquire(
        fixture.repo.to_str().unwrap(),
        &["alias.md".into()],
        None,
        &snapshot,
        &fixture.root.path().join("cache"),
    )
    .unwrap();
    let alias = snapshot.join("alias.md");
    assert_eq!(fs::read(alias.clone()).unwrap(), b"# Selected\n");
    assert!(
        !fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn escaping_and_cyclic_symlinks_fail_explicitly() {
    let fixture = Repository::new(true);
    symlink(&fixture, "escape.md", "../outside.md");
    symlink(&fixture, "a.md", "b.md");
    symlink(&fixture, "b.md", "a.md");
    git(&fixture.repo, &["commit", "--quiet", "-m", "unsafe links"]);
    for name in ["escape.md", "a.md"] {
        let error = acquire(
            fixture.repo.to_str().unwrap(),
            &[name.into()],
            None,
            &fixture.root.path().join("snapshot"),
            &fixture.root.path().join("cache"),
        )
        .unwrap_err();
        assert!(error.contains("symlink"), "{error}");
    }
}

#[test]
fn nested_submodules_use_pinned_commits_and_never_contact_siblings() {
    let parent = Repository::new(true);
    let mut child = Repository::new(true);
    let mut grandchild = Repository::new(true);
    for (repository, name) in [(&mut child, "child.git"), (&mut grandchild, "grand.git")] {
        let destination = parent.root.path().join(name);
        fs::rename(&repository.repo, &destination).unwrap();
        repository.repo = destination;
    }
    write(
        &child.repo,
        ".gitmodules",
        "[submodule \"nested\"]\n path = nested\n url = ../grand.git\n",
    );
    write(&child.repo, "selected.md", "[grand](nested/selected.md)");
    git(&child.repo, &["add", "-A"]);
    git(
        &child.repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},nested", grandchild.head),
        ],
    );
    git(&child.repo, &["commit", "--quiet", "-m", "nested pin"]);
    let pin = git(&child.repo, &["rev-parse", "HEAD"]);
    let pinned_blob = git(&child.repo, &["rev-parse", "HEAD:selected.md"]);
    write(&child.repo, "selected.md", "# newer mutable HEAD");
    commit(&mut child);
    let newer_blob = git(&child.repo, &["rev-parse", "HEAD:selected.md"]);
    write(
        &parent.repo,
        ".gitmodules",
        "[submodule \"child\"]\n path = child\n url = ../child.git\n[submodule \"sibling\"]\n path = sibling\n url = ../missing-sibling.git\n",
    );
    write(&parent.repo, "selected.md", "[child](child/selected.md)");
    git(&parent.repo, &["add", "-A"]);
    for name in ["child", "sibling"] {
        git(
            &parent.repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{pin},{name}"),
            ],
        );
    }
    git(&parent.repo, &["commit", "--quiet", "-m", "child pins"]);
    let (_server, url, log) = http_server(&parent, false);
    let snapshot = parent.root.path().join("snapshot");
    acquire(
        &url,
        &["selected.md".into()],
        None,
        &snapshot,
        &parent.root.path().join("cache"),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(snapshot.join("child/selected.md")).unwrap(),
        "[grand](nested/selected.md)"
    );
    assert_eq!(
        fs::read(snapshot.join("child/nested/selected.md")).unwrap(),
        b"# Selected\n"
    );
    assert_eq!(wants(&log, &newer_blob), 0);
    assert_eq!(wants(&log, &pinned_blob), 1);
    let requests = fs::read_to_string(log).unwrap();
    assert!(!requests.contains("missing-sibling"));
    let mut child_requests = 0;
    for line in requests.lines() {
        let request: serde_json::Value = serde_json::from_str(line).unwrap();
        if request["path"]
            .as_str()
            .unwrap_or("")
            .contains("/child.git/")
        {
            child_requests += 1;
            assert!(
                !request["body"]
                    .as_str()
                    .unwrap_or("")
                    .contains("command=ls-refs")
            );
        }
    }
    assert!(child_requests > 0);
}
