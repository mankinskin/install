use super::super::settings;
use super::*;

fn build(
    repo: &Path,
    selected: &[&str],
    operation: LinkOperation,
) -> Result<settings::Plan, String> {
    settings::build_plan(
        repo,
        &selected.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        operation,
        false,
    )
}

fn apply(repo: &Path, selected: &[&str], operation: LinkOperation) {
    settings::apply_plan(&build(repo, selected, operation).unwrap()).unwrap();
}

fn put_settings(repo: &Path, text: &str) {
    fs::create_dir_all(repo.join(".vscode")).unwrap();
    fs::write(repo.join(".vscode/settings.json"), text).unwrap();
}

#[test]
fn invalid_settings_block_without_mutation() {
    for value in [
        "[]",
        "{\"a\":1 \"b\":2}",
        "{'a':1}",
        "{a:1}",
        "{ broken",
        "{\"chat.agentFilesLocations\":[]}",
        "{\"chat.agentFilesLocations\":{\".github/agents/root\":false}}",
        "{\"chat.agentFilesLocations\":{\".github/agents/root\":7}}",
        "{\"chat.agentFilesLocations\":{},\"chat.agentFilesLocations\":{}}",
        "{\"chat.agentFilesLocations\":{\".github/agents/root\":true,\".github/agents/root\":true}}",
    ] {
        let repo = fixture();
        put_settings(repo.path(), value);
        let result = build(repo.path(), &["root=.agents"], LinkOperation::Link);
        let blocked = match result {
            Err(_) => true,
            Ok(plan) => settings::apply_plan(&plan).is_err(),
        };
        assert!(blocked, "{value}");
        assert_eq!(
            fs::read_to_string(repo.path().join(".vscode/settings.json")).unwrap(),
            value
        );
        assert!(!repo.path().join(".github").exists());
    }
}

#[test]
fn settings_staleness_blocks_before_links_are_created() {
    let repo = fixture();
    let plan = build(repo.path(), &["root=.agents"], LinkOperation::Link).unwrap();
    put_settings(repo.path(), "{\"user\":true}");
    assert!(settings::apply_plan(&plan).unwrap_err().contains("stale"));
    assert!(!repo.path().join(".github").exists());
}

#[test]
fn unowned_absent_unlink_is_a_true_noop() {
    let repo = fixture();
    put_settings(
        repo.path(),
        "{\"chat.agentFilesLocations\":{\".github/agents/root\":true}}",
    );
    let before = fs::read(repo.path().join(".vscode/settings.json")).unwrap();
    apply(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    assert_eq!(
        fs::read(repo.path().join(".vscode/settings.json")).unwrap(),
        before
    );
    assert!(
        !repo
            .path()
            .join(".vscode/install-ctl-guidance-links.json")
            .exists()
    );
}

#[test]
fn invalid_ownership_is_not_adopted() {
    for record in ["{}", "{\"version\":99,\"sources\":{}}", "not json"] {
        let repo = fixture();
        put_settings(repo.path(), "{}");
        fs::write(
            repo.path().join(".vscode/install-ctl-guidance-links.json"),
            record,
        )
        .unwrap();
        let result = build(repo.path(), &["root=.agents"], LinkOperation::Link);
        assert!(result.is_err() || settings::apply_plan(&result.unwrap()).is_err());
        assert!(!repo.path().join(".github").exists());
    }
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn jsonc_comments_crlf_and_user_owned_true_survive_unlink() {
    let repo = fixture();
    let original = "{\r\n\t// retain me\r\n\t\"editor.tabSize\": 3,\r\n\t\"chat.agentFilesLocations\": {\".github/agents/root\": true,},\r\n}\r\n";
    put_settings(repo.path(), original);
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let linked = fs::read_to_string(repo.path().join(".vscode/settings.json")).unwrap();
    assert!(linked.contains("// retain me"));
    assert!(linked.contains("\"editor.tabSize\": 3"));
    assert!(!linked.replace("\r\n", "").contains('\n'));
    assert!(linked.contains("\t\"editor.tabSize\""));
    apply(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    let after = fs::read_to_string(repo.path().join(".vscode/settings.json")).unwrap();
    assert!(after.contains("// retain me"));
    assert!(after.contains("\".github/agents/root\": true"));
    assert!(after.contains("\"editor.tabSize\": 3"));
    assert!(!after.contains(".github/prompts/root"));
    assert!(
        !repo
            .path()
            .join(".vscode/install-ctl-guidance-links.json")
            .exists()
    );
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn subset_unlink_retains_other_sources_and_last_cleanup_removes_created_settings() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join("nested/.agents/agents")).unwrap();
    apply(
        repo.path(),
        &["root=.agents", "nested=nested/.agents"],
        LinkOperation::Link,
    );
    apply(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    assert!(repo.path().join(".github/agents/nested").is_dir());
    let remaining = fs::read_to_string(repo.path().join(".vscode/settings.json")).unwrap();
    assert!(remaining.contains(".github/agents/nested"));
    assert!(!remaining.contains(".github/agents/root"));
    apply(
        repo.path(),
        &["nested=nested/.agents"],
        LinkOperation::Unlink,
    );
    assert!(!repo.path().join(".vscode/settings.json").exists());
    assert!(
        !repo
            .path()
            .join(".vscode/install-ctl-guidance-links.json")
            .exists()
    );
    assert!(repo.path().join(".github/agents").is_dir());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn changed_owned_value_blocks_unlink_and_preserves_links() {
    let repo = fixture();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let path = repo.path().join(".vscode/settings.json");
    let text = fs::read_to_string(&path).unwrap().replace("true", "false");
    fs::write(&path, &text).unwrap();
    let result = build(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    assert!(result.is_err() || settings::apply_plan(&result.unwrap()).is_err());
    assert!(repo.path().join(".github/agents/root").is_dir());
    assert_eq!(fs::read_to_string(path).unwrap(), text);
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn source_rebinding_and_unowned_selected_links_are_rejected() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join("other/agents")).unwrap();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    assert!(build(repo.path(), &["root=other"], LinkOperation::Link).is_err());
    fs::remove_file(repo.path().join(".vscode/install-ctl-guidance-links.json")).unwrap();
    assert!(build(repo.path(), &["root=.agents"], LinkOperation::Unlink).is_err());
    assert!(repo.path().join(".github/agents/root").is_dir());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn incremental_link_is_idempotent_and_missing_sources_allow_cleanup() {
    let repo = fixture();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let settings = fs::read(repo.path().join(".vscode/settings.json")).unwrap();
    let record = fs::read(repo.path().join(".vscode/install-ctl-guidance-links.json")).unwrap();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    assert_eq!(
        fs::read(repo.path().join(".vscode/settings.json")).unwrap(),
        settings
    );
    assert_eq!(
        fs::read(repo.path().join(".vscode/install-ctl-guidance-links.json")).unwrap(),
        record
    );
    fs::rename(repo.path().join(".agents"), repo.path().join("hidden")).unwrap();
    apply(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    assert!(!repo.path().join(".vscode/settings.json").exists());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn each_configuration_failure_restores_exact_bytes_and_raw_legacy_target() {
    for failing_write in [1, 2] {
        let repo = fixture();
        let original = "{\r\n  // user settings\r\n  \"editor.tabSize\": 3,\r\n}\r\n";
        put_settings(repo.path(), original);
        fs::create_dir(repo.path().join(".github")).unwrap();
        let alias = repo.path().join(".github/agents");
        create_symlink(Path::new("../.agents/agents"), &alias).unwrap();
        let raw = fs::read_link(&alias).unwrap();
        let selected = settings::build_plan(
            repo.path(),
            &["root=.agents".to_owned()],
            LinkOperation::Link,
            true,
        )
        .unwrap();
        let mut count = 0;
        let result = settings::apply_with_writer(&selected, |path, after| {
            count += 1;
            fs::write(path, after.as_ref().unwrap()).map_err(|error| error.to_string())?;
            if count == failing_write {
                Err("injected after replacement".to_owned())
            } else {
                Ok(())
            }
        });
        assert!(result.unwrap_err().contains("injected after replacement"));
        assert_eq!(
            fs::read(repo.path().join(".vscode/settings.json")).unwrap(),
            original.as_bytes()
        );
        assert!(
            !repo
                .path()
                .join(".vscode/install-ctl-guidance-links.json")
                .exists()
        );
        assert_eq!(fs::read_link(&alias).unwrap(), raw);
        assert!(!repo.path().join(".github/instructions").exists());
    }
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn failed_unlink_restores_links_and_both_configuration_files() {
    let repo = fixture();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let settings_path = repo.path().join(".vscode/settings.json");
    let record_path = repo.path().join(".vscode/install-ctl-guidance-links.json");
    let before_settings = fs::read(&settings_path).unwrap();
    let before_record = fs::read(&record_path).unwrap();
    let selected = build(repo.path(), &["root=.agents"], LinkOperation::Unlink).unwrap();
    let result = settings::apply_with_writer(&selected, |path, after| {
        match after {
            Some(bytes) => fs::write(path, bytes).unwrap(),
            None => fs::remove_file(path).unwrap(),
        }
        Err("injected unlink failure".to_owned())
    });
    assert!(result.unwrap_err().contains("injected unlink failure"));
    assert_eq!(fs::read(settings_path).unwrap(), before_settings);
    assert_eq!(fs::read(record_path).unwrap(), before_record);
    assert!(repo.path().join(".github/agents/root").is_dir());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn user_comments_added_to_created_settings_are_preserved() {
    let repo = fixture();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let path = repo.path().join(".vscode/settings.json");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("// user note\n{text}")).unwrap();
    apply(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    assert!(fs::read_to_string(path).unwrap().contains("// user note"));
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn failure_after_creating_settings_removes_only_call_created_directories() {
    let repo = fixture();
    let selected = build(repo.path(), &["root=.agents"], LinkOperation::Link).unwrap();
    let result = settings::apply_with_writer(&selected, |path, after| {
        fs::create_dir(path.parent().unwrap()).unwrap();
        fs::write(path, after.as_ref().unwrap()).unwrap();
        Err("injected new-settings failure".to_owned())
    });
    assert!(
        result
            .unwrap_err()
            .contains("injected new-settings failure")
    );
    assert!(!repo.path().join(".github").exists());
    assert!(!repo.path().join(".vscode").exists());
    assert_eq!(
        fs::read_to_string(repo.path().join(".agents/agents/example.md")).unwrap(),
        "original"
    );
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn ownership_byte_changes_invalidate_unlink_plan_without_removing_links() {
    let repo = fixture();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let plan = build(repo.path(), &["root=.agents"], LinkOperation::Unlink).unwrap();
    let path = repo.path().join(".vscode/install-ctl-guidance-links.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(&path, &bytes).unwrap();
    assert!(settings::apply_plan(&plan).unwrap_err().contains("stale"));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(repo.path().join(".github/agents/root").is_dir());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn comments_next_to_owned_properties_are_not_erased_on_unlink() {
    let repo = fixture();
    apply(repo.path(), &["root=.agents"], LinkOperation::Link);
    let path = repo.path().join(".vscode/settings.json");
    let text = fs::read_to_string(&path).unwrap().replace(
        "\".github/agents/root\"",
        "// user annotation\n    \".github/agents/root\"",
    );
    fs::write(&path, text).unwrap();
    apply(repo.path(), &["root=.agents"], LinkOperation::Unlink);
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .contains("// user annotation")
    );
}
