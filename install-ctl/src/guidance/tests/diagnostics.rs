use super::*;

// -- recipe corpus (AC2, AC3) -------------------------------------------------

#[test]
fn recipe_copy_step_produces_an_artifact() {
    let src = TempDir::new().unwrap();
    write(src.path(), "raw/notes.md", "generated notes");
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"recipe-example\"\n\n[[corpus]]\nid = \"gen\"\n\n[corpus.recipe]\nstep = [{ kind = \"copy\", from = \"raw/notes.md\", to = \"notes.md\" }]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["gen".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(!plan.is_blocking(), "diagnostics: {:?}", plan.diagnostics);
    assert_eq!(plan.artifacts.len(), 1);
    assert_eq!(plan.artifacts[0].id, "notes.md");
}

#[test]
fn unsupported_recipe_step_is_a_blocking_diagnostic() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"bad-recipe\"\n\n[[corpus]]\nid = \"gen\"\n\n[corpus.recipe]\nstep = [{ kind = \"download\", from = \"http://example.com\", to = \"notes.md\" }]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["gen".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::UnsupportedRecipeStep))
    );
}

// -- rejection cases (AC4, AC10) ----------------------------------------------

#[test]
fn missing_dependency_is_a_reported_diagnostic() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [gone](missing.md)");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(!plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::MissingDependency))
    );
}

#[test]
fn dependency_cycle_is_a_reported_diagnostic() {
    let src = TempDir::new().unwrap();
    write(src.path(), "a.md", "see [b](b.md)");
    write(src.path(), "b.md", "see [a](a.md)");
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"cycle\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"a.md\"]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(!plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::Cycle))
    );
}

#[test]
fn traversal_link_is_a_blocking_diagnostic() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [out](../../outside.md)");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::PathTraversal))
    );
}

#[test]
fn absolute_source_path_is_rejected_at_load_time() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"bad\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"/etc/passwd\"]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
    );

    let err = crate::guidance::profile::load_profile(&src.path().join("guidance.toml"))
        .expect_err("absolute source path must be rejected");
    assert!(err.contains("absolute"));
}

#[test]
fn malformed_profile_toml_is_rejected() {
    let src = TempDir::new().unwrap();
    write(src.path(), "guidance.toml", "this is not { valid toml");
    let err = crate::guidance::profile::load_profile(&src.path().join("guidance.toml"))
        .expect_err("malformed TOML must be rejected");
    assert!(err.contains("guidance.toml"));
}

#[test]
fn recipe_copy_id_collision_from_different_sources_is_ambiguous_ownership() {
    let src = TempDir::new().unwrap();
    write(src.path(), "raw/a.md", "from a");
    write(src.path(), "raw/b.md", "from b");
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"ambiguous\"\n\n[[corpus]]\nid = \"gen-a\"\n\n[corpus.recipe]\nstep = [{ kind = \"copy\", from = \"raw/a.md\", to = \"notes.md\" }]\n\n[[corpus]]\nid = \"gen-b\"\n\n[corpus.recipe]\nstep = [{ kind = \"copy\", from = \"raw/b.md\", to = \"notes.md\" }]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["gen-a".to_string(), "gen-b".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::AmbiguousOwnership))
    );
}

// -- duplicate destination (AC2, AC4) -----------------------------------------

#[test]
fn distinct_sources_falling_back_to_the_same_canonical_path_is_duplicate_destination() {
    // Neither "sub" nor "other" is an initialized submodule at the target,
    // so both `sub/.agents/prompt.md` and the top-level `prompt.md` fall
    // back to the same canonical `.agents/prompt.md` destination.
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "top level");
    write(src.path(), "sub/.agents/prompt.md", "nested fallback");
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"dup\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"prompt.md\", \"sub/.agents/prompt.md\"]\n\n[destination]\nscope = \"repo\"\n",
    );

    // Target has no submodules initialized at all.
    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: None,
        explicit_override: None,
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::DuplicateDestination))
    );
}

// -- symlink escape (AC2, AC4) -------------------------------------------------

#[test]
#[cfg(unix)]
fn symlink_escaping_the_source_root_is_a_blocking_diagnostic() {
    use std::os::unix::fs::symlink;

    let outside = TempDir::new().unwrap();
    write(outside.path(), "secret.md", "outside content");

    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [escaped](escaped.md)");
    symlink(
        outside.path().join("secret.md"),
        src.path().join("escaped.md"),
    )
    .unwrap();
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, crate::guidance::graph::DiagnosticCode::SymlinkEscape))
    );
    assert!(!explicit.exists());
}
