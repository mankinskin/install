//! End-to-end parity fixture: real Cargo builds/installations through both
//! scripted and PTY TUI entry points.  No Cargo subprocess is mocked.

mod support;

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use ratatui_testlib::KeyCode;
use support::bounded_pty::BoundedPty;

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    scripted_root: PathBuf,
    tui_root: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("install")).unwrap();
    fs::write(
        repo.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"bins/tool-one\", \"bins/tool-two\", \"bins/install-ctl\"]\n",
    )
    .unwrap();
    for (package, bin, sentinel) in [
        ("tool-one-package", "tool-one", "one-sentinel"),
        ("tool-two-package", "tool-two", "two-sentinel"),
        ("install-ctl-package", "install-ctl", "self-sentinel"),
    ] {
        let dir = repo.join("bins").join(bin);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                 \n[[bin]]\nname = \"{bin}\"\npath = \"src/main.rs\"\n"
            ),
        )
        .unwrap();
        fs::write(
            dir.join("src/main.rs"),
            format!("fn main() {{ println!(\"{sentinel}\"); }}\n"),
        )
        .unwrap();
    }
    fs::write(
        repo.join("install/artifacts.toml"),
        r#"version = 1

[[artifact]]
id = "tool-one"
category = "cli"
kind = "rust-binary"
source_path = "bins/tool-one"
owner = "fixture"
lifecycle = ["install", "inspect", "uninstall"]
safety = "approval-required"
bin = "tool-one"

[[artifact]]
id = "tool-two"
category = "cli"
kind = "rust-binary"
source_path = "bins/tool-two"
owner = "fixture"
lifecycle = ["install", "inspect", "uninstall"]
safety = "approval-required"
bin = "tool-two"

[[artifact]]
id = "install-ctl"
category = "cli"
kind = "rust-binary"
source_path = "bins/install-ctl"
owner = "fixture"
lifecycle = ["install", "inspect", "uninstall"]
safety = "approval-required"
bin = "install-ctl"
"#,
    )
    .unwrap();
    let fixture = Fixture {
        _temp: temp,
        scripted_root: repo.join("scripted-root"),
        tui_root: repo.join("tui-root"),
        repo,
    };
    generate_lockfile(&fixture);
    fixture
}

fn binary(path_root: &Path, name: &str) -> PathBuf {
    path_root
        .join("bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);

fn spawn_pipe_reader(
    mut pipe: impl Read + Send + 'static,
) -> mpsc::Receiver<Result<Vec<u8>, std::io::Error>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    receiver
}

fn read_pipe_bounded(
    receiver: mpsc::Receiver<Result<Vec<u8>, std::io::Error>>,
    timeout: Duration,
    label: &str,
) -> Vec<u8> {
    receiver
        .recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("{label} output reader did not finish after {timeout:?}"))
        .unwrap_or_else(|error| panic!("{label} output reader failed: {error}"))
}

/// Run an actual child under a wall-clock deadline. On expiry the exact child
/// is killed and reaped before the test fails; stdout and stderr are drained
/// separately so a noisy Cargo child cannot deadlock on either pipe.
fn run_bounded(command: &mut Command, label: &str) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("could not launch {label}: {error}"));
    let stdout = spawn_pipe_reader(child.stdout.take().expect("configured stdout pipe"));
    let stderr = spawn_pipe_reader(child.stderr.take().expect("configured stderr pipe"));
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("could not observe child") {
            break status;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .unwrap_or_else(|error| panic!("could not kill timed-out {label}: {error}"));
            child.wait().expect("could not reap timed-out child");
            // Reaping closes both pipes; wait a short, fixed interval so the
            // two reader threads cannot outlive this failing test unchecked.
            let _ = read_pipe_bounded(stdout, Duration::from_secs(5), label);
            let _ = read_pipe_bounded(stderr, Duration::from_secs(5), label);
            panic!("{label} exceeded {COMMAND_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    Output {
        status,
        stdout: read_pipe_bounded(stdout, remaining, label),
        stderr: read_pipe_bounded(stderr, remaining, label),
    }
}

fn generate_lockfile(fixture: &Fixture) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(&fixture.repo)
        .args(["generate-lockfile", "--offline", "--manifest-path"])
        .arg(fixture.repo.join("Cargo.toml"));
    let output = run_bounded(&mut command, "fixture cargo generate-lockfile");
    assert!(
        output.status.success(),
        "fixture lockfile stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.repo.join("Cargo.lock").is_file());
}

fn run_scripted_output(fixture: &Fixture) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_install-ctl"));
    command
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .args(["install", "tool-one", "--offline", "--locked", "--root"])
        .arg(&fixture.scripted_root);
    run_bounded(&mut command, "scripted install-ctl")
}

fn run_scripted(fixture: &Fixture) -> String {
    let output = run_scripted_output(fixture);
    assert!(
        output.status.success(),
        "scripted stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn artifact_command(fixture: &Fixture, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_install-ctl"));
    command
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .args(args)
        .arg("--root")
        .arg(&fixture.scripted_root);
    run_bounded(&mut command, "artifact status/update")
}

fn successful_stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn artifact_status_defaults_to_all_and_does_not_install_missing_binaries() {
    let fixture = fixture();
    let stdout = successful_stdout(artifact_command(&fixture, &["status"]));
    for id in ["tool-one", "tool-two", "install-ctl"] {
        assert!(stdout.contains(&format!("{id}\tnot-installed")), "{stdout}");
    }
    assert!(!fixture.scripted_root.exists());
    assert!(!fixture.repo.join("target/install-tools").exists());
}

#[test]
fn artifact_status_detects_source_changes_and_update_leaves_missing_tools_alone() {
    let fixture = fixture();
    run_scripted(&fixture);
    let installed = binary(&fixture.scripted_root, "tool-one");
    let original = fs::read(&installed).unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\tup-to-date"), "{stdout}");
    assert!(!stdout.contains("tool-two\t"), "{stdout}");
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "fn main() { println!(\"changed-sentinel\"); }\n",
    )
    .unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status"]));
    assert!(stdout.contains("tool-one\toutdated"), "{stdout}");
    assert_eq!(fs::read(&installed).unwrap(), original);

    let preview = successful_stdout(artifact_command(&fixture, &["--dry-run", "update"]));
    assert!(preview.contains("tool-one"), "{preview}");
    assert_eq!(fs::read(&installed).unwrap(), original);
    successful_stdout(artifact_command(
        &fixture,
        &["update", "--offline", "--locked"],
    ));
    assert_ne!(fs::read(&installed).unwrap(), original);
    assert!(!binary(&fixture.scripted_root, "tool-two").exists());
    assert!(!binary(&fixture.scripted_root, "install-ctl").exists());
    let modified = fs::metadata(&installed).unwrap().modified().unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["update"]));
    assert!(stdout.contains("unchanged"), "{stdout}");
    assert_eq!(
        fs::metadata(&installed).unwrap().modified().unwrap(),
        modified
    );
    let stdout = successful_stdout(artifact_command(&fixture, &["status"]));
    assert!(stdout.contains("tool-one\tup-to-date"), "{stdout}");
}

#[test]
fn artifact_update_empty_selection_is_a_noop_and_unknown_ids_fail() {
    let fixture = fixture();
    successful_stdout(artifact_command(&fixture, &["update"]));
    assert!(!fixture.scripted_root.exists());
    assert!(!fixture.repo.join("target/install-tools").exists());
    for command in ["status", "update"] {
        let output = artifact_command(&fixture, &[command, "no-such-artifact"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unknown artifact"));
    }
}

#[test]
fn artifact_status_accepts_categories_commas_and_deduplicates_ids() {
    let fixture = fixture();
    let stdout = successful_stdout(artifact_command(
        &fixture,
        &["status", "tool-two,tool-one", "cli", "all"],
    ));
    for id in ["tool-one", "tool-two", "install-ctl"] {
        assert_eq!(stdout.matches(&format!("{id}\tnot-installed")).count(), 1);
    }
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-two"]));
    assert!(stdout.contains("tool-two\tnot-installed"), "{stdout}");
    assert!(!stdout.contains("tool-one\t"), "{stdout}");
}

#[test]
fn artifact_status_reports_changed_sources_and_update_propagates_build_failure() {
    let fixture = fixture();
    run_scripted(&fixture);
    let installed = binary(&fixture.scripted_root, "tool-one");
    let original = fs::read(&installed).unwrap();
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "compile_error!(\"fixture-build-failure\");",
    )
    .unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status"]));
    assert!(stdout.contains("tool-one\toutdated"), "{stdout}");
    for command in ["update"] {
        let output = artifact_command(&fixture, &[command]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("fixture-build-failure"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("tool-one\toutdated"));
        assert_eq!(fs::read(&installed).unwrap(), original);
        assert!(!binary(&fixture.scripted_root, "tool-two").exists());
    }
}

#[test]
fn artifact_update_respects_selection_and_status_dry_run_does_not_build() {
    let fixture = fixture();
    run_scripted(&fixture);
    let installed = binary(&fixture.scripted_root, "tool-one");
    let original = fs::read(&installed).unwrap();
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "compile_error!(\"must-not-build\");",
    )
    .unwrap();
    successful_stdout(artifact_command(&fixture, &["--dry-run", "status"]));
    successful_stdout(artifact_command(&fixture, &["update", "tool-two"]));
    assert_eq!(fs::read(&installed).unwrap(), original);
    assert!(!binary(&fixture.scripted_root, "tool-two").exists());
}

#[test]
fn self_update_keeps_the_existing_remote_installer_plan() {
    let output = Command::new(env!("CARGO_BIN_EXE_install-ctl"))
        .args(["--dry-run", "self-update"])
        .output()
        .unwrap();
    let stdout = successful_stdout(output);
    assert!(stdout.contains("git clone --depth 1 https://github.com/mankinskin/workflow-tools"));
    assert!(stdout.contains("cargo install --path"));
    assert!(stdout.contains("--bin install-ctl --force"));
}

#[test]
fn artifact_status_never_builds_and_unchanged_sources_skip_update_builds() {
    let fixture = fixture();
    run_scripted(&fixture);
    fs::remove_dir_all(fixture.repo.join("target/install-tools")).unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\tup-to-date"), "{stdout}");
    assert!(!fixture.repo.join("target/install-tools").exists());
    successful_stdout(artifact_command(&fixture, &["update", "tool-one"]));
    assert!(!fixture.repo.join("target/install-tools").exists());
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "compile_error!(\"status-must-not-compile\");",
    )
    .unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\toutdated"), "{stdout}");
    assert!(!fixture.repo.join("target/install-tools").exists());
}

#[test]
fn artifact_status_defaults_to_the_running_installers_root_not_cargo_home() {
    let fixture = fixture();
    fs::create_dir_all(fixture.scripted_root.join("bin")).unwrap();
    let installed = binary(&fixture.scripted_root, "install-ctl");
    fs::copy(env!("CARGO_BIN_EXE_install-ctl"), &installed).unwrap();
    let output = Command::new(&installed)
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .env("CARGO_HOME", fixture.repo.join("different-cargo-home"))
        .args(["status", "install-ctl"])
        .output()
        .unwrap();
    let stdout = successful_stdout(output);
    assert!(stdout.contains("install-ctl\toutdated"), "{stdout}");
    assert!(!stdout.contains("not-installed"), "{stdout}");
    let output = Command::new(&installed)
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .args(["status", "install-ctl", "--root"])
        .arg(fixture.repo.join("empty-root"))
        .output()
        .unwrap();
    assert!(successful_stdout(output).contains("install-ctl\tnot-installed"));
}

#[test]
fn artifact_fingerprints_track_local_dependencies_but_not_unrelated_packages_or_build_outputs() {
    let fixture = fixture();
    let dependency = fixture.repo.parent().unwrap().join("shared");
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(
        dependency.join("Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        dependency.join("src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .unwrap();
    let manifest = fixture.repo.join("bins/tool-one/Cargo.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[dependencies]\nshared = { path = \"../../../shared\" }\n");
    fs::write(&manifest, text).unwrap();
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "fn main() { println!(\"{}\", shared::value()); }\n",
    )
    .unwrap();
    generate_lockfile(&fixture);
    run_scripted(&fixture);
    fs::write(
        fixture.repo.join("bins/tool-two/src/main.rs"),
        "invalid but unrelated",
    )
    .unwrap();
    fs::create_dir_all(dependency.join("target")).unwrap();
    fs::write(dependency.join("target/generated"), "ignored output").unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\tup-to-date"), "{stdout}");
    fs::write(
        dependency.join("src/lib.rs"),
        "pub fn value() -> u8 { 2 }\n",
    )
    .unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\toutdated"), "{stdout}");
    successful_stdout(artifact_command(&fixture, &["update", "tool-one"]));
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\tup-to-date"), "{stdout}");
}

#[test]
fn artifact_fingerprints_follow_registry_path_patches_without_fetching_dependencies() {
    let fixture = fixture();
    let dependency = fixture.repo.parent().unwrap().join("patched");
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(dependency.join("Cargo.toml"),
        "[package]\nname = \"fingerprint-patch-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
    fs::write(
        dependency.join("src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .unwrap();
    let workspace_manifest = fixture.repo.join("Cargo.toml");
    let mut text = fs::read_to_string(&workspace_manifest).unwrap();
    text.push_str("\n[patch.crates-io]\nfingerprint-patch-fixture = { path = \"../patched\" }\n");
    fs::write(&workspace_manifest, text).unwrap();
    let manifest = fixture.repo.join("bins/tool-one/Cargo.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[dependencies]\nfingerprint-patch-fixture = \"=0.1.0\"\n");
    fs::write(&manifest, text).unwrap();
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "fn main() { println!(\"{}\", fingerprint_patch_fixture::value()); }\n",
    )
    .unwrap();
    generate_lockfile(&fixture);
    run_scripted(&fixture);
    fs::write(
        dependency.join("src/lib.rs"),
        "pub fn value() -> u8 { 2 }\n",
    )
    .unwrap();
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\toutdated")
    );
    successful_stdout(artifact_command(
        &fixture,
        &["update", "tool-one", "--offline"],
    ));
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\tup-to-date")
    );
}

#[test]
fn artifact_fingerprints_detect_binary_tampering_and_missing_or_invalid_receipts() {
    let fixture = fixture();
    run_scripted(&fixture);
    let installed = binary(&fixture.scripted_root, "tool-one");
    let original = fs::read(&installed).unwrap();
    fs::write(&installed, b"tampered").unwrap();
    let stdout = successful_stdout(artifact_command(&fixture, &["status", "tool-one"]));
    assert!(stdout.contains("tool-one\toutdated"), "{stdout}");
    successful_stdout(artifact_command(&fixture, &["update", "tool-one"]));
    assert_eq!(fs::read(&installed).unwrap(), original);
    let receipt = fs::read_dir(fixture.scripted_root.join(".install-ctl"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let valid_receipt = fs::read(&receipt).unwrap();
    fs::write(&receipt, b"invalid json").unwrap();
    let output = artifact_command(&fixture, &["status", "tool-one"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid receipt"));
    let mut future: serde_json::Value = serde_json::from_slice(&valid_receipt).unwrap();
    future["version"] = 99.into();
    fs::write(&receipt, serde_json::to_vec(&future).unwrap()).unwrap();
    let output = artifact_command(&fixture, &["status", "tool-one"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported receipt version"));
    fs::remove_file(&receipt).unwrap();
    let output = artifact_command(&fixture, &["status", "tool-one"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("no source fingerprint"));
    assert!(successful_stdout(output).contains("tool-one\toutdated"));
    fs::write(&receipt, valid_receipt).unwrap();
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\tup-to-date")
    );
}

#[test]
fn artifact_fingerprints_include_lockfile_config_environment_and_file_additions_and_deletions() {
    let fixture = fixture();
    run_scripted(&fixture);
    let lock = fixture.repo.join("Cargo.lock");
    let original = fs::read(&lock).unwrap();
    fs::write(
        &lock,
        format!("{}\n# changed\n", String::from_utf8_lossy(&original)),
    )
    .unwrap();
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\toutdated")
    );
    fs::write(&lock, original).unwrap();
    let asset = fixture.repo.join("bins/tool-one/asset.txt");
    fs::write(&asset, "asset").unwrap();
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\toutdated")
    );
    successful_stdout(artifact_command(&fixture, &["update", "tool-one"]));
    fs::remove_file(asset).unwrap();
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\toutdated")
    );
    successful_stdout(artifact_command(&fixture, &["update", "tool-one"]));
    let output = Command::new(env!("CARGO_BIN_EXE_install-ctl"))
        .env("INSTALL_CTL_REPO_ROOT", &fixture.repo)
        .env("RUSTFLAGS", "--cfg fingerprint_change")
        .args(["status", "tool-one", "--root"])
        .arg(&fixture.scripted_root)
        .output()
        .unwrap();
    assert!(successful_stdout(output).contains("tool-one\toutdated"));
    fs::create_dir_all(fixture.repo.join(".cargo")).unwrap();
    fs::write(
        fixture.repo.join(".cargo/config.toml"),
        "[build]\nrustflags = []\n",
    )
    .unwrap();
    assert!(
        successful_stdout(artifact_command(&fixture, &["status", "tool-one"]))
            .contains("tool-one\toutdated")
    );
}

fn run_tui(fixture: &Fixture) -> String {
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_install-ctl"));
    command.arg("tui");
    command.arg("--offline");
    command.arg("--locked");
    command.arg("--root");
    command.arg(&fixture.tui_root);
    command.env("INSTALL_CTL_REPO_ROOT", &fixture.repo);
    let mut pty = BoundedPty::spawn(command, 100, 30, Duration::from_secs(120)).unwrap();
    pty.wait_for_text("Artifact categories").unwrap();
    pty.send_key(KeyCode::Down).unwrap(); // CLI category
    pty.send_key(KeyCode::Char(' ')).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Artifacts").unwrap();
    pty.send_key(KeyCode::Down).unwrap(); // tool-two
    pty.send_key(KeyCode::Char(' ')).unwrap(); // retain only tool-one
    pty.send_key(KeyCode::Down).unwrap(); // install-ctl
    pty.send_key(KeyCode::Char(' ')).unwrap(); // retain only tool-one
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Review").unwrap();
    pty.wait_for_text("tool-one").unwrap();
    pty.send_key(KeyCode::Char('y')).unwrap();
    pty.wait_for_text("written: tool-one").unwrap();
    let output = pty.screen_contents();
    assert!(pty.wait_exit().unwrap().success());
    pty.finish().unwrap();
    output
}

fn run_tui_self_with_root_edit(fixture: &Fixture) -> String {
    let initial_root = fixture.repo.join("different-initial-root");
    let edited_root = fixture.repo.join("edited-root");
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_install-ctl"));
    command.args(["tui", "--offline", "--locked", "--root"]);
    command.arg(&initial_root);
    command.env("INSTALL_CTL_REPO_ROOT", &fixture.repo);
    let mut pty = BoundedPty::spawn(command, 100, 30, Duration::from_secs(120)).unwrap();
    pty.wait_for_text("Artifact categories").unwrap();
    pty.send_key(KeyCode::Down).unwrap();
    pty.send_key(KeyCode::Char(' ')).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Artifacts").unwrap();
    pty.send_key(KeyCode::Char(' ')).unwrap(); // deselect tool-one
    pty.send_key(KeyCode::Down).unwrap();
    pty.send_key(KeyCode::Char(' ')).unwrap(); // deselect tool-two
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Review").unwrap();
    pty.send_key(KeyCode::Char('r')).unwrap();
    pty.wait_for_text("Binary installation root").unwrap();
    for _ in 0..initial_root.display().to_string().len() {
        pty.send_key(KeyCode::Backspace).unwrap();
    }
    pty.send_keys(&edited_root.display().to_string()).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Review").unwrap();
    pty.send_key(KeyCode::Char('y')).unwrap();
    pty.wait_for_text("written: install-ctl").unwrap();
    let output = pty.screen_contents();
    pty.finish().unwrap();
    assert!(!binary(&initial_root, "install-ctl").exists());
    let mut installed = Command::new(binary(&edited_root, "install-ctl"));
    let result = run_bounded(&mut installed, "edited-root self fixture");
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        "self-sentinel"
    );
    output
}

#[test]
fn scripted_and_tui_apply_same_real_binary_and_report_changed_write_then_unchanged() {
    let fixture = fixture();

    let scripted_first = run_scripted(&fixture);
    let tui_first = run_tui(&fixture);
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "fn main() { println!(\"changed-sentinel\"); }\n",
    )
    .unwrap();
    let scripted_changed = run_scripted(&fixture);
    let scripted_unchanged = run_scripted(&fixture);

    assert!(scripted_first.contains("written: tool-one"));
    assert!(tui_first.contains("written: tool-one"));
    assert!(scripted_changed.contains("written: tool-one"));
    assert!(scripted_unchanged.contains("unchanged: tool-one"));
    assert!(!binary(&fixture.scripted_root, "tool-two").exists());
    assert!(!binary(&fixture.tui_root, "tool-two").exists());
    for (root, sentinel) in [
        (&fixture.scripted_root, "changed-sentinel"),
        (&fixture.tui_root, "one-sentinel"),
    ] {
        let mut command = Command::new(binary(root, "tool-one"));
        let output = run_bounded(&mut command, "installed fixture binary");
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), sentinel);
    }
}

#[test]
fn tui_shadows_before_interaction_and_applies_self_selection_after_root_edit() {
    let fixture = fixture();
    let output = run_tui_self_with_root_edit(&fixture);

    assert!(output.contains("written: install-ctl"));
}

#[test]
fn warmup_compile_failure_renders_failed_action_before_nonzero_exit() {
    let fixture = fixture();
    fs::write(
        fixture.repo.join("bins/tool-one/src/main.rs"),
        "fn main( { this is intentionally invalid Rust }\n",
    )
    .unwrap();

    let output = run_scripted_output(&fixture);
    let rendered = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !output.status.success(),
        "invalid fixture source unexpectedly installed"
    );
    assert!(
        rendered.contains("failed: tool-one"),
        "failure must be rendered as an action before nonzero exit; output={rendered}"
    );
}
