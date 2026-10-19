//! Real PTY coverage for the composed guidance review.  Pure state-model
//! tests remain separate; this drives the compiled install-ctl binary.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

mod support;

use ratatui_testlib::KeyCode;
use support::bounded_pty::BoundedPty;

struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    profile: PathBuf,
    guidance_target: PathBuf,
    binary_root: PathBuf,
}

fn fixture(scope: &str, count: usize) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let profile = temp.path().join("profile.toml");
    let guidance_target = temp.path().join("guidance-target");
    let binary_root = temp.path().join("binary-root");
    let mut paths = Vec::new();
    for index in 0..count {
        let relative = format!(".agents/items/{index:02}.md");
        let source_path = source.join(&relative);
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(source_path, format!("fixture guidance {index}\n")).unwrap();
        paths.push(format!("\"{relative}\""));
    }
    let explicit = if scope == "explicit" {
        "path = \"../escape\"\n"
    } else {
        ""
    };
    fs::write(
        &profile,
        format!(
            "[profile]\nid = \"pty-profile\"\n\n[[corpus]]\nid = \"root\"\npaths = [{}]\n\n[destination]\nscope = \"{scope}\"\n{explicit}",
            paths.join(", ")
        ),
    )
    .unwrap();
    Fixture {
        _temp: temp,
        source,
        profile,
        guidance_target,
        binary_root,
    }
}

fn spawn(fixture: &Fixture) -> BoundedPty {
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_install-ctl"));
    command.arg("tui");
    command.arg("--root");
    command.arg(&fixture.binary_root);
    command.arg("--guidance-source");
    command.arg(&fixture.source);
    command.arg("--guidance-profile");
    command.arg(&fixture.profile);
    command.arg("--guidance-select");
    command.arg("root");
    command.arg("--guidance-target");
    command.arg(&fixture.guidance_target);
    command.env(
        "INSTALL_CTL_REPO_ROOT",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap(),
    );
    let mut harness = BoundedPty::spawn(command, 100, 12, Duration::from_secs(30)).unwrap();
    harness.wait_for_text("Artifact categories").unwrap();
    harness
}

fn review_guidance_only(harness: &mut BoundedPty) {
    harness.send_key(KeyCode::Enter).unwrap(); // Categories -> Artifacts
    harness.wait_for_text("Artifacts").unwrap();
    harness.send_key(KeyCode::Enter).unwrap(); // Artifacts -> Guidance
    harness.wait_for_text("Guidance profiles").unwrap();
    harness.send_key(KeyCode::Char(' ')).unwrap();
    harness.send_key(KeyCode::Enter).unwrap();
    harness.wait_for_text("Review").unwrap();
}

fn cancel_and_wait(harness: &mut BoundedPty) {
    harness.send_key(KeyCode::Char('n')).unwrap();
    assert!(harness.wait_exit().unwrap().success());
}

fn rendered_guidance_destination(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn scroll_until(harness: &mut BoundedPty, text: &str) {
    // Reset to the top first, then search downward. This makes assertions
    // independent of the previous metadata assertion's review-scroll offset.
    for _ in 0..64 {
        harness.send_key(KeyCode::Up).unwrap();
    }
    for _ in 0..64 {
        harness.repaint().unwrap();
        if harness.screen_contents().contains(text) {
            return;
        }
        harness.send_key(KeyCode::Down).unwrap();
    }
    panic!(
        "review never exposed {text:?} after bounded scrolling; screen={:?}",
        harness.screen_contents()
    );
}

#[test]
fn guidance_review_shows_plan_before_confirm_and_root_edit_does_not_change_guidance_target() {
    let fixture = fixture("repo", 16);
    let mut harness = spawn(&fixture);
    review_guidance_only(&mut harness);

    scroll_until(&mut harness, "pty-profile");
    scroll_until(
        &mut harness,
        &rendered_guidance_destination(&fixture.guidance_target),
    );
    scroll_until(&mut harness, ".agents/items/00.md");

    harness.send_key(KeyCode::Char('r')).unwrap();
    // At this height the terminal parser may wrap the title's long leading
    // words, but the editable prompt suffix remains contiguous.
    harness.wait_for_text("root — type path").unwrap();
    for _ in fixture.binary_root.display().to_string().chars() {
        harness.send_key(KeyCode::Backspace).unwrap();
    }
    let edited_root = fixture._temp.path().join("edited-binary-root");
    harness
        .send_keys(&edited_root.display().to_string())
        .unwrap();
    harness.send_key(KeyCode::Enter).unwrap();
    harness.wait_for_text("Review — Enter/y install").unwrap();
    scroll_until(&mut harness, &edited_root.display().to_string());
    scroll_until(
        &mut harness,
        &rendered_guidance_destination(&fixture.guidance_target),
    );

    scroll_until(&mut harness, ".agents/items/15.md");
    cancel_and_wait(&mut harness);
    assert!(!edited_root.exists());
    assert!(!fixture.guidance_target.exists());
}

#[test]
fn guidance_only_confirm_writes_fixture_through_real_tui_without_binary_install() {
    let fixture = fixture("repo", 1);
    let mut harness = spawn(&fixture);
    review_guidance_only(&mut harness);
    harness.send_key(KeyCode::Char('y')).unwrap();
    harness
        .wait_for_path(&fixture.guidance_target.join(".agents/items/00.md"))
        .unwrap();
    assert!(harness.wait_exit().unwrap().success());

    assert_eq!(
        fs::read_to_string(fixture.guidance_target.join(".agents/items/00.md")).unwrap(),
        "fixture guidance 0\n"
    );
    assert!(
        !fixture.binary_root.exists(),
        "guidance-only confirmation must not install binaries"
    );
}

#[test]
fn blocking_guidance_review_rejects_confirmation_then_cancels_without_writes() {
    let fixture = fixture("explicit", 1);
    let mut harness = spawn(&fixture);
    review_guidance_only(&mut harness);
    harness.send_key(KeyCode::Char('y')).unwrap();
    harness
        .wait_for_text("review has blocking diagnostics")
        .unwrap();
    cancel_and_wait(&mut harness);
    assert!(!fixture.guidance_target.exists());
    assert!(!fixture.binary_root.exists());
}
