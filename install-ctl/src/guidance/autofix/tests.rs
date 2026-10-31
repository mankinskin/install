//! Autofix plan/apply tests.

use std::collections::BTreeMap;

use tempfile::tempdir;

use super::{apply_plan, build_plan};

fn rewrites(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(old, new)| (old.to_string(), new.to_string()))
        .collect()
}

#[test]
fn plan_is_read_only_and_reports_matching_operation() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
    std::fs::write(
        repo.path().join(".agents/links.md"),
        "[moved](old-name.md)\n",
    )
    .unwrap();

    let before = std::fs::read(repo.path().join(".agents/links.md")).unwrap();
    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "README.md")]),
    )
    .expect("plan");
    let after = std::fs::read(repo.path().join(".agents/links.md")).unwrap();

    assert_eq!(before, after, "plan must not write to disk");
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].new_destination, "README.md");
    assert!(plan.unresolved.is_empty());
}

#[test]
fn plan_leaves_findings_without_a_rewrite_unresolved() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/links.md"), "[gone](missing.md)\n").unwrap();

    let plan = build_plan(repo.path(), None, &BTreeMap::new()).expect("plan");

    assert!(plan.operations.is_empty());
    assert_eq!(plan.unresolved.len(), 1);
    assert!(
        plan.unresolved[0]
            .reason
            .contains("no registered safe transformation")
    );
}

#[test]
fn apply_requires_explicit_confirmation() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
    std::fs::write(
        repo.path().join(".agents/links.md"),
        "[moved](old-name.md)\n",
    )
    .unwrap();

    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "README.md")]),
    )
    .expect("plan");

    let error = apply_plan(repo.path(), &plan, false).expect_err("must refuse without --yes");
    assert!(error.contains("--yes"));
}

#[test]
fn apply_writes_atomically_and_reruns_audit() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
    std::fs::write(
        repo.path().join(".agents/links.md"),
        "[moved](old-name.md)\n",
    )
    .unwrap();

    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "README.md")]),
    )
    .expect("plan");
    let result = apply_plan(repo.path(), &plan, true).expect("apply");

    assert_eq!(result.applied.len(), 1);
    assert!(!result.rolled_back);
    assert_eq!(result.post_audit_blocking_findings, 0);
    let rewritten = std::fs::read_to_string(repo.path().join(".agents/links.md")).unwrap();
    assert!(rewritten.contains("[moved](README.md)"));
}

#[test]
fn apply_refuses_stale_hash_with_zero_writes() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
    std::fs::write(
        repo.path().join(".agents/links.md"),
        "[moved](old-name.md)\n",
    )
    .unwrap();

    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "README.md")]),
    )
    .expect("plan");

    // Source changes after the plan was generated.
    std::fs::write(
        repo.path().join(".agents/links.md"),
        "[moved](old-name.md)\nextra content\n",
    )
    .unwrap();
    let before_apply = std::fs::read(repo.path().join(".agents/links.md")).unwrap();

    let result = apply_plan(repo.path(), &plan, true).expect("apply call succeeds");

    assert!(result.applied.is_empty());
    assert_eq!(result.refused.len(), 1);
    assert!(result.refused[0].reason.contains("stale plan"));
    let after_apply = std::fs::read(repo.path().join(".agents/links.md")).unwrap();
    assert_eq!(
        before_apply, after_apply,
        "stale-hash refusal must not write"
    );
}

#[test]
fn apply_rolls_back_when_post_audit_still_blocks() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    // Deliberately rewrite to another missing target so the post-audit
    // still blocks and the apply must roll back to the original file.
    std::fs::write(
        repo.path().join(".agents/links.md"),
        "[moved](old-name.md)\n",
    )
    .unwrap();

    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "still-missing.md")]),
    )
    .expect("plan");
    let original = std::fs::read(repo.path().join(".agents/links.md")).unwrap();

    let result = apply_plan(repo.path(), &plan, true).expect("apply");

    assert!(result.rolled_back);
    assert_eq!(result.post_audit_blocking_findings, 1);
    let after = std::fs::read(repo.path().join(".agents/links.md")).unwrap();
    assert_eq!(original, after, "rollback must restore original content");
}

#[test]
fn apply_leaves_code_fences_and_spans_byte_identical() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
    let original = "[moved](old-name.md)\n\n```\n[moved](old-name.md)\n```\n\nInline `[moved](old-name.md)`.\n";
    std::fs::write(repo.path().join(".agents/links.md"), original).unwrap();

    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "README.md")]),
    )
    .expect("plan");
    let result = apply_plan(repo.path(), &plan, true).expect("apply");

    assert_eq!(result.applied.len(), 1);
    let rewritten = std::fs::read_to_string(repo.path().join(".agents/links.md")).unwrap();
    assert_eq!(rewritten, original.replacen("old-name.md", "README.md", 1));
}

#[test]
fn apply_refuses_an_unlocatable_destination() {
    let repo = tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
    std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
    let original = "[moved](old\\-name.md)\n";
    std::fs::write(repo.path().join(".agents/links.md"), original).unwrap();

    let plan = build_plan(
        repo.path(),
        None,
        &rewrites(&[("old-name.md", "README.md")]),
    )
    .expect("plan");
    let result = apply_plan(repo.path(), &plan, true).expect("apply");

    assert!(result.applied.is_empty(), "{result:?}");
    assert_eq!(result.refused.len(), 1, "{result:?}");
    assert!(
        result.refused[0]
            .reason
            .contains("was not found verbatim in the source; refusing an ambiguous rewrite"),
        "{result:?}"
    );
    let after = std::fs::read_to_string(repo.path().join(".agents/links.md")).unwrap();
    assert_eq!(after, original);
}
