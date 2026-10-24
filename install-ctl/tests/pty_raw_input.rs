#![cfg(unix)]

#[expect(
    dead_code,
    reason = "Shared PTY support also exposes helpers used by sibling integration targets."
)]
mod support;

use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyCode as DecodedCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use portable_pty::CommandBuilder;
use ratatui_testlib::KeyCode;
use support::bounded_pty::BoundedPty;

const CHILD_CASE: &str = "INSTALL_CTL_PTY_RAW_INPUT_CASE";
const TIMEOUT: Duration = Duration::from_secs(5);

struct RawMode;

impl Drop for RawMode {
    fn drop(&mut self) {
        disable_raw_mode().expect("restore child terminal mode");
    }
}

fn check_raw_input(name: &str, input: &[KeyCode], expected: &[KeyEvent]) {
    if std::env::var(CHILD_CASE).as_deref() == Ok(name) {
        enable_raw_mode().expect("enable real child terminal raw mode");
        let _raw_mode = RawMode;
        println!("RAW_INPUT_READY");
        io::stdout().flush().unwrap();

        let deadline = Instant::now() + TIMEOUT;
        let mut actual = Vec::new();
        while actual.len() < expected.len() {
            assert!(
                event::poll(deadline.saturating_duration_since(Instant::now())).unwrap(),
                "raw input deadline; decoded so far: {actual:?}"
            );
            if let Event::Key(key) = event::read().unwrap() {
                actual.push(key);
            }
        }
        assert_eq!(actual, expected, "actual crossterm raw-mode events");
        println!("RAW_INPUT_VERIFIED");
        io::stdout().flush().unwrap();
        return;
    }

    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args(["--exact", name, "--nocapture"]);
    command.env(CHILD_CASE, name);
    let mut pty = BoundedPty::spawn(command, 100, 30, TIMEOUT).unwrap();
    let result = (|| {
        pty.wait_for_text("RAW_INPUT_READY")?;
        for key in input {
            pty.send_key(*key)?;
        }
        let status = pty.wait_exit()?;
        if status.success() {
            pty.wait_for_text("RAW_INPUT_VERIFIED")?;
        }
        pty.repaint()?;
        Ok::<_, String>(status)
    })();
    let output = pty.screen_contents();
    let cleanup = pty.finish();
    assert!(cleanup.is_ok(), "PTY cleanup: {cleanup:?}; {output}");
    let status = result.unwrap_or_else(|error| panic!("{error}; child output: {output}"));
    assert!(status.success(), "child status: {status:?}; {output}");
    assert!(output.contains("RAW_INPUT_VERIFIED"), "{output}");
}

#[test]
fn raw_enter_decodes_as_enter() {
    check_raw_input(
        "raw_enter_decodes_as_enter",
        &[KeyCode::Enter],
        &[KeyEvent::new(DecodedCode::Enter, KeyModifiers::NONE)],
    );
}

#[test]
fn raw_arrows_and_characters_preserve_order() {
    check_raw_input(
        "raw_arrows_and_characters_preserve_order",
        &[
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('a'),
            KeyCode::Char('7'),
        ],
        &[
            KeyEvent::new(DecodedCode::Up, KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Down, KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Left, KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Right, KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Char('a'), KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Char('7'), KeyModifiers::NONE),
        ],
    );
}

#[test]
fn raw_literal_lf_remains_control_j() {
    check_raw_input(
        "raw_literal_lf_remains_control_j",
        &[KeyCode::Char('\n')],
        &[KeyEvent::new(DecodedCode::Char('j'), KeyModifiers::CONTROL)],
    );
}

#[test]
fn raw_enter_and_literal_lf_remain_distinct_in_sequence() {
    check_raw_input(
        "raw_enter_and_literal_lf_remain_distinct_in_sequence",
        &[
            KeyCode::Char('a'),
            KeyCode::Enter,
            KeyCode::Char('\n'),
            KeyCode::Enter,
        ],
        &[
            KeyEvent::new(DecodedCode::Char('a'), KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Enter, KeyModifiers::NONE),
            KeyEvent::new(DecodedCode::Char('j'), KeyModifiers::CONTROL),
            KeyEvent::new(DecodedCode::Enter, KeyModifiers::NONE),
        ],
    );
}
