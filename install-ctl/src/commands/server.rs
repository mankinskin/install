//! Server lifecycle: build, install, start, stop (ported from viewer-ctl).

use std::{
    env, fs,
    io::Write,
    net::TcpStream,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::{
    config::{Config, Server},
    error, info,
    paths::{crate_manifest_path_str, disp},
    process::{kill_process, pids_by_image_name, pids_on_port, print_process_info},
    shell::{run_cmd_args, which},
    warn,
};

pub fn build_server(root: &Path, s: &Server) -> Result<(), String> {
    let tag = s.name.as_str();
    let crate_path = root.join(&s.source_dir);
    let manifest = crate_manifest_path_str(&crate_path)?;
    info!(tag, "cargo build --release in {}", disp(&crate_path));
    run_cmd_args(
        "cargo",
        &["build", "--release", "--manifest-path", manifest.as_str()],
        root,
        tag,
    )
}

pub fn install_server(root: &Path, s: &Server) -> Result<(), String> {
    let tag = s.name.as_str();

    // Determine the binary name: on Windows the release binary has an .exe suffix.
    let bin_name = if cfg!(target_os = "windows") {
        format!("{}.exe", s.package)
    } else {
        s.package.clone()
    };

    // Look for a pre-built release binary in the workspace target directory.
    // This avoids rebuilding from source when `build_server` / `trunk build`
    // was already run — and also avoids the mixed-separator issues that arise
    // from passing a Windows Path to `cargo install --path`.
    let release_bin = root.join("target").join("release").join(&bin_name);

    if release_bin.exists() {
        // Install by copying the binary to ~/.cargo/bin/.
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".cargo")))
            .ok_or_else(|| {
                "cannot locate cargo home (CARGO_HOME not set and HOME unknown)".to_string()
            })?;
        let dest = cargo_home.join("bin").join(&bin_name);
        info!(
            tag,
            "installing pre-built binary {} → {}",
            disp(&release_bin),
            disp(&dest)
        );
        fs::copy(&release_bin, &dest).map_err(|e| format!("failed to copy binary: {e}"))?;
        Ok(())
    } else {
        // No pre-built binary found — fall back to building from source.
        // Run from the crate directory and install `--path .` so Cargo sees a
        // valid package path on every platform.
        let crate_path = root.join(&s.source_dir);
        info!(
            tag,
            "no pre-built binary found; building via cargo install --path . from {}",
            disp(&crate_path)
        );
        run_cmd_args(
            "cargo",
            &["install", "--path", ".", "--force"],
            &crate_path,
            tag,
        )
    }
}

pub fn uninstall_server(_root: &Path, s: &Server) -> Result<(), String> {
    let tag = s.name.as_str();
    let port = port_for(s);

    let listeners = pids_on_port(port);
    if !listeners.is_empty() {
        info!(
            tag,
            "stopping server on port {port} before uninstallation..."
        );
        for pid in &listeners {
            kill_process(*pid, tag);
        }
    }

    let bin_name = if cfg!(target_os = "windows") {
        format!("{}.exe", s.package)
    } else {
        s.package.clone()
    };

    if let Some(cargo_home) = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cargo")))
    {
        let dest = cargo_home.join("bin").join(&bin_name);
        if dest.is_file() {
            info!(tag, "removing installed binary {}", disp(&dest));
            fs::remove_file(&dest)
                .map_err(|e| format!("failed to remove binary {}: {e}", dest.display()))?;
        } else {
            info!(tag, "installed binary {} is already missing", disp(&dest));
        }
    }

    Ok(())
}

pub fn cmd_start(
    cfg: &Config,
    root: &Path,
    server: &str,
    foreground: bool,
    extra: Vec<String>,
) -> Result<(), String> {
    let s = cfg
        .server(server)
        .ok_or_else(|| format!("no [[server]] named `{server}` in viewer-ctl.toml"))?;
    let tag = s.name.as_str();
    let port = port_for(s);

    // Step 1: ensure port is free.
    info!(tag, "checking port {port} for existing instances...");
    let listeners = pids_on_port(port);
    if !listeners.is_empty() {
        warn!(
            tag,
            "port {port} in use by PID(s): {:?}",
            listeners.iter().map(|p| p.as_u32()).collect::<Vec<_>>()
        );
        for pid in &listeners {
            kill_process(*pid, tag);
        }
        std::thread::sleep(Duration::from_secs(1));
        let remaining = pids_on_port(port);
        if !remaining.is_empty() {
            return Err(format!(
                "port {port} still occupied by {:?} — aborting",
                remaining.iter().map(|p| p.as_u32()).collect::<Vec<_>>()
            ));
        }
        info!(tag, "port {port} freed.");
    }

    // Step 2: locate the binary; auto-install on first run.
    let bin_path = match which(&s.package) {
        Ok(p) => p,
        Err(_) => {
            info!(
                tag,
                "{} not found on PATH — installing from source...", s.package
            );
            install_server(root, s)?;
            which(&s.package).map_err(|_| {
                format!(
                    "installed {} but still not found on PATH — check ~/.cargo/bin is in PATH",
                    s.package
                )
            })?
        }
    };

    // Step 3: derive STATIC_DIR from linked frontend if installed.
    let mut env_vars: Vec<(String, String)> =
        s.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Some(fe) = cfg.frontend_for_server(&s.name) {
        let install_dir = cfg.frontend_install_dir(&fe.name);
        if install_dir.is_dir() {
            info!(tag, "STATIC_DIR={}", disp(&install_dir));
            env_vars.push((
                "STATIC_DIR".into(),
                install_dir.to_string_lossy().into_owned(),
            ));
        } else {
            warn!(
                tag,
                "frontend `{}` not installed (no {}); server will use its built-in default",
                fe.name,
                disp(&install_dir)
            );
        }
    }
    env_vars.push(("PORT".into(), port.to_string()));

    // Step 4: assemble args and launch.
    let mut server_args: Vec<String> = s.start_args.clone();
    server_args.extend(extra);

    info!(tag, "starting {} on port {port}", disp(&bin_path));

    if foreground {
        info!(
            tag,
            "foreground mode: stdout/stderr inherited; blocking until exit."
        );
        run_server_foreground(&bin_path, &server_args, &env_vars, root, tag)
    } else {
        spawn_server(&bin_path, &server_args, &env_vars, root, tag)?;

        // Wait for the server to actually bind the port so callers (VS Code
        // `serverReadyAction`, scripts, etc.) can rely on the URL being live
        // when this process exits. Print a single, stable line that downstream
        // pattern matchers can latch onto:
        //     Listening on http://localhost:<port>
        wait_for_port_ready(port, Duration::from_secs(15), tag);
        println!("Listening on http://localhost:{port}");
        // Explicitly flush stdout so piped consumers (tail, grep, etc.) see the
        // sentinel line before viewer-ctl exits.
        let _ = std::io::stdout().flush();
        Ok(())
    }
}

/// Poll TCP connect to localhost:port until success or timeout.
///
/// Logs a warning on timeout but does not fail — the server may still come
/// up later, and the caller can decide what to do. We only block start()
/// long enough to make `serverReadyAction` reliable.
fn wait_for_port_ready(port: u16, timeout: Duration, tag: &str) {
    let deadline = Instant::now() + timeout;
    let addr = format!("127.0.0.1:{port}");
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &addr.parse().expect("valid socket addr"),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            info!(tag, "port {port} is accepting connections.");
            return;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    warn!(
        tag,
        "timed out waiting for port {port} to become ready after {:?}", timeout
    );
}

pub fn cmd_stop(cfg: &Config, server: &str) -> Result<(), String> {
    let s = cfg
        .server(server)
        .ok_or_else(|| format!("no [[server]] named `{server}` in viewer-ctl.toml"))?;
    let tag = s.name.as_str();
    let port = port_for(s);

    // ── Phase 1: kill any process listening on the server port ────────────
    info!(tag, "looking for {} on port {port}...", s.package);
    let pids = pids_on_port(port);
    if pids.is_empty() {
        info!(tag, "no process listening on port {port}.");
    }
    for pid in &pids {
        warn!(tag, "found process on port {port}:");
        print_process_info(*pid, tag);
        if kill_process(*pid, tag) {
            info!(tag, "PID {} terminated.", pid.as_u32());
        } else {
            error!(tag, "could not terminate PID {}.", pid.as_u32());
            if cfg!(windows) {
                error!(tag, "  taskkill /F /PID {}", pid.as_u32());
            }
            error!(tag, "  kill -9 {}", pid.as_u32());
            return Err(format!("failed to kill PID {}", pid.as_u32()));
        }
    }

    // ── Phase 2: kill any orphaned processes by image name ─────────────────
    // Processes that crashed before binding (or were launched from a different
    // path like target/release/) won't appear in the port scan above, but they
    // still hold file locks that prevent `install` from copying a new binary.
    let zombies: Vec<_> = pids_by_image_name(&s.package)
        .into_iter()
        .filter(|pid| !pids.contains(pid))
        .collect();
    if !zombies.is_empty() {
        warn!(
            tag,
            "found {} orphaned {} process(es) not on port {port} — cleaning up",
            zombies.len(),
            s.package
        );
        for pid in zombies {
            print_process_info(pid, tag);
            if kill_process(pid, tag) {
                info!(tag, "orphan PID {} terminated.", pid.as_u32());
            } else {
                error!(tag, "could not terminate orphan PID {}.", pid.as_u32());
            }
        }
    }

    Ok(())
}

fn port_for(s: &Server) -> u16 {
    env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(s.port)
}

/// Run the server in the foreground: inherit stdout/stderr from the current
/// process and block until the child exits. The caller's terminal sees all
/// server output directly. Returns an error if the process exits with a
/// non-zero status.
fn run_server_foreground(
    bin_path: &Path,
    args: &[String],
    env_vars: &[(String, String)],
    cwd: &Path,
    tag: &str,
) -> Result<(), String> {
    let mut cmd = Command::new(bin_path);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch {}: {e}", disp(bin_path)))?;

    info!(
        tag,
        "running in foreground (PID {}). Press Ctrl-C to stop.",
        child.id()
    );

    let status = child
        .wait()
        .map_err(|e| format!("error waiting for {}: {e}", disp(bin_path)))?;

    if status.success() {
        info!(tag, "process exited successfully.");
        Ok(())
    } else {
        Err(format!("{} exited with status: {}", disp(bin_path), status))
    }
}

fn spawn_server(
    bin_path: &Path,
    args: &[String],
    env_vars: &[(String, String)],
    cwd: &Path,
    tag: &str,
) -> Result<(), String> {
    // Spawn the server detached so viewer-ctl can exit cleanly.
    // Using exec() on Unix would replace the viewer-ctl process with the
    // server, which means wait_for_port_ready and the "Listening on..."
    // sentinel message would never be reached, causing piped consumers
    // (tail, grep, VS Code serverReadyAction) to hang indefinitely.
    let mut cmd = Command::new(bin_path);
    for (k, v) in env_vars {
        cmd.env(k, v);
    }
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // On Unix, move the child into a new process group so it is not
    // killed by SIGHUP when the terminal/parent session closes.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    // On Windows the shell creates the pipe with bInheritHandle=TRUE, so
    // every child spawned via CreateProcess(bInheritHandles=TRUE) inherits
    // the write-end of the pipe even though we redirect the child's own
    // stdout/stderr to NUL.  The child then keeps the pipe alive after
    // viewer-ctl exits, and `tail` never sees EOF.
    //
    // Fix: mark the parent's stdout and stderr handles as non-inheritable
    // right before we spawn, so the child receives no reference to the pipe.
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::HANDLE_FLAG_INHERIT,
            System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE},
        };
        unsafe {
            for handle_id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                let h = GetStdHandle(handle_id);
                if !h.is_null() && h != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                    windows_sys::Win32::Foundation::SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }

    cmd.spawn()
        .map_err(|e| format!("failed to launch {}: {e}", disp(bin_path)))?;
    info!(tag, "launched (detached). viewer-ctl exiting.");
    Ok(())
}
