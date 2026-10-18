use super::*;

fn sources(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn deterministic_named_sources_use_actual_destination_parent_without_writes() {
    let repo = fixture();
    fs::create_dir_all(repo.path().join("nested/.agents/agents")).unwrap();
    let selected = sources(&["zeta=nested/.agents", "root=.agents"]);
    let result =
        super::super::vscode::build_plan(repo.path(), &selected, LinkOperation::Link, false)
            .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let destinations: Vec<_> = result
        .entries
        .iter()
        .map(|entry| entry.destination.as_str())
        .collect();
    assert_eq!(
        destinations,
        [
            ".github/agents/root",
            ".github/instructions/root",
            ".github/prompts/root",
            ".github/agents/zeta",
        ]
    );
    assert_eq!(
        result.entries[0].target,
        PathBuf::from("../../.agents/agents")
    );
    assert_eq!(
        result.entries[3].target,
        PathBuf::from("../../nested/.agents/agents")
    );
    assert!(!repo.path().join(".github").exists());
    assert!(!repo.path().join(".vscode").exists());
}

#[test]
fn invalid_names_and_traversal_are_rejected() {
    let repo = fixture();
    for value in [
        "=.agents",
        "../root=.agents",
        "Root=.agents",
        "con=.agents",
        "root=../outside",
        "root=",
        "root",
        "root=C:\\outside",
    ] {
        let result = super::super::vscode::build_plan(
            repo.path(),
            &sources(&[value]),
            LinkOperation::Link,
            false,
        );
        assert!(
            result.is_err() || !result.unwrap().diagnostics.is_empty(),
            "{value}"
        );
    }
}

#[test]
fn missing_explicit_sources_and_duplicate_names_do_not_succeed() {
    let repo = fixture();
    for selected in [
        sources(&[]),
        sources(&["root=.agents", "root=.agents"]),
        sources(&["absent=not-present"]),
    ] {
        let result =
            super::super::vscode::build_plan(repo.path(), &selected, LinkOperation::Link, false);
        assert!(result.is_err() || !result.unwrap().diagnostics.is_empty());
    }
}

#[test]
fn absent_individual_kinds_are_explicitly_skipped() {
    let repo = TempDir::new().unwrap();
    fs::create_dir_all(repo.path().join(".agents/prompts")).unwrap();
    let result = super::super::vscode::build_plan(
        repo.path(),
        &sources(&["root=.agents"]),
        LinkOperation::Link,
        false,
    )
    .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].destination, ".github/prompts/root");
    assert_eq!(result.skipped_sources.len(), 2);
}
