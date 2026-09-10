//! Integration tests exercising `build_plan`/`install_plan` end-to-end
//! against real temporary directories (never the host user/system dirs).

use std::{collections::HashSet, fs, path::Path, path::PathBuf};

use serde::Deserialize;
use tempfile::TempDir;

use super::destination::DestinationPaths;
use super::install::install_plan;
use super::plan::{PlanInputs, build_plan};
use super::profile::DestinationScopeKind;

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn test_destination_paths(tmp: &TempDir) -> DestinationPaths {
    DestinationPaths {
        user_root: tmp.path().join("user-dest"),
        system_root: tmp.path().join("system-dest"),
    }
}

fn direct_profile(dest_scope: &str, dest_path: Option<&str>) -> String {
    let dest_path_line = dest_path
        .map(|p| format!("path = \"{p}\"\n"))
        .unwrap_or_default();
    format!(
        "[profile]\nid = \"example\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"prompt.md\"]\n\n[destination]\nscope = \"{dest_scope}\"\n{dest_path_line}"
    )
}

#[derive(Debug, Deserialize)]
struct FixtureManifest {
    version: u32,
    fixture: Vec<FixtureEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureEntry {
    id: String,
    category: String,
    test: String,
    expected: String,
    assertions: Vec<String>,
}

#[test]
fn fixture_manifest_is_versioned_and_declares_expected_contract() {
    let manifest: FixtureManifest = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../guidance-fixtures/v1/manifest.toml"
    )))
    .expect("fixture manifest must parse");

    assert_eq!(manifest.version, 1);
    assert!(manifest.fixture.len() >= 15);

    let ids: HashSet<&str> = manifest
        .fixture
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        manifest.fixture.len(),
        "fixture ids must be unique"
    );
    assert!(ids.contains("direct-prompt-closure"));
    assert!(ids.contains("full-agents-surface"));
    assert!(ids.contains("existing-submodule"));
    assert!(ids.contains("absent-submodule-fallback"));
    assert!(ids.contains("non-guidance-audit"));
    assert!(ids.contains("autofix-stale-report"));

    for entry in manifest.fixture {
        assert!(!entry.category.is_empty(), "{} has no category", entry.id);
        assert!(!entry.test.is_empty(), "{} has no test", entry.id);
        assert!(matches!(entry.expected.as_str(), "pass" | "blocked"));
        assert!(
            !entry.assertions.is_empty(),
            "{} has no assertions",
            entry.id
        );
    }
}

// -- direct corpus + closure (AC3, AC10) -------------------------------------

#[test]
fn direct_corpus_includes_markdown_closure() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [base](instructions/base.md)");
    write(src.path(), "instructions/base.md", "base content");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/does-not-matter")),
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
    let ids: Vec<&str> = plan.artifacts.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&"prompt.md"));
    assert!(ids.contains(&"instructions/base.md"));
    assert_eq!(plan.artifacts.len(), 2);
}

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
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::UnsupportedRecipeStep))
    );
}

// -- rejection cases (AC4, AC10) ----------------------------------------------

#[test]
fn missing_dependency_is_a_blocking_diagnostic() {
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

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::MissingDependency))
    );
}

#[test]
fn dependency_cycle_is_a_blocking_diagnostic() {
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

    assert!(plan.is_blocking());
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::Cycle))
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
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::PathTraversal))
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

    let err = super::profile::load_profile(&src.path().join("guidance.toml"))
        .expect_err("absolute source path must be rejected");
    assert!(err.contains("absolute"));
}

#[test]
fn malformed_profile_toml_is_rejected() {
    let src = TempDir::new().unwrap();
    write(src.path(), "guidance.toml", "this is not { valid toml");
    let err = super::profile::load_profile(&src.path().join("guidance.toml"))
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
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::AmbiguousOwnership))
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
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::DuplicateDestination))
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
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::SymlinkEscape))
    );
    assert!(!explicit.exists());
}

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
    let err = super::profile::load_profile(&src.path().join("guidance.toml"))
        .expect_err("duplicate corpus id must be rejected");
    assert!(err.contains("duplicate"));
}

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
            .any(|d| matches!(d.code, super::graph::DiagnosticCode::PathTraversal))
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

// -- plan is read-only (AC7) --------------------------------------------------

#[test]
fn plan_never_writes_to_source_or_destination() {
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
    let _plan = build_plan(&inputs).expect("plan should build");

    assert!(!explicit.exists(), "plan must not create the destination");
    // Source tree is unchanged: still exactly the two files we wrote.
    let mut entries: Vec<String> = walk(src.path());
    entries.sort();
    assert_eq!(entries, vec!["guidance.toml", "prompt.md"]);
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in walkdir(root, root) {
        out.push(entry);
    }
    out
}

fn walkdir(root: &Path, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(root, &path));
        } else {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    out
}

// -- install: refuses on diagnostics, atomic, idempotent (AC8, AC9) ----------

#[test]
fn install_refuses_to_write_a_blocking_plan() {
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
    assert!(plan.is_blocking());

    let result = install_plan(&plan, src.path());
    assert!(result.is_err());
    assert!(!explicit.exists(), "a blocked install must write nothing");
}

#[test]
fn install_is_idempotent_and_preserves_unrelated_files() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [base](instructions/base.md)");
    write(src.path(), "instructions/base.md", "base content");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    // An unrelated pre-existing file under the destination must survive.
    write(&explicit, "unrelated.txt", "keep me");

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

    let first = install_plan(&plan, src.path()).expect("first install succeeds");
    assert_eq!(first.written.len(), 2);
    assert_eq!(first.unchanged.len(), 0);

    let second_plan = build_plan(&inputs).expect("second plan should build");
    let second = install_plan(&second_plan, src.path()).expect("second install succeeds");
    assert_eq!(second.written.len(), 0, "re-install must be a no-op");
    assert_eq!(second.unchanged.len(), 2);

    assert_eq!(
        fs::read_to_string(explicit.join("unrelated.txt")).unwrap(),
        "keep me"
    );
}

// -- guidance get (W9, AC1-AC10) ----------------------------------------------

use super::{GuidanceGetArgs, run_get};
use crate::guidance::profile::DestinationScopeKind as ScopeKind;

/// Recursively copies `fixture` to `dest`, standing in for `git clone`.
fn copy_dir_recursive(fixture: &Path, dest: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(fixture)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn fake_clone_from(fixture_root: PathBuf) -> impl Fn(&str, &Path) -> Result<(), String> {
    move |_url, dest| copy_dir_recursive(&fixture_root, dest).map_err(|e| e.to_string())
}

fn failing_clone(_url: &str, _dest: &Path) -> Result<(), String> {
    Err("simulated network failure".to_string())
}

fn get_args(repository_url: &str, select: Vec<String>, target: PathBuf) -> GuidanceGetArgs {
    GuidanceGetArgs {
        repository_url: repository_url.to_string(),
        select,
        profile: None,
        target,
        destination_scope: Some(ScopeKind::Explicit),
        destination_path: None,
        keep_checkout: false,
        json: false,
    }
}

#[test]
fn get_profile_less_select_installs_a_bare_path_directly() {
    let fixture = TempDir::new().unwrap();
    write(
        fixture.path(),
        "prompt.md",
        "see [base](instructions/base.md)",
    );
    write(fixture.path(), "instructions/base.md", "base content");

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit.clone());

    let (result, checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    result.expect("profile-less get should succeed");
    assert!(explicit.join("prompt.md").is_file());
    assert!(explicit.join("instructions/base.md").is_file());
    assert!(
        !checkout_path.exists(),
        "managed checkout must be cleaned up by default"
    );
}

#[test]
fn get_explicit_profile_is_resolved_inside_the_clone() {
    let fixture = TempDir::new().unwrap();
    write(fixture.path(), "prompt.md", "no links here");
    write(
        fixture.path(),
        "guidance.toml",
        "[profile]\nid = \"repo-published\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"prompt.md\"]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n",
    );

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["root".to_string()],
        target.path().to_path_buf(),
    );
    args.profile = Some(PathBuf::from("guidance.toml"));
    args.destination_path = Some(explicit.clone());

    let (result, checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    result.expect("explicit-profile get should succeed");
    assert!(explicit.join("prompt.md").is_file());
    assert!(!checkout_path.exists());
}

#[test]
fn get_keep_checkout_retains_the_managed_directory() {
    let fixture = TempDir::new().unwrap();
    write(fixture.path(), "prompt.md", "content");

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit);
    args.keep_checkout = true;

    let (result, checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    result.expect("get with --keep-checkout should still succeed");
    assert!(
        checkout_path.join("prompt.md").is_file(),
        "kept checkout must still contain the cloned content"
    );
}

#[test]
fn get_blocking_plan_writes_nothing_and_still_cleans_up() {
    let fixture = TempDir::new().unwrap();
    write(fixture.path(), "prompt.md", "see [gone](missing.md)");

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit.clone());

    let (result, checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    assert!(result.is_err(), "blocking plan must surface as an error");
    assert!(!explicit.exists(), "no writes on a blocking plan");
    assert!(!checkout_path.exists(), "cleanup runs even on failure");
}

#[test]
fn get_clone_failure_attempts_no_plan_or_install() {
    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");
    let mut args = get_args(
        "https://example.invalid/unreachable.git",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit.clone());

    let (result, checkout_path) = run_get(&args, failing_clone);

    let error = result.expect_err("clone failure must surface as an error");
    assert!(error.contains("failed to clone"));
    assert!(!explicit.exists());
    assert!(!checkout_path.exists());
}

#[test]
fn get_rejects_absolute_select_path_before_any_clone_side_effect() {
    let target = TempDir::new().unwrap();
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["/etc/passwd".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(target.path().join("out"));

    let fixture = TempDir::new().unwrap();
    let (result, _checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    let error = result.expect_err("absolute select path must be rejected");
    assert!(error.contains("absolute"));
}

#[test]
fn get_rerun_is_idempotent() {
    let fixture = TempDir::new().unwrap();
    write(fixture.path(), "prompt.md", "content");

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit.clone());

    let (first, _) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));
    first.expect("first get should succeed");

    let (second, _) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));
    second.expect("second get should succeed and be idempotent");
    assert!(explicit.join("prompt.md").is_file());
}

#[test]
fn get_installs_all_meta_workspace_guidance() {
    let fixture = TempDir::new().unwrap();
    write(
        fixture.path(),
        "context-engine/AGENTS.md",
        "See [world model](../workflow-tools/.agents/instructions/workflow/agent-world-model.instructions.md) and [implement](../.agents/agents/implement.agent.md).",
    );
    write(
        fixture.path(),
        "workflow-tools/.agents/instructions/workflow/agent-world-model.instructions.md",
        "See [code quality](code-quality.instructions.md).",
    );
    write(
        fixture.path(),
        "workflow-tools/.agents/instructions/workflow/code-quality.instructions.md",
        "# Code Quality Rules",
    );
    write(
        fixture.path(),
        ".agents/agents/implement.agent.md",
        "See [write and die](../instructions/workflow/write-and-die.instructions.md).",
    );
    write(
        fixture.path(),
        ".agents/instructions/workflow/write-and-die.instructions.md",
        "# Write and Die Protocol",
    );
    write(
        fixture.path(),
        "workflow-tools/.agents/agents/orchestrator.agent.md",
        "# Orchestrator Agent",
    );
    write(
        fixture.path(),
        "workflow-tools/.agents/prompts/iteration.prompt.md",
        "# Iteration Prompt",
    );

    let target = TempDir::new().unwrap();
    let mut args = get_args(
        "https://github.com/mankinskin/meta-workspace.git",
        vec![
            "context-engine/AGENTS.md".to_string(),
            ".agents/agents/implement.agent.md".to_string(),
            "workflow-tools/.agents/agents/orchestrator.agent.md".to_string(),
            "workflow-tools/.agents/prompts/iteration.prompt.md".to_string(),
        ],
        target.path().to_path_buf(),
    );
    args.destination_scope = Some(ScopeKind::Repo);

    let (result, checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    result.expect("installing all meta-workspace guidance should succeed");

    // Under repo scope, AGENTS.md stays at context-engine/AGENTS.md, while .agents/... lands under target's .agents/
    let dest = target.path();
    assert!(dest.join(".agents/context-engine/AGENTS.md").is_file());
    assert!(
        dest.join(".agents/instructions/workflow/agent-world-model.instructions.md")
            .is_file()
    );
    assert!(
        dest.join(".agents/instructions/workflow/code-quality.instructions.md")
            .is_file()
    );
    assert!(dest.join(".agents/agents/implement.agent.md").is_file());
    assert!(
        dest.join(".agents/instructions/workflow/write-and-die.instructions.md")
            .is_file()
    );
    assert!(dest.join(".agents/agents/orchestrator.agent.md").is_file());
    assert!(dest.join(".agents/prompts/iteration.prompt.md").is_file());

    assert!(
        !checkout_path.exists(),
        "managed checkout must be cleaned up after installing all guidance"
    );
}

#[test]
fn installer_oneliner_updates_existing_installation_with_force_flag() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_script = manifest_dir.join("../../install.sh");
    assert!(
        relative_script.is_file(),
        "install.sh must exist at {}",
        relative_script.display()
    );

    let output = std::process::Command::new("bash")
        .current_dir(&manifest_dir)
        .arg("../../install.sh")
        .arg("--root")
        .arg("/tmp/mock-install-root")
        .arg("--dry-run")
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                out.status.success(),
                "install.sh --dry-run must succeed, status: {:?}, stdout: {stdout}, stderr: {stderr}",
                out.status
            );
            assert!(
                stdout.contains("cargo install --force"),
                "installer command must pass --force to ensure re-install/update overwrites an existing install-ctl binary: got {stdout}"
            );
            assert!(
                stdout.contains("--branch main"),
                "installer command must target --branch main by default to pull latest revision: got {stdout}"
            );
        }
        Err(e) => {
            eprintln!("skipping install.sh test: bash process launch failed: {e}");
        }
    }
}

#[test]
fn installer_oneliner_fetches_and_builds_latest_branch_revision_in_sandbox() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_script = manifest_dir.join("../../install.sh");
    if !relative_script.is_file() {
        return;
    }

    let repo_temp = TempDir::new().unwrap();
    let repo_dir = repo_temp.path();

    let git_init = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_dir)
        .output();
    let Ok(init_out) = git_init else {
        eprintln!("skipping test: git command not available");
        return;
    };
    if !init_out.status.success() {
        eprintln!("skipping test: git init failed");
        return;
    }

    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_dir)
        .output();

    write(
        repo_dir,
        "Cargo.toml",
        r#"[package]
name = "install-ctl"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "install-ctl"
path = "src/main.rs"
"#,
    );
    write(repo_dir, "src/main.rs", r#"fn main() { println!("v1"); }"#);

    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "v1"])
        .current_dir(repo_dir)
        .output();

    let raw_repo_path = repo_dir.to_string_lossy().replace('\\', "/");
    let clean_repo_path = raw_repo_path
        .trim_start_matches("//?/")
        .trim_start_matches("\\\\?\\")
        .trim_start_matches('/');
    let repo_url = format!("file:///{clean_repo_path}");

    let install_target = TempDir::new().unwrap();
    let install_root = install_target.path().to_string_lossy().replace('\\', "/");

    let mut path_env = std::env::var("PATH").unwrap_or_default();
    if let Ok(cargo_home) = std::env::var("CARGO_HOME").or_else(|_| {
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(|h| format!("{h}/.cargo"))
    }) {
        let bin_dir = format!("{cargo_home}/bin");
        path_env = format!("{bin_dir};{bin_dir}:{path_env}");
    }

    let run1 = std::process::Command::new("bash")
        .current_dir(&manifest_dir)
        .env("PATH", &path_env)
        .arg("../../install.sh")
        .arg("--root")
        .arg(&install_root)
        .arg("--repository")
        .arg(&repo_url)
        .arg("--branch")
        .arg("main")
        .output();

    let Ok(out1) = run1 else {
        eprintln!("skipping test: bash or cargo not available");
        return;
    };
    assert!(
        out1.status.success(),
        "first install.sh run must succeed: stdout: {}, stderr: {}",
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr)
    );

    let bin_path = if cfg!(windows) {
        install_target.path().join("bin/install-ctl.exe")
    } else {
        install_target.path().join("bin/install-ctl")
    };
    assert!(
        bin_path.is_file(),
        "installed binary must exist after first run"
    );

    let run_bin1 = std::process::Command::new(&bin_path).output().unwrap();
    let output_v1 = String::from_utf8_lossy(&run_bin1.stdout).trim().to_string();
    assert_eq!(output_v1, "v1", "first install must produce v1 binary");

    write(repo_dir, "src/main.rs", r#"fn main() { println!("v2"); }"#);
    let _ = std::process::Command::new("git")
        .args(["commit", "-am", "v2"])
        .current_dir(repo_dir)
        .output();

    let run2 = std::process::Command::new("bash")
        .current_dir(&manifest_dir)
        .env("PATH", &path_env)
        .arg("../../install.sh")
        .arg("--root")
        .arg(&install_root)
        .arg("--repository")
        .arg(&repo_url)
        .arg("--branch")
        .arg("main")
        .output()
        .unwrap();

    assert!(
        run2.status.success(),
        "second install.sh run must succeed: stdout: {}, stderr: {}",
        String::from_utf8_lossy(&run2.stdout),
        String::from_utf8_lossy(&run2.stderr)
    );

    let run_bin2 = std::process::Command::new(&bin_path).output().unwrap();
    let output_v2 = String::from_utf8_lossy(&run_bin2.stdout).trim().to_string();
    assert_eq!(
        output_v2, "v2",
        "second install must update installed binary to v2 from latest main branch"
    );
}

#[test]
fn guidance_uninstall_removes_installed_plan() {
    let source = TempDir::new().unwrap();
    write(source.path(), "prompt.md", "content");

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");

    let mut args = get_args(
        "unused",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit.clone());

    let (result, _) = run_get(&args, fake_clone_from(source.path().to_path_buf()));
    result.expect("install should succeed");
    assert!(explicit.join("prompt.md").is_file());

    let plan_inputs = super::PlanInputs {
        source_root: source.path(),
        profile_path: source.path(),
        select: &["prompt.md".to_string()],
        target_root: target.path(),
        scope_override: Some(ScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &super::DestinationPaths::platform_default(),
    };
    let profile = super::synthesize_direct_profile(&["prompt.md".to_string()]).unwrap();
    let plan = super::build_plan_with_profile(profile, &plan_inputs).unwrap();

    let report = super::uninstall::uninstall_plan(&plan).unwrap();
    assert_eq!(report.removed.len(), 1);
    assert!(!explicit.join("prompt.md").exists());
}
