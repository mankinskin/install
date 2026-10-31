use super::*;

// -- mismatched nested directory (present, not a submodule) (AC2, AC3) -------

#[test]
fn present_but_uninitialized_directory_is_not_treated_as_an_owning_submodule() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "workflow-tools/.agents/instructions/foo.md",
        "content",
    );
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"mismatch\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"workflow-tools/.agents/instructions/foo.md\"]\n\n[destination]\nscope = \"repo\"\n",
    );

    // "workflow-tools" exists at the target but has no `.git` gitlink, so it
    // is a plain directory, not an initialized submodule.
    let target = TempDir::new().unwrap();
    write(
        target.path(),
        "workflow-tools/existing-real-file.md",
        "unrelated pre-existing content",
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
    // Falls back to the canonical root, never into the mismatched directory.
    assert_eq!(foo.final_path, ".agents/instructions/foo.md");

    let result = install_plan(&plan, src.path()).expect("install succeeds");
    assert_eq!(result.written.len(), 1);
    assert_eq!(
        fs::read_to_string(target.path().join("workflow-tools/existing-real-file.md")).unwrap(),
        "unrelated pre-existing content",
        "install must never touch the mismatched directory's existing contents"
    );
    assert!(
        !target
            .path()
            .join("workflow-tools/.agents/instructions/foo.md")
            .exists(),
        "install must not write into the mismatched directory"
    );
}

// -- full AGENTS surface closure (AC2, AC3) -----------------------------------

#[test]
fn full_agents_surface_closure_installs_every_referenced_file() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "AGENTS.md",
        "See [instructions](.agents/instructions/base.instructions.md) and [agent](.agents/agents/implement.agent.md) and [skill](.agents/skills/topic/SKILL.md).",
    );
    write(
        src.path(),
        ".agents/instructions/base.instructions.md",
        "base instructions",
    );
    write(
        src.path(),
        ".agents/agents/implement.agent.md",
        "see [also](../instructions/base.instructions.md)",
    );
    write(src.path(), ".agents/skills/topic/SKILL.md", "skill content");
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"full-agents\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"AGENTS.md\"]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
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
    assert!(!plan.is_blocking(), "diagnostics: {:?}", plan.diagnostics);

    let mut ids: Vec<&str> = plan.artifacts.iter().map(|a| a.id.as_str()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            ".agents/agents/implement.agent.md",
            ".agents/instructions/base.instructions.md",
            ".agents/skills/topic/SKILL.md",
            "AGENTS.md",
        ]
    );

    let result = install_plan(&plan, src.path()).expect("install succeeds");
    assert_eq!(result.written.len(), 4);
}

#[test]
fn duplicate_corpus_id_is_rejected_at_load_time() {
    let src = TempDir::new().unwrap();
    write(
        src.path(),
        "guidance.toml",
        "[profile]\nid = \"dup\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"a.md\"]\n\n[[corpus]]\nid = \"root\"\npaths = [\"b.md\"]\n\n[destination]\nscope = \"repo\"\n",
    );
    let err = crate::guidance::profile::load_profile(&src.path().join("guidance.toml"))
        .expect_err("duplicate corpus id must be rejected");
    assert!(err.contains("duplicate"));
}
