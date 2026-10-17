#[allow(dead_code)]
mod support;

use std::{fs, path::PathBuf, process::Command, time::Duration};

use ratatui_testlib::KeyCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::bounded_pty::BoundedPty;

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    bundle: PathBuf,
    manifest: PathBuf,
    home: PathBuf,
    bytes: Vec<u8>,
    data: Value,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let bundle = temp.path().join("bundle");
        fs::create_dir_all(repo.join("install")).unwrap();
        fs::create_dir_all(&bundle).unwrap();
        let mut registry = "version = 1\n".to_string();
        let bytes = fs::read(env!("CARGO_BIN_EXE_install-ctl")).unwrap();
        let mut tools = Vec::new();
        for id in ["one", "two"] {
            registry.push_str(&format!(
                "\n[[artifact]]\nid = \"{id}\"\ncategory = \"cli\"\nkind = \"rust-binary\"\n\
                 source_path = \"missing/{id}\"\nowner = \"fixture\"\n\
                 lifecycle = [\"install\", \"inspect\"]\nsafety = \"approval-required\"\nbin = \"{id}\"\n"
            ));
            let mut delivered = bytes.clone();
            // Distinct hashes make second-tool corrupt-cache coverage order-sensitive.
            if id == "two" {
                delivered.push(0);
            }
            fs::write(bundle.join(id), &delivered).unwrap();
            tools.push(json!({"id": id, "path": id, "sha256": format!("{:x}", Sha256::digest(&delivered))}));
        }
        fs::write(repo.join("install/artifacts.toml"), registry).unwrap();
        let data = json!({
            "version": 1,
            "target": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "tools": tools
        });
        let fixture = Self {
            manifest: bundle.join("manifest.json"),
            home: temp.path().join("home"),
            _temp: temp,
            repo,
            bundle,
            bytes,
            data,
        };
        fixture.save();
        fixture
    }

    fn save(&self) {
        fs::write(&self.manifest, serde_json::to_vec(&self.data).unwrap()).unwrap();
    }

    fn run(&self, selection: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_install-ctl"))
            .arg("prebuilt-install")
            .args(selection)
            .arg("--bundle")
            .arg(&self.bundle)
            .arg("--manifest")
            .arg(&self.manifest)
            .arg("--home")
            .arg(&self.home)
            .env("INSTALL_CTL_REPO_ROOT", &self.repo)
            // No source workspace and no Cargo available: fallback cannot pass.
            .env("PATH", "")
            .output()
            .unwrap()
    }

    fn binary(&self, id: &str) -> PathBuf {
        self.home
            .join(".workflow-tools/bin")
            .join(format!("{id}{}", std::env::consts::EXE_SUFFIX))
    }

    fn assert_rejected_without_writes(&self, selection: &[&str], error: &str) {
        let result = self.run(selection);
        assert!(!result.status.success());
        assert!(output_text(&result).contains(error), "{result:?}");
        assert!(!self.home.exists());
    }
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn delivered_executable_installs_and_verified_cache_is_reused_without_cargo() {
    let fixture = Fixture::new();
    assert!(fixture.run(&["one"]).status.success());
    assert_eq!(fs::read(fixture.binary("one")).unwrap(), fixture.bytes);
    let cached = fixture
        .home
        .join(".workflow-tools/cache")
        .join(fixture.data["tools"][0]["sha256"].as_str().unwrap());
    let modified = fs::metadata(&cached).unwrap().modified().unwrap();
    assert_eq!(fs::read(&cached).unwrap(), fixture.bytes);
    fs::remove_file(fixture.binary("one")).unwrap();
    assert!(fixture.run(&["one"]).status.success());
    assert_eq!(fs::metadata(&cached).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read(fixture.binary("one")).unwrap(), fixture.bytes);
    let help = Command::new(fixture.binary("one"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("prebuilt-install"));
}

#[test]
fn second_corrupt_cache_prevents_all_destination_and_cache_writes() {
    let fixture = Fixture::new();
    let cache = fixture.home.join(".workflow-tools/cache");
    fs::create_dir_all(&cache).unwrap();
    let corrupt = cache.join(fixture.data["tools"][1]["sha256"].as_str().unwrap());
    fs::write(&corrupt, "corrupt").unwrap();
    let result = fixture.run(&["one", "two"]);
    assert!(!result.status.success());
    assert!(output_text(&result).contains("cached SHA-256 mismatch for two"));
    assert!(!fixture.binary("one").exists());
    assert!(!fixture.binary("two").exists());
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
    assert_eq!(fs::read(&corrupt).unwrap(), b"corrupt");
}

#[test]
fn invalid_hash_target_version_path_and_missing_source_do_not_create_home() {
    for (field, value, expected) in [
        ("sha256", json!("0".repeat(64)), "SHA-256 mismatch"),
        ("sha256", json!("invalid"), "invalid SHA-256"),
        ("path", json!("../outside"), "unsafe bundle path"),
        ("path", json!("absent"), "missing bundle file"),
        ("path", json!("."), "regular in-bundle file"),
    ] {
        let mut fixture = Fixture::new();
        fixture.data["tools"][0][field] = value;
        fixture.save();
        fixture.assert_rejected_without_writes(&["one"], expected);
    }
    for (field, value, expected) in [
        ("target", json!("wrong-platform"), "does not match"),
        ("version", json!(2), "unsupported prebuilt manifest version"),
    ] {
        let mut fixture = Fixture::new();
        fixture.data[field] = value;
        fixture.save();
        fixture.assert_rejected_without_writes(&["one"], expected);
    }
}

#[test]
fn duplicate_and_unknown_ids_are_rejected_before_writes() {
    let mut fixture = Fixture::new();
    fixture.assert_rejected_without_writes(&["unknown"], "unknown artifact");
    let duplicate = fixture.data["tools"][0].clone();
    fixture.data["tools"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    fixture.save();
    fixture.assert_rejected_without_writes(&["one"], "duplicate manifest tool");
}

#[test]
fn native_pty_review_shows_prebuilt_source_and_applies_reviewed_destination() {
    let fixture = Fixture::new();
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_install-ctl"));
    command.arg("tui");
    for (option, path) in [
        ("--prebuilt-bundle", &fixture.bundle),
        ("--prebuilt-manifest", &fixture.manifest),
        ("--prebuilt-home", &fixture.home),
    ] {
        command.arg(option);
        command.arg(path);
    }
    command.env("INSTALL_CTL_REPO_ROOT", &fixture.repo);
    let mut pty = BoundedPty::spawn(command, 180, 40, Duration::from_secs(30)).unwrap();
    pty.wait_for_text("Artifact categories").unwrap();
    pty.send_key(KeyCode::Down).unwrap();
    pty.send_key(KeyCode::Char(' ')).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Artifacts").unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Review").unwrap();
    pty.wait_for_text("local prebuilt").unwrap();
    assert!(!fixture.home.exists(), "review must be read-only");
    let edited_root = fixture._temp.path().join("edited-tools");
    pty.send_key(KeyCode::Char('r')).unwrap();
    pty.wait_for_text("Enter save").unwrap();
    for _ in fixture
        .home
        .join(".workflow-tools")
        .display()
        .to_string()
        .chars()
    {
        pty.send_key(KeyCode::Backspace).unwrap();
    }
    pty.send_keys(edited_root.to_str().unwrap()).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Review").unwrap();
    pty.send_key(KeyCode::Char('y')).unwrap();
    pty.wait_for_text("written: one").unwrap();
    let installed = edited_root
        .join("bin")
        .join(format!("one{}", std::env::consts::EXE_SUFFIX));
    pty.wait_for_path(&installed).unwrap();
    assert!(pty.wait_exit().unwrap().success());
    pty.finish().unwrap();
    assert!(
        !fixture.home.exists(),
        "apply must respect the edited review root"
    );
    assert_eq!(fs::read(&installed).unwrap(), fixture.bytes);
    assert!(
        Command::new(installed)
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn dry_run_verifies_but_does_not_create_home() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_install-ctl"))
        .arg("--dry-run")
        .arg("prebuilt-install")
        .arg("one")
        .arg("--bundle")
        .arg(&fixture.bundle)
        .arg("--manifest")
        .arg(&fixture.manifest)
        .arg("--home")
        .arg(&fixture.home)
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output_text(&output).contains("dry-run, no writes"));
    assert!(!fixture.home.exists());
}

#[test]
fn late_source_mismatch_leaves_existing_destination_untouched() {
    let mut fixture = Fixture::new();
    let binary = fixture.binary("one");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, "original").unwrap();
    fixture.data["tools"][1]["sha256"] = json!("0".repeat(64));
    fixture.save();
    let output = fixture.run(&["one", "two"]);
    assert!(!output.status.success());
    assert!(output_text(&output).contains("SHA-256 mismatch for two"));
    assert_eq!(fs::read(binary).unwrap(), b"original");
    assert!(!fixture.home.join(".workflow-tools/cache").exists());
}

#[test]
fn scripted_self_replacement_is_rejected_before_cache_writes() {
    let fixture = Fixture::new();
    let running = fixture.binary("one");
    fs::create_dir_all(running.parent().unwrap()).unwrap();
    fs::write(&running, &fixture.bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&running, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let result = Command::new(&running)
        .arg("prebuilt-install")
        .arg("one")
        .arg("--bundle")
        .arg(&fixture.bundle)
        .arg("--manifest")
        .arg(&fixture.manifest)
        .arg("--home")
        .arg(&fixture.home)
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(output_text(&result).contains("cannot replace running installer"));
    assert_eq!(fs::read(running).unwrap(), fixture.bytes);
    assert!(!fixture.home.join(".workflow-tools/cache").exists());
}

#[test]
fn native_pty_cancel_leaves_prebuilt_home_absent() {
    let fixture = Fixture::new();
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_install-ctl"));
    command.arg("tui");
    for (option, path) in [
        ("--prebuilt-bundle", &fixture.bundle),
        ("--prebuilt-manifest", &fixture.manifest),
        ("--prebuilt-home", &fixture.home),
    ] {
        command.arg(option);
        command.arg(path);
    }
    command.env("INSTALL_CTL_REPO_ROOT", &fixture.repo);
    let mut pty = BoundedPty::spawn(command, 180, 40, Duration::from_secs(30)).unwrap();
    pty.wait_for_text("Artifact categories").unwrap();
    pty.send_key(KeyCode::Down).unwrap();
    pty.send_key(KeyCode::Char(' ')).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Artifacts").unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("local prebuilt").unwrap();
    pty.send_key(KeyCode::Char('n')).unwrap();
    pty.wait_for_text("cancelled; no writes").unwrap();
    assert!(pty.wait_exit().unwrap().success());
    pty.finish().unwrap();
    assert!(!fixture.home.exists());
}
