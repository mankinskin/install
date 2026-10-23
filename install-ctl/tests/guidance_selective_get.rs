#[path = "support/selective_guidance_behaviors.rs"]
mod behaviors;
#[path = "support/selective_guidance.rs"]
mod support;

use std::{fs, path::Path};
use support::{Fixture, commit, git, write};

#[test]
fn invalid_selection_then_corrected_and_unchanged_retries_reuse_verified_objects() {
    let mut fixture = Fixture::new(true, false);
    let selected = git(&fixture.repo, &["rev-parse", "HEAD:selected.md"]);
    let dependency = git(&fixture.repo, &["rev-parse", "HEAD:deps/base.md"]);
    let unrelated = git(&fixture.repo, &["rev-parse", "HEAD:unrelated.bin"]);
    let bad = fixture.invoke(&[
        "--select",
        "selected.md",
        "--select",
        "missing/instruction.md",
    ]);
    assert!(!bad.status.success());
    assert!(bad.stderr.contains("missing/instruction.md"));
    assert!(
        bad.stderr
            .contains(&git(&fixture.repo, &["rev-parse", "HEAD"]))
    );
    assert_eq!(fixture.wants(&selected), 0);
    assert!(!fixture.root.path().join("target/.agents").exists());
    assert_eq!(
        fs::read_dir(fixture.root.path().join("temporary"))
            .unwrap()
            .count(),
        0
    );

    let good = fixture.invoke(&["--select", "selected.md", "--keep-checkout", "--json"]);
    good.assert_success();
    let plan: serde_json::Value = serde_json::from_str(&good.stdout)
        .expect("JSON stdout is exactly the existing plan object");
    assert_eq!(plan["selected_roots"], serde_json::json!(["selected.md"]));
    let snapshot = good.kept_path();
    assert!(snapshot.starts_with(fixture.root.path().join("temporary")));
    assert!(snapshot.join("selected.md").is_file());
    assert!(snapshot.join("deps/base.md").is_file());
    assert!(!snapshot.join(".git").exists());
    assert!(!snapshot.join("unrelated.bin").exists());
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("target/.agents/deps/base.md")).unwrap(),
        "# Base\n"
    );
    assert_eq!(fixture.wants(&selected), 1);
    assert_eq!(fixture.wants(&dependency), 1);
    assert_eq!(fixture.wants(&unrelated), 0);
    let before = fixture.requests().len();
    let retry = fixture.invoke(&["--select", "selected.md"]);
    retry.assert_success();
    assert!(retry.stdout.contains("2 unchanged"));
    let later = fixture.requests();
    let later = &later[before..];
    assert_eq!(
        later
            .iter()
            .filter(|request| request["body"]
                .as_str()
                .is_some_and(|body| body.contains("command=ls-refs")))
            .count(),
        1
    );
    assert!(!later.iter().any(|request| {
        request["body"]
            .as_str()
            .is_some_and(|body| body.contains("command=fetch"))
    }));

    let cache = fixture.cache(&fixture.url.clone()).join("SHA1");
    let stored = fs::read(cache.join("objects").join(format!("{selected}.obj"))).unwrap();
    assert_eq!(stored, b"blob\n[dep](deps/base.md)\n");
    assert!(
        !cache
            .join("objects")
            .join(format!("{unrelated}.obj"))
            .exists()
    );
    if let Some(path) = std::env::var_os("GUIDANCE_CLI_EVIDENCE_ROOT") {
        let path = Path::new(&path);
        fs::create_dir_all(path).unwrap();
        fs::copy(&fixture.log, path.join("requests.jsonl")).unwrap();
        fs::copy(snapshot.join("selected.md"), path.join("selected.md")).unwrap();
        fs::write(path.join("selected.obj"), &stored).unwrap();
        fs::write(path.join("plan.json"), &good.stdout).unwrap();
        fs::write(path.join("inventory.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "selected_oid": selected, "dependency_oid": dependency, "unrelated_oid": unrelated,
            "selected_wants": fixture.wants(&selected), "dependency_wants": fixture.wants(&dependency),
            "unrelated_wants": fixture.wants(&unrelated), "retry_requests": later,
            "destination_content": fs::read_to_string(fixture.root.path().join("target/.agents/selected.md")).unwrap(),
        })).unwrap()).unwrap();
    }
    fs::remove_dir_all(snapshot).unwrap();
    assert_eq!(
        fs::read_dir(fixture.root.path().join("temporary"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn warm_cache_never_hides_revoked_access_or_remote_failure() {
    let fixture = Fixture::new(true, true);
    fixture
        .invoke(&["--select", "selected.md"])
        .assert_success();
    write(fixture.root.path(), "deny-access", "");
    let denied = fixture.invoke(&["--select", "selected.md", "--keep-checkout"]);
    assert!(!denied.status.success());
    let snapshot = denied.kept_path();
    assert_eq!(fs::read_dir(&snapshot).unwrap().count(), 0);
    fs::remove_dir_all(snapshot).unwrap();
}

#[test]
fn unsupported_filter_fails_without_clone_or_unfiltered_fetch() {
    let fixture = Fixture::new(false, false);
    let result = fixture.invoke(&["--select", "selected.md"]);
    assert!(!result.status.success());
    assert!(result.stderr.contains("filter"), "{}", result.stderr);
    assert!(!result.stderr.contains("git clone"));
    assert!(!fixture.requests().iter().any(|request| {
        request["body"]
            .as_str()
            .is_some_and(|body| body.contains("command=fetch"))
    }));
    assert!(!fixture.root.path().join("target/.agents").exists());
}

#[test]
fn changed_head_reuses_unchanged_dependencies_but_installs_new_selected_bytes() {
    let fixture = Fixture::new(true, false);
    fixture
        .invoke(&["--select", "selected.md"])
        .assert_success();
    let dependency = git(&fixture.repo, &["rev-parse", "HEAD:deps/base.md"]);
    write(
        &fixture.repo,
        "selected.md",
        "# Changed\n[dep](deps/base.md)\n",
    );
    commit(&fixture.repo);
    fixture
        .invoke(&["--select", "selected.md"])
        .assert_success();
    assert_eq!(fixture.wants(&dependency), 1);
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("target/.agents/selected.md")).unwrap(),
        "# Changed\n[dep](deps/base.md)\n"
    );
}
