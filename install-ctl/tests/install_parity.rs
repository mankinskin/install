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
