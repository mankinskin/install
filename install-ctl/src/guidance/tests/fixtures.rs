use super::*;

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
