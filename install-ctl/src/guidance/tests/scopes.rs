use super::*;

// -- destination scopes (AC5) -------------------------------------------------

#[test]
fn user_and_system_scopes_use_injected_roots() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "content");
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"scopes\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"prompt.md\"]\n\n[destination]\nscope = \"user\"\n",
    );

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
    assert!(!plan.is_blocking());
    assert_eq!(plan.destination_root, crate::paths::disp(&paths.user_root));

    let inputs = PlanInputs {
        scope_override: Some(DestinationScopeKind::System),
        ..inputs
    };
    let plan = build_plan(&inputs).expect("plan should build");
    assert!(!plan.is_blocking());
    assert_eq!(
        plan.destination_root,
        crate::paths::disp(&paths.system_root)
    );
}

#[test]
fn explicit_destination_escaping_itself_is_blocked() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "content");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let escaping = std::path::PathBuf::from("../../etc");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&escaping),
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

// -- submodule preference and canonical fallback with rewrites (AC6) ---------

#[test]
fn existing_submodule_is_preferred_and_keeps_intra_submodule_links() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "workflow-tools/.agents/instructions/foo.md",
        "see [base](../reference/base.md)",
    );
    write(
        src.path(),
        "workflow-tools/.agents/reference/base.md",
        "base",
    );
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"submodule-example\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"workflow-tools/.agents/instructions/foo.md\"]\n\n[destination]\nscope = \"repo\"\n",
    );

    let target = TempDir::new().unwrap();
    // Mark workflow-tools as an initialized submodule (gitlink file).
    write(
        target.path(),
        "workflow-tools/.git",
        "gitdir: ../.git/modules/workflow-tools\n",
    );

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
    assert!(!plan.is_blocking(), "diagnostics: {:?}", plan.diagnostics);

    let foo = plan
        .artifacts
        .iter()
        .find(|a| a.id.ends_with("foo.md"))
        .unwrap();
    assert_eq!(foo.final_path, "workflow-tools/.agents/instructions/foo.md");
    // Same relative nesting before and after -> no rewrite needed.
    assert!(foo.rewrites.is_empty());
}

#[test]
fn missing_submodule_falls_back_to_canonical_path() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "workflow-tools/.agents/instructions/foo.md",
        "see [base](../reference/base.md)",
    );
    write(
        src.path(),
        "workflow-tools/.agents/reference/base.md",
        "base",
    );
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"submodule-example\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"workflow-tools/.agents/instructions/foo.md\"]\n\n[destination]\nscope = \"repo\"\n",
    );

    // No workflow-tools submodule initialized at the target.
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
    assert!(!plan.is_blocking(), "diagnostics: {:?}", plan.diagnostics);

    let foo = plan
        .artifacts
        .iter()
        .find(|a| a.id.ends_with("foo.md"))
        .unwrap();
    assert_eq!(foo.final_path, ".agents/instructions/foo.md");
    // Both files move to the same `.agents/` canonical root together via
    // the same owning-dir strip, so the relative shape between them is
    // unchanged -> no rewrite needed. AC6 requires a rewrite for every
    // *changed* reference, not an unconditional rewrite of every reference.
    assert!(foo.rewrites.is_empty());
}

#[test]
fn relocation_across_different_directories_rewrites_links() {
    // `foo.md` lives in a submodule that IS initialized at the target, so it
    // keeps its nested path. It links to `thing.md` in a different owning
    // directory that has NO matching submodule at the target, so `thing.md`
    // falls back to the top-level canonical `.agents/` root. The two files
    // end up at different depths, so the link between them must be rewritten.
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "workflow-tools/.agents/instructions/foo.md",
        "see [thing](../../../other-mod/.agents/instructions/thing.md)",
    );
    write(
        src.path(),
        "other-mod/.agents/instructions/thing.md",
        "thing",
    );
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"reloc\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"workflow-tools/.agents/instructions/foo.md\"]\n\n[destination]\nscope = \"repo\"\n",
    );

    let target = TempDir::new().unwrap();
    write(
        target.path(),
        "workflow-tools/.git",
        "gitdir: ../.git/modules/workflow-tools\n",
    );
    // other-mod has no matching submodule at the target.

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
    assert!(!plan.is_blocking(), "diagnostics: {:?}", plan.diagnostics);

    let foo = plan
        .artifacts
        .iter()
        .find(|a| a.id.ends_with("foo.md"))
        .unwrap();
    assert_eq!(foo.final_path, "workflow-tools/.agents/instructions/foo.md");
    assert_eq!(foo.rewrites.len(), 1);
    assert_eq!(
        foo.rewrites[0].new_link,
        "../../../.agents/instructions/thing.md"
    );

    let thing = plan
        .artifacts
        .iter()
        .find(|a| a.id.ends_with("thing.md"))
        .unwrap();
    assert_eq!(thing.final_path, ".agents/instructions/thing.md");
}
