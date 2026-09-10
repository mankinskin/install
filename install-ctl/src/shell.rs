//! Subprocess helpers (ported from viewer-ctl).
//!
//! On Windows, `.cmd`/`.bat` shims (npm, vsce, trunk, …) are routed through
//! `cmd /C` because [`std::process::Command`] cannot launch them directly.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

const MAX_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_LINES: usize = 120;

/// Run a command from string slices.
pub fn run_cmd_args(program: &str, args: &[&str], cwd: &Path, tag: &str) -> Result<(), String> {
    let owned: Vec<String> = std::iter::once(program.to_string())
        .chain(args.iter().map(|s| s.to_string()))
        .collect();
    run_cmd_owned(&owned, cwd, tag)
}

/// Run a command described as `[program, arg1, arg2, …]`. On Windows, routes
/// `.cmd`/`.bat` wrappers (npm, vsce, trunk, …) through `cmd /C`.
pub fn run_cmd_owned(parts: &[String], cwd: &Path, tag: &str) -> Result<(), String> {
    if parts.is_empty() {
        return Err(format!("[{tag}] empty command"));
    }
    let program = &parts[0];
    let args: Vec<&str> = parts[1..].iter().map(String::as_str).collect();
    let rendered = render_cmd(parts);

    #[cfg(windows)]
    let mut child = {
        let mut cmd_args = vec!["/C", program.as_str()];
        cmd_args.extend_from_slice(&args);
        let mut cmd = Command::new("cmd");
        cmd.args(&cmd_args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn().map_err(|e| {
            format!(
                "failed to run `{rendered}` in {} via cmd: {e}",
                crate::paths::disp(cwd)
            )
        })?
    };
    #[cfg(not(windows))]
    let mut child = {
        let mut cmd = Command::new(program);
        cmd.args(&args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn().map_err(|e| {
            format!(
                "failed to run `{rendered}` in {}: {e}",
                crate::paths::disp(cwd)
            )
        })?
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("failed to capture stdout for `{rendered}`"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("failed to capture stderr for `{rendered}`"))?;

    let stdout_handle = forward_stream(stdout, std::io::stdout());
    let stderr_handle = forward_stream(stderr, std::io::stderr());

    let status = child.wait().map_err(|e| {
        format!(
            "failed to wait for `{rendered}` in {}: {e}",
            crate::paths::disp(cwd)
        )
    })?;

    let stdout = join_stream(stdout_handle, "stdout")?;
    let stderr = join_stream(stderr_handle, "stderr")?;

    if !status.success() {
        return Err(format_failure_report(
            &rendered,
            cwd,
            &status.to_string(),
            &stdout,
            &stderr,
        ));
    }
    Ok(())
}

fn forward_stream<R, W>(mut reader: R, mut writer: W) -> thread::JoinHandle<Result<Vec<u8>, String>>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buf = [0_u8; 8192];
        loop {
            let count = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            push_tail(&mut captured, &buf[..count]);
            writer.write_all(&buf[..count]).map_err(|e| e.to_string())?;
            writer.flush().map_err(|e| e.to_string())?;
        }
        Ok(captured)
    })
}

fn join_stream(
    handle: thread::JoinHandle<Result<Vec<u8>, String>>,
    name: &str,
) -> Result<Vec<u8>, String> {
    handle
        .join()
        .map_err(|_| format!("failed to join {name} forwarding thread"))?
}

fn push_tail(captured: &mut Vec<u8>, chunk: &[u8]) {
    if chunk.len() >= MAX_CAPTURE_BYTES {
        captured.clear();
        captured.extend_from_slice(&chunk[chunk.len() - MAX_CAPTURE_BYTES..]);
        return;
    }

    let overflow = captured
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(MAX_CAPTURE_BYTES);
    if overflow > 0 {
        captured.drain(..overflow);
    }
    captured.extend_from_slice(chunk);
}

fn format_failure_report(
    rendered: &str,
    cwd: &Path,
    status: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let mut msg = format!(
        "`{rendered}` failed in {} with status {status}",
        crate::paths::disp(cwd)
    );

    if let Some(stderr_tail) = format_output_tail(stderr) {
        msg.push_str("\n--- stderr tail ---\n");
        msg.push_str(&stderr_tail);
    }
    if let Some(stdout_tail) = format_output_tail(stdout) {
        msg.push_str("\n--- stdout tail ---\n");
        msg.push_str(&stdout_tail);
    }

    msg
}

fn format_output_tail(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let start = lines.len().saturating_sub(MAX_CAPTURE_LINES);
    let tail = lines[start..].join("\n");
    if bytes.len() == MAX_CAPTURE_BYTES || start > 0 {
        Some(format!("(captured tail)\n{tail}"))
    } else {
        Some(tail)
    }
}

fn render_cmd(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| quote_cmd_part(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_cmd_part(part: &str) -> String {
    if part.is_empty() || part.chars().any(|ch| ch.is_whitespace() || ch == '"') {
        format!("\"{}\"", part.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        part.to_string()
    }
}

/// Resolve an executable on PATH (`where` on Windows, `which` elsewhere).
pub fn which(name: &str) -> Result<PathBuf, ()> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let out = Command::new(cmd).arg(name).output().map_err(|_| ())?;
    if out.status.success() {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .lines()
                .next()
                .unwrap_or(""),
        ))
    } else {
        Err(())
    }
}
