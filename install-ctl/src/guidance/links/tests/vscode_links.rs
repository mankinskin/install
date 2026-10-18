use super::super::{vscode, vscode_links};
use super::*;

fn plan(repo: &Path, sources: &[&str], operation: LinkOperation, migrate: bool) -> vscode::Plan {
    vscode::build_plan(
        repo,
        &sources
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        operation,
        migrate,
    )
    .unwrap()
}

#[test]
fn ordinary_conflict_blocks_before_any_write() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join(".github/agents/root")).unwrap();
    let result = plan(repo.path(), &["root=.agents"], LinkOperation::Link, false);
    assert!(!result.diagnostics.is_empty());
    assert!(vscode_links::apply_plan(&result).is_err());
    assert!(!repo.path().join(".github/instructions").exists());
}

#[test]
fn stale_plan_blocks_before_any_write() {
    let repo = fixture();
    let result = plan(repo.path(), &["root=.agents"], LinkOperation::Link, false);
    fs::create_dir_all(repo.path().join(".github/prompts/root")).unwrap();
    let error = vscode_links::apply_plan(&result).err().unwrap();
    assert!(error.contains("stale"), "{error}");
    assert!(!repo.path().join(".github/agents").exists());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn named_lifecycle_preserves_unselected_sources_and_user_entries() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join("nested/.agents/agents")).unwrap();
    fs::create_dir_all(repo.path().join(".github/agents")).unwrap();
    fs::write(repo.path().join(".github/agents/user.agent.md"), "user").unwrap();
    let selected = ["root=.agents", "nested=nested/.agents"];
    let result = plan(repo.path(), &selected, LinkOperation::Link, false);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    vscode_links::apply_plan(&result).unwrap();
    assert_eq!(
        fs::read_link(repo.path().join(".github/agents/root")).unwrap(),
        PathBuf::from("../../.agents/agents")
    );
    let result = plan(repo.path(), &["root=.agents"], LinkOperation::Unlink, false);
    vscode_links::apply_plan(&result).unwrap();
    assert!(fs::symlink_metadata(repo.path().join(".github/agents/root")).is_err());
    assert!(repo.path().join(".github/agents/nested").is_dir());
    assert_eq!(
        fs::read_to_string(repo.path().join(".github/agents/user.agent.md")).unwrap(),
        "user"
    );
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn migration_is_explicit_and_rollback_restores_raw_alias() {
    let repo = fixture();
    fs::create_dir(repo.path().join(".github")).unwrap();
    let alias = repo.path().join(".github/agents");
    let raw = PathBuf::from("../.agents/agents");
    create_symlink(&raw, &alias).unwrap();
    let refused = plan(repo.path(), &["root=.agents"], LinkOperation::Link, false);
    assert!(!refused.diagnostics.is_empty());
    let allowed = plan(repo.path(), &["root=.agents"], LinkOperation::Link, true);
    assert!(allowed.diagnostics.is_empty(), "{:?}", allowed.diagnostics);
    let journal = vscode_links::apply_plan(&allowed).unwrap();
    assert!(alias.join("root").is_dir());
    journal.rollback().unwrap();
    assert_eq!(fs::read_link(&alias).unwrap(), raw);
    assert!(!repo.path().join(".github/instructions").exists());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn indirect_ancestor_and_foreign_legacy_alias_are_blocked() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join("foreign")).unwrap();
    create_symlink(Path::new("foreign"), &repo.path().join(".github")).unwrap();
    let indirect = plan(repo.path(), &["root=.agents"], LinkOperation::Link, true);
    assert!(!indirect.diagnostics.is_empty());
    remove_symlink(&repo.path().join(".github")).unwrap();
    fs::create_dir(repo.path().join(".github")).unwrap();
    create_symlink(Path::new("../foreign"), &repo.path().join(".github/agents")).unwrap();
    let foreign = plan(repo.path(), &["root=.agents"], LinkOperation::Link, true);
    assert!(!foreign.diagnostics.is_empty());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn unlink_journal_restores_removed_links_after_sources_disappear() {
    let repo = fixture();
    let result = plan(repo.path(), &["root=.agents"], LinkOperation::Link, false);
    vscode_links::apply_plan(&result).unwrap();
    let source = repo.path().join(".agents");
    fs::rename(&source, repo.path().join("hidden-agents")).unwrap();
    let unlink = plan(repo.path(), &["root=.agents"], LinkOperation::Unlink, false);
    assert!(unlink.diagnostics.is_empty(), "{:?}", unlink.diagnostics);
    let journal = vscode_links::apply_plan(&unlink).unwrap();
    journal.rollback().unwrap();
    assert_eq!(
        fs::read_link(repo.path().join(".github/agents/root")).unwrap(),
        PathBuf::from("../../.agents/agents")
    );
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn raw_target_change_invalidates_plan_even_when_resolved_target_matches() {
    let repo = fixture();
    vscode_links::apply_plan(&plan(
        repo.path(),
        &["root=.agents"],
        LinkOperation::Link,
        false,
    ))
    .unwrap();
    let before = plan(repo.path(), &["root=.agents"], LinkOperation::Unlink, false);
    let path = repo.path().join(".github/agents/root");
    remove_symlink(&path).unwrap();
    create_symlink(
        &fs::canonicalize(repo.path().join(".agents/agents")).unwrap(),
        &path,
    )
    .unwrap();
    assert!(
        vscode_links::apply_plan(&before)
            .unwrap_err()
            .contains("stale")
    );
    assert!(path.is_dir());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn injected_failure_restores_migration_and_removes_only_created_paths() {
    let repo = fixture();
    fs::create_dir(repo.path().join(".github")).unwrap();
    let alias = repo.path().join(".github/agents");
    create_symlink(Path::new("../.agents/agents"), &alias).unwrap();
    let raw = fs::read_link(&alias).unwrap();
    let selected = plan(repo.path(), &["root=.agents"], LinkOperation::Link, true);
    let mut count = 0;
    let result = vscode_links::apply_with_creator(&selected, |target, path| {
        count += 1;
        if count == 2 {
            Err(std::io::Error::other("injected creation failure"))
        } else {
            create_symlink(target, path)
        }
    });
    assert!(result.unwrap_err().contains("injected creation failure"));
    assert_eq!(fs::read_link(&alias).unwrap(), raw);
    assert!(!repo.path().join(".github/instructions").exists());
    assert!(!repo.path().join(".github/prompts").exists());
}
