use super::super::settings;
use super::*;

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn three_sources_are_registered_and_subset_unlink_preserves_user_content() {
    let repo = fixture();
    for source in ["workflow-tools", "context-engine"] {
        for kind in ["agents", "instructions", "prompts"] {
            fs::create_dir_all(repo.path().join(source).join(".agents").join(kind)).unwrap();
        }
    }
    let original = "---\napplyTo: \"root-target.txt\"\n---\nRetain source applicability.\n";
    fs::write(
        repo.path()
            .join(".agents/instructions/root.instructions.md"),
        original,
    )
    .unwrap();
    let sources = [
        "workflow-tools=workflow-tools/.agents",
        "root=.agents",
        "context-engine=context-engine/.agents",
    ]
    .map(str::to_owned);
    let before = settings::build_plan(repo.path(), &sources, LinkOperation::Link, false).unwrap();
    assert_eq!(before.links.entries.len(), 9);
    assert!(!repo.path().join(".github").exists());
    let json = serde_json::to_value(&before).unwrap();
    assert!(json["configuration_changes"].as_array().unwrap().len() >= 9);
    settings::apply_plan(&before).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(repo.path().join(".vscode/settings.json")).unwrap())
            .unwrap();
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(repo.path().join(".vscode/install-ctl-guidance-links.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["version"], 1);
    for (kind, key) in [
        ("agents", "chat.agentFilesLocations"),
        ("instructions", "chat.instructionsFilesLocations"),
        ("prompts", "chat.promptFilesLocations"),
    ] {
        for name in ["root", "workflow-tools", "context-engine"] {
            let destination = format!(".github/{kind}/{name}");
            assert_eq!(config[key][&destination], true);
            assert_eq!(
                record["sources"][name]["links"][kind]["destination"],
                destination
            );
            assert!(
                !fs::read_link(
                    repo.path()
                        .join(Path::new(&destination).components().collect::<PathBuf>())
                )
                .unwrap()
                .is_absolute()
            );
        }
    }
    let unlink = settings::build_plan(
        repo.path(),
        &["workflow-tools=workflow-tools/.agents".to_owned()],
        LinkOperation::Unlink,
        false,
    )
    .unwrap();
    settings::apply_plan(&unlink).unwrap();
    let config = fs::read_to_string(repo.path().join(".vscode/settings.json")).unwrap();
    assert!(config.contains(".github/agents/root"));
    assert!(config.contains(".github/prompts/context-engine"));
    assert!(!config.contains(".github/agents/workflow-tools"));
    assert_eq!(
        fs::read_to_string(
            repo.path()
                .join(".agents/instructions/root.instructions.md")
        )
        .unwrap(),
        original
    );
    assert!(!repo.path().join(".claude").exists());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn configuration_indirection_is_rejected_before_link_mutation() {
    let repo = fixture();
    let outside = TempDir::new().unwrap();
    create_symlink(outside.path(), &repo.path().join(".vscode")).unwrap();
    assert!(
        settings::build_plan(
            repo.path(),
            &["root=.agents".to_owned()],
            LinkOperation::Link,
            false
        )
        .is_err()
    );
    assert!(!repo.path().join(".github").exists());
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn record_tampering_cannot_remove_another_sources_setting() {
    let repo = fixture();
    let selected = ["root=.agents".to_owned()];
    settings::apply_plan(
        &settings::build_plan(repo.path(), &selected, LinkOperation::Link, false).unwrap(),
    )
    .unwrap();
    let path = repo.path().join(".vscode/install-ctl-guidance-links.json");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace(".github/agents/root", ".github/agents/other"),
    )
    .unwrap();
    assert!(settings::build_plan(repo.path(), &selected, LinkOperation::Unlink, false).is_err());
    assert!(repo.path().join(".github/agents/root").is_dir());
}
