use super::*;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let repo = TempDir::new().unwrap();
    for directory in ["agents", "instructions", "prompts"] {
        fs::create_dir_all(repo.path().join(".agents").join(directory)).unwrap();
        fs::write(
            repo.path()
                .join(".agents")
                .join(directory)
                .join("example.md"),
            "original",
        )
        .unwrap();
    }
    repo
}

fn plan(repo: &Path, operation: LinkOperation) -> LinkPlan {
    build_link_plan(repo, Harness::CopilotCli, operation).unwrap()
}

#[test]
fn planning_reports_relative_mappings_without_writes() {
    let repo = fixture();
    let result = plan(repo.path(), LinkOperation::Link);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.entries.len(), 3);
    assert_eq!(result.entries[0].target, PathBuf::from("../.agents/agents"));
    assert_eq!(result.entries[2].destination, ".claude/commands");
    #[cfg(windows)]
    assert!(
        result
            .entries
            .iter()
            .all(|entry| !entry.target.to_string_lossy().contains('/'))
    );
    assert!(
        result
            .entries
            .iter()
            .all(|entry| entry.action == LinkAction::Create)
    );
    assert!(!repo.path().join(".github").exists());
    assert!(!repo.path().join(".claude").exists());
}

#[test]
fn missing_optional_sources_are_skipped_but_empty_corpus_blocks() {
    let repo = TempDir::new().unwrap();
    let empty = plan(repo.path(), LinkOperation::Link);
    assert_eq!(empty.skipped_sources.len(), 3);
    assert_eq!(empty.diagnostics.len(), 1);
    fs::create_dir_all(repo.path().join(".agents/agents")).unwrap();
    let partial = plan(repo.path(), LinkOperation::Link);
    assert!(partial.diagnostics.is_empty());
    assert_eq!(partial.entries.len(), 1);
    assert_eq!(partial.skipped_sources.len(), 2);
}

#[test]
fn real_destination_blocks_all_writes_and_unlink() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join(".github/instructions")).unwrap();
    fs::write(repo.path().join(".github/instructions/keep.md"), "keep").unwrap();
    let result = plan(repo.path(), LinkOperation::Link);
    assert!(
        apply_link_plan(&result)
            .unwrap_err()
            .contains("not a symbolic link")
    );
    assert!(!repo.path().join(".github/agents").exists());
    assert!(apply_link_plan(&plan(repo.path(), LinkOperation::Unlink)).is_err());
    assert_eq!(
        fs::read_to_string(repo.path().join(".github/instructions/keep.md")).unwrap(),
        "keep"
    );
}

#[test]
fn stale_plan_refuses_new_destination_conflicts() {
    let repo = fixture();
    let result = plan(repo.path(), LinkOperation::Link);
    fs::create_dir_all(repo.path().join(".github/agents")).unwrap();
    assert!(apply_link_plan(&result).unwrap_err().contains("stale"));
    assert!(!repo.path().join(".claude").exists());
}

#[test]
fn creation_failure_has_no_fallback_and_removes_only_new_parents() {
    let repo = fixture();
    let result = plan(repo.path(), LinkOperation::Link);
    let error = apply_with_creator(&result, |_, _| {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
    })
    .unwrap_err();
    assert!(error.contains("denied"));
    assert!(error.contains("no copy or junction fallback"));
    assert!(!repo.path().join(".github").exists());
    assert!(!repo.path().join(".claude").exists());
    assert_eq!(
        fs::read_to_string(repo.path().join(".agents/agents/example.md")).unwrap(),
        "original"
    );
}

#[test]
fn creation_failure_preserves_preexisting_empty_parent() {
    let repo = fixture();
    fs::create_dir(repo.path().join(".github")).unwrap();
    let result = plan(repo.path(), LinkOperation::Link);
    assert!(apply_with_creator(&result, |_, _| Err(io::Error::other("failure"))).is_err());
    assert!(repo.path().join(".github").is_dir());
    assert!(!repo.path().join(".github/agents").exists());
}

#[test]
fn unlink_without_sources_is_read_only_and_idempotent() {
    let repo = TempDir::new().unwrap();
    let result = plan(repo.path(), LinkOperation::Unlink);
    assert!(result.diagnostics.is_empty());
    assert!(
        result
            .entries
            .iter()
            .all(|entry| entry.action == LinkAction::Missing)
    );
    apply_link_plan(&result).unwrap();
    assert_eq!(fs::read_dir(repo.path()).unwrap().count(), 0);
}

#[test]
#[cfg(windows)]
fn windows_junctions_are_not_adopted_or_traversed() {
    let repo = fixture();
    fs::create_dir(repo.path().join(".github")).unwrap();
    let junction = repo.path().join(".github/agents");
    let output = std::process::Command::new("cmd.exe")
        .current_dir(repo.path())
        .args(["/C", "mklink", "/J", r".github\agents", r".agents\agents"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = plan(repo.path(), LinkOperation::Link);
    assert!(
        !result.diagnostics.is_empty(),
        "junction must not be adopted as a symbolic link"
    );
    assert!(
        !plan(repo.path(), LinkOperation::Unlink)
            .diagnostics
            .is_empty()
    );
    fs::remove_dir(&junction).unwrap();
    fs::remove_dir(repo.path().join(".github")).unwrap();
    let output = std::process::Command::new("cmd.exe")
        .current_dir(repo.path())
        .args(["/C", "mklink", "/J", ".github", ".agents"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        plan(repo.path(), LinkOperation::Link)
            .diagnostics
            .iter()
            .any(|error| error.contains("ancestor"))
    );
    fs::remove_dir(repo.path().join(".github")).unwrap();
    assert!(repo.path().join(".agents/agents/example.md").is_file());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires Windows Developer Mode or symlink privilege"
)]
fn real_links_are_live_relative_idempotent_and_safely_removed() {
    let repo = fixture();
    apply_link_plan(&plan(repo.path(), LinkOperation::Link)).unwrap();
    let alias = repo.path().join(".github/agents");
    assert!(
        fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&alias).unwrap(),
        PathBuf::from("../.agents/agents")
    );
    fs::write(repo.path().join(".agents/agents/example.md"), "changed").unwrap();
    assert_eq!(
        fs::read_to_string(alias.join("example.md")).unwrap(),
        "changed"
    );
    let repeat = plan(repo.path(), LinkOperation::Link);
    assert!(
        repeat
            .entries
            .iter()
            .all(|entry| entry.action == LinkAction::Unchanged)
    );
    apply_link_plan(&repeat).unwrap();
    apply_link_plan(&plan(repo.path(), LinkOperation::Unlink)).unwrap();
    assert!(fs::symlink_metadata(alias).is_err());
    assert_eq!(
        fs::read_to_string(repo.path().join(".agents/agents/example.md")).unwrap(),
        "changed"
    );
    apply_link_plan(&plan(repo.path(), LinkOperation::Unlink)).unwrap();
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires Windows Developer Mode or symlink privilege"
)]
fn broken_expected_links_can_be_unlinked_but_foreign_links_block() {
    let repo = fixture();
    apply_link_plan(&plan(repo.path(), LinkOperation::Link)).unwrap();
    fs::remove_dir_all(repo.path().join(".agents")).unwrap();
    apply_link_plan(&plan(repo.path(), LinkOperation::Unlink)).unwrap();
    create_symlink(Path::new("../foreign"), &repo.path().join(".github/agents")).unwrap();
    assert!(
        apply_link_plan(&plan(repo.path(), LinkOperation::Unlink))
            .unwrap_err()
            .contains("points elsewhere")
    );
    assert!(
        fs::symlink_metadata(repo.path().join(".github/agents"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires Windows Developer Mode or symlink privilege"
)]
fn source_escape_and_destination_ancestor_links_are_rejected() {
    let repo = fixture();
    let outside = TempDir::new().unwrap();
    create_symlink(outside.path(), &repo.path().join(".github")).unwrap();
    assert!(
        plan(repo.path(), LinkOperation::Link)
            .diagnostics
            .iter()
            .any(|error| error.contains("ancestor"))
    );
    remove_symlink(&repo.path().join(".github")).unwrap();
    fs::remove_dir_all(repo.path().join(".agents/agents")).unwrap();
    create_symlink(outside.path(), &repo.path().join(".agents/agents")).unwrap();
    assert!(
        plan(repo.path(), LinkOperation::Link)
            .diagnostics
            .iter()
            .any(|error| error.contains("inside the repository"))
    );
    assert!(!repo.path().join(".github").exists());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "requires Windows Developer Mode or symlink privilege"
)]
fn later_creation_failure_rolls_back_created_links() {
    let repo = fixture();
    let result = plan(repo.path(), LinkOperation::Link);
    let mut calls = 0;
    let error = apply_with_creator(&result, |target, destination| {
        calls += 1;
        if calls == 2 {
            Err(io::Error::other("second link failed"))
        } else {
            create_symlink(target, destination)
        }
    })
    .unwrap_err();
    assert!(error.contains("second link failed"));
    assert!(!repo.path().join(".github").exists());
    assert!(!repo.path().join(".claude").exists());
    assert!(repo.path().join(".agents/agents/example.md").is_file());
}
