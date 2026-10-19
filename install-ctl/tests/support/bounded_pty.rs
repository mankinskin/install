//! Bounded real-PTY test adapter.
//!
//! `ratatui-testlib` owns the literal VT parser (`ScreenState`), while this
//! adapter replaces only its blocking ConPTY read loop with one owned reader
//! thread and channel-backed deadlines.

use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use portable_pty::{Child, CommandBuilder, ExitStatus, PtyPair, PtySize};
use ratatui_testlib::{KeyCode, KeyEvent};

use super::terminal_parser::TerminalScreen;

const RAW_CAPTURE_CAP: usize = 64 * 1024;

pub struct BoundedPty {
    pair: Option<PtyPair>,
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Option<Box<dyn Write + Send>>,
    output: Receiver<Result<Vec<u8>, String>>,
    reader: Option<JoinHandle<()>>,
    screen: TerminalScreen,
    raw_tail: Vec<u8>,
    width: u16,
    height: u16,
    timeout: Duration,
}

impl BoundedPty {
    pub fn spawn(
        command: CommandBuilder,
        width: u16,
        height: u16,
        timeout: Duration,
    ) -> Result<Self, String> {
        let system = portable_pty::native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| error.to_string())?;
        let (sender, output) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut buffer = vec![0; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if sender.send(Ok(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            pair: Some(pair),
            child: Some(child),
            writer: Some(writer),
            output,
            reader: Some(reader_thread),
            screen: TerminalScreen::new(width, height),
            raw_tail: Vec::new(),
            width,
            height,
            timeout: diagnostic_timeout(timeout),
        })
    }

    pub fn send_key(&mut self, key: KeyCode) -> Result<(), String> {
        let bytes = KeyEvent::new(key).to_bytes();
        self.writer
            .as_mut()
            .ok_or_else(|| "PTY writer is unavailable".to_string())?
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        self.writer
            .as_mut()
            .unwrap()
            .flush()
            .map_err(|error| error.to_string())?;
        self.wait_for_repaint(Instant::now() + Duration::from_millis(250))
    }

    pub fn send_keys(&mut self, text: &str) -> Result<(), String> {
        for character in text.chars() {
            self.send_key(KeyCode::Char(character))?;
        }
        Ok(())
    }

    pub fn wait_for_text(&mut self, text: &str) -> Result<(), String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if self.screen.contains(text) || self.raw_tail_contains(text) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let child = self
                    .child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten());
                return Err(format!(
                    "timeout waiting for {text:?}; pty={}x{} child={child:?}; screen={:?}; raw_tail={}",
                    self.width,
                    self.height,
                    self.screen.contents(),
                    self.escaped_raw_tail(),
                ));
            }
            self.receive_until(deadline)?;
        }
    }

    /// Wait for a child process (including a shadow-relaunched child) to
    /// produce its promised filesystem result. The PTY's direct child may
    /// have already exited after handing its terminal to that shadow child.
    pub fn wait_for_path(&mut self, path: &Path) -> Result<(), String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if path.exists() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "timeout waiting for path {}; pty={}x{} child={:?}; screen={:?}; raw_tail={}",
                    path.display(),
                    self.width,
                    self.height,
                    self.child
                        .as_mut()
                        .and_then(|child| child.try_wait().ok().flatten()),
                    self.screen.contents(),
                    self.escaped_raw_tail(),
                ));
            }
            self.receive_until(deadline)?;
        }
    }

    pub fn repaint(&mut self) -> Result<(), String> {
        self.drain_repaint()
    }

    pub fn screen_contents(&self) -> String {
        self.screen.contents()
    }

    pub fn wait_exit(&mut self) -> Result<ExitStatus, String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .ok_or_else(|| "PTY child is unavailable".to_string())?
                .try_wait()
                .map_err(|error| error.to_string())?
            {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("timeout waiting for PTY child exit".into());
            }
            self.receive_until(deadline)?;
        }
    }

    fn receive_until(&mut self, deadline: Instant) -> Result<(), String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self
            .output
            .recv_timeout(remaining.min(Duration::from_millis(25)))
        {
            Ok(Ok(bytes)) => self.feed(&bytes),
            Ok(Err(error)) => return Err(format!("PTY reader failed: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        self.drain_repaint()
    }

    fn wait_for_repaint(&mut self, deadline: Instant) -> Result<(), String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.output.recv_timeout(remaining) {
            Ok(Ok(bytes)) => self.feed(&bytes),
            Ok(Err(error)) => return Err(format!("PTY reader failed: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                return Ok(());
            }
        }
        self.drain_repaint()
    }

    fn drain_repaint(&mut self) -> Result<(), String> {
        loop {
            match self.output.try_recv() {
                Ok(Ok(bytes)) => self.feed(&bytes),
                Ok(Err(error)) => return Err(format!("PTY reader failed: {error}")),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.screen.feed(bytes);
        self.raw_tail.extend_from_slice(bytes);
        if self.raw_tail.len() > RAW_CAPTURE_CAP {
            let excess = self.raw_tail.len() - RAW_CAPTURE_CAP;
            self.raw_tail.drain(..excess);
        }
    }

    fn escaped_raw_tail(&self) -> String {
        self.raw_tail
            .iter()
            .flat_map(|byte| std::ascii::escape_default(*byte))
            .map(char::from)
            .collect()
    }

    fn raw_tail_contains(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.raw_tail).contains(text)
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if let Some(child) = self.child.as_mut() {
            if child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
            {
                child.kill().map_err(|error| error.to_string())?;
            }
        }
        let deadline = Instant::now() + self.timeout;
        while self
            .reader
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
            && Instant::now() < deadline
        {
            self.pair.take();
            self.writer.take();
            self.receive_until(deadline)?;
        }
        if self
            .reader
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
        {
            return Err("PTY reader did not stop after child termination".into());
        }
        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .map_err(|_| "PTY reader panicked".to_string())?;
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
        Ok(())
    }
}

fn diagnostic_timeout(default: Duration) -> Duration {
    std::env::var("INSTALL_CTL_PTY_DIAGNOSTIC_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}

impl Drop for BoundedPty {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
