use super::support::{Fixture, commit, git, init_repo, stage_link, write};
use std::{fs, process::Command};

#[test]
fn source_credentials_are_absent_from_public_progress_and_errors() {
    let fixture = Fixture::new(false, false);
    let url = fixture.url.replace(
        "http://",
        "http://private-user-marker:private-password-marker@",
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_install-ctl"));
    command
        .args([
            "guidance",
            "get",
            &url,
            "--select",
            "selected.md",
            "--target",
        ])
        .arg(fixture.root.path().join("target"));
    let result = fixture.execute(command);
    assert!(!result.status.success());
    for output in [&result.stdout, &result.stderr] {
        assert!(!output.contains("private-user-marker"));
        assert!(!output.contains("private-password-marker"));
    }
}

#[test]
fn explicit_profile_unknown_id_and_recipe_preserve_input_and_output_identity() {
    let fixture = Fixture::new(true, false);
    write(
        &fixture.repo,
        "profile.toml",
        r#"
[profile]
id = "recipe"
[[corpus]]
id = "recipe"
[corpus.recipe]
[[corpus.recipe.step]]
kind = "copy"
from = "template.txt"
to = "instructions/root.md"
[destination]
scope = "repo"
"#,
    );
    write(&fixture.repo, "template.txt", "[dep](../deps/base.md)\n");
    commit(&fixture.repo);
    let input = git(&fixture.repo, &["rev-parse", "HEAD:template.txt"]);
    let bad = fixture.invoke(&["--profile", "profile.toml", "--select", "unknown"]);
    assert!(!bad.status.success());
    assert_eq!(fixture.wants(&input), 0);
    let good = fixture.invoke(&[
        "--profile",
        "profile.toml",
        "--select",
        "recipe",
        "--keep-checkout",
    ]);
    good.assert_success();
    let snapshot = good.kept_path();
    assert!(snapshot.join("template.txt").exists());
    assert!(!snapshot.join("instructions/root.md").exists());
    assert_eq!(
        fs::read_to_string(
            fixture
                .root
                .path()
                .join("target/.agents/instructions/root.md")
        )
        .unwrap(),
        "[dep](../deps/base.md)\n"
    );
    fs::remove_dir_all(snapshot).unwrap();
}

#[test]
fn directory_multiple_selectors_and_explicit_destination_remain_idempotent() {
    let fixture = Fixture::new(true, false);
    write(&fixture.repo, "docs/other.md", "# Other\n");
    write(&fixture.repo, "docs/ignored.txt", "ignored");
    commit(&fixture.repo);
    let destination = fixture.root.path().join("explicit");
    let args = [
        "--select",
        "docs",
        "--select",
        "selected.md",
        "--destination-scope",
        "explicit",
        "--destination-path",
        destination.to_str().unwrap(),
    ];
    fixture.invoke(&args).assert_success();
    assert!(destination.join("docs/other.md").exists());
    assert!(destination.join("deps/base.md").exists());
    assert!(!destination.join("docs/ignored.txt").exists());
    let repeated = fixture.invoke(&args);
    repeated.assert_success();
    assert!(repeated.stdout.contains("3 unchanged"));
    assert!(!fixture.root.path().join("target/.agents").exists());
}

#[test]
fn contained_and_escaping_git_symlinks_need_no_windows_symlink_privilege() {
    let fixture = Fixture::new(true, false);
    stage_link(&fixture.repo, "alias.md", "selected.md");
    stage_link(&fixture.repo, "escape.md", "../outside.md");
    git(&fixture.repo, &["commit", "--quiet", "-m", "symlinks"]);
    fixture.invoke(&["--select", "alias.md"]).assert_success();
    assert!(
        !fs::symlink_metadata(fixture.root.path().join("target/.agents/alias.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let bad = fixture.invoke(&["--select", "escape.md"]);
    assert!(!bad.status.success());
    assert!(bad.stderr.contains("escape"), "{}", bad.stderr);
    assert!(
        !fixture
            .root
            .path()
            .join("target/.agents/escape.md")
            .exists()
    );
}

#[test]
fn nested_relative_submodules_use_the_gitlink_revision_and_ignore_siblings() {
    let mut fixture = Fixture::new(true, false);
    let child = fixture.root.path().join("child.git");
    let grandchild = fixture.root.path().join("grand.git");
    init_repo(&child, true);
    init_repo(&grandchild, true);
    let pin = git(&grandchild, &["rev-parse", "HEAD"]);
    write(
        &child,
        ".gitmodules",
        "[submodule \"nested\"]\npath = nested\nurl = ../grand.git\n",
    );
    git(&child, &["add", ".gitmodules"]);
    git(
        &child,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{pin},nested"),
        ],
    );
    git(&child, &["commit", "--quiet", "-m", "pin grandchild"]);
    let child_pin = git(&child, &["rev-parse", "HEAD"]);
    write(&grandchild, "selected.md", "# Newer, not pinned\n");
    commit(&grandchild);
    write(
        &fixture.repo,
        ".gitmodules",
        "[submodule \"needed\"]\npath = needed\nurl = ../child.git\n[submodule \"sibling\"]\npath = sibling\nurl = ../nonexistent.git\n",
    );
    write(
        &fixture.repo,
        "selected.md",
        "[child](needed/nested/selected.md)\n",
    );
    git(&fixture.repo, &["add", ".gitmodules", "selected.md"]);
    for path in ["needed", "sibling"] {
        git(
            &fixture.repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{child_pin},{path}"),
            ],
        );
    }
    git(&fixture.repo, &["commit", "--quiet", "-m", "pin child"]);
    fixture.cache(&fixture.url.replace("/repo.git", "/child.git"));
    fixture.cache(&fixture.url.replace("/repo.git", "/grand.git"));
    let missing = fixture.invoke(&["--select", "not-here/instruction.md"]);
    assert!(!missing.status.success());
    assert!(
        fixture
            .requests()
            .iter()
            .all(|request| request["path"].as_str().unwrap().starts_with("/repo.git/"))
    );
    fixture
        .invoke(&["--select", "selected.md"])
        .assert_success();
    assert_eq!(
        fs::read_to_string(
            fixture
                .root
                .path()
                .join("target/.agents/needed/nested/selected.md")
        )
        .unwrap(),
        "[dep](deps/base.md)\n"
    );
    let requests = fixture.requests();
    let children: Vec<_> = requests
        .iter()
        .filter(|request| !request["path"].as_str().unwrap().starts_with("/repo.git/"))
        .collect();
    assert!(!children.is_empty());
    assert!(children.iter().all(|request| {
        !request["body"]
            .as_str()
            .unwrap()
            .contains("command=ls-refs")
    }));
    assert!(
        requests
            .iter()
            .all(|request| !request["path"].as_str().unwrap().contains("nonexistent"))
    );
}

#[test]
fn local_plan_and_install_do_not_need_a_network_or_source_repository() {
    let fixture = Fixture::new(true, false);
    let result = fixture.invoke(&["--select", "selected.md", "--keep-checkout"]);
    result.assert_success();
    let snapshot = result.kept_path();
    write(
        &snapshot,
        "profile.toml",
        "[profile]\nid='local'\n[[corpus]]\nid='root'\npaths=['selected.md']\n[destination]\nscope='repo'\n",
    );
    let requests = fixture.requests().len();
    for operation in ["plan", "install"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_install-ctl"));
        command
            .args(["guidance", operation, "--source"])
            .arg(&snapshot)
            .arg("--profile")
            .arg(snapshot.join("profile.toml"))
            .args(["--select", "root", "--target"])
            .arg(fixture.root.path().join("local-target"))
            .env("http_proxy", "http://127.0.0.1:1")
            .env("https_proxy", "http://127.0.0.1:1")
            .env("GIT_SSH_COMMAND", "not-a-real-ssh-client");
        fixture.execute(command).assert_success();
    }
    assert_eq!(fixture.requests().len(), requests);
    assert!(
        fixture
            .root
            .path()
            .join("local-target/.agents/selected.md")
            .exists()
    );
    fs::remove_dir_all(snapshot).unwrap();
}
