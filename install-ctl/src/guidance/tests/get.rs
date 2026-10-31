use super::*;

// -- guidance get (W9, AC1-AC10) ----------------------------------------------

use crate::guidance::profile::DestinationScopeKind as ScopeKind;
use crate::guidance::{GuidanceGetArgs, run_get, run_get_with_reporter};

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

pub(super) fn fake_clone_from(fixture_root: PathBuf) -> impl Fn(&str, &Path) -> Result<(), String> {
    move |_url, dest| copy_dir_recursive(&fixture_root, dest).map_err(|e| e.to_string())
}

fn failing_clone(_url: &str, _dest: &Path) -> Result<(), String> {
    Err("simulated network failure".to_string())
}

pub(super) fn get_args(repository_url: &str, select: Vec<String>, target: PathBuf) -> GuidanceGetArgs {
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
fn get_reports_each_execution_phase() {
    let fixture = TempDir::new().unwrap();
    write(fixture.path(), "prompt.md", "content");
    let target = TempDir::new().unwrap();
    let mut args = get_args(
        "https://example.invalid/repo.git",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(target.path().join("out"));
    let messages = std::cell::RefCell::new(Vec::new());

    let (result, _) = run_get_with_reporter(
        &args,
        fake_clone_from(fixture.path().to_path_buf()),
        &|message| messages.borrow_mut().push(message.to_string()),
    );

    result.expect("get should succeed");
    assert_eq!(
        messages.into_inner(),
        vec![
            "creating managed checkout",
            "acquiring selected source files",
            "validated selected guidance profile",
            "building installation plan",
            "installing guidance artifacts",
            "cleaning up managed checkout",
        ]
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
    write(fixture.path(), "prompt.md", "see [out](../../outside.md)");

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
    assert!(error.contains("failed to acquire selected guidance"));
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
        "workflow-tools/AGENTS.md",
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
            "workflow-tools/AGENTS.md".to_string(),
            ".agents/agents/implement.agent.md".to_string(),
            "workflow-tools/.agents/agents/orchestrator.agent.md".to_string(),
            "workflow-tools/.agents/prompts/iteration.prompt.md".to_string(),
        ],
        target.path().to_path_buf(),
    );
    args.destination_scope = Some(ScopeKind::Repo);

    let (result, checkout_path) = run_get(&args, fake_clone_from(fixture.path().to_path_buf()));

    result.expect("installing all meta-workspace guidance should succeed");

    // Under repo scope, AGENTS.md stays at workflow-tools/AGENTS.md, while .agents/... lands under target's .agents/
    let dest = target.path();
    assert!(dest.join(".agents/workflow-tools/AGENTS.md").is_file());
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
