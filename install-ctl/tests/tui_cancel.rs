//! The production terminal is driven through a real PTY, not a mocked event
//! source.  This is deliberately a small smoke test: it proves cancellation
//! cannot reach the installer apply path.

use std::fs;

mod support;

use ratatui_testlib::KeyCode;
use support::bounded_pty::BoundedPty;

#[test]
fn escape_cancels_before_any_target_write() {
    let root = tempfile::tempdir().unwrap();
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_install-ctl"));
    command.arg("tui");
    command.arg("--root");
    command.arg(root.path());
    command.env(
        "INSTALL_CTL_REPO_ROOT",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap(),
    );
    let mut harness =
        BoundedPty::spawn(command, 100, 30, std::time::Duration::from_secs(30)).unwrap();
    if let Err(error) = harness.wait_for_text("Artifact categories") {
        panic!(
            "TUI did not render its bounded startup marker: {error}; screen={:?}",
            harness.screen_contents()
        );
    }
    harness.send_key(KeyCode::Esc).unwrap();
    if let Err(error) = harness.wait_for_text("cancelled; no writes performed") {
        panic!(
            "TUI did not report cancellation: {error}; screen={:?}",
            harness.screen_contents()
        );
    }
    assert!(
        fs::read_dir(root.path()).unwrap().next().is_none(),
        "cancelled TUI must not write the target root"
    );
    harness.finish().unwrap();
}
