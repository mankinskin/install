use std::{fs, path::Path, process::Command};

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn mission_registry_has_exact_transport_metadata() {
    let registry: toml::Value =
        toml::from_str(&fs::read_to_string(repo_root().join("install/artifacts.toml")).unwrap())
            .unwrap();
    let entries = registry["artifact"].as_array().unwrap();
    let mission_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry["id"].as_str().unwrap().starts_with("mission-"))
        .collect();
    assert_eq!(mission_entries.len(), 2);
    for (id, category, bin, feature) in [
        ("mission-cli", "cli", "mission", "cli"),
        ("mission-mcp", "mcp", "mission-mcp", "mcp"),
    ] {
        let matches: Vec<_> = mission_entries
            .iter()
            .filter(|entry| entry["id"].as_str() == Some(id))
            .collect();
        assert_eq!(matches.len(), 1, "{id}");
        let entry = matches[0];
        assert_eq!(entry["category"].as_str(), Some(category));
        assert_eq!(
            entry["source_path"].as_str(),
            Some("mission/crates/mission")
        );
        assert_eq!(entry["bin"].as_str(), Some(bin));
        assert_eq!(entry["features"].as_array().unwrap().len(), 1);
        assert_eq!(entry["features"][0].as_str(), Some(feature));
    }
}

#[test]
fn mission_artifacts_support_separate_and_combined_selection() {
    for selection in [
        vec!["mission-cli"],
        vec!["mission-mcp"],
        vec!["mission-cli", "mission-mcp"],
    ] {
        let target = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_install-ctl"))
            .args(["--dry-run", "install", "--root"])
            .arg(target.path())
            .args(&selection)
            .env("INSTALL_CTL_REPO_ROOT", repo_root())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{selection:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
    }
}
