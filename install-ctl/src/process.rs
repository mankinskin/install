//! Process discovery and termination (ported from viewer-ctl).

use std::{
    process::{Command, Stdio},
    time::Duration,
};

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

use crate::warn;

/// Find PIDs of processes listening on `port`.
///
/// Tool selection priority (most reliable first):
/// - Windows: PowerShell `Get-NetTCPConnection` (locale-agnostic) → ss → netstat
/// - Linux/macOS: `ss` → `lsof` → `netstat`
pub fn pids_on_port(port: u16) -> Vec<Pid> {
    if cfg!(windows)
        && let Ok(out) = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "(Get-NetTCPConnection -LocalPort {port} -State Listen \
                     -ErrorAction SilentlyContinue).OwningProcess"
                ),
            ])
            .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let pids: Vec<Pid> = text
            .split_whitespace()
            .filter_map(|s| s.parse::<usize>().ok())
            .map(Pid::from)
            .collect();
        if !pids.is_empty() || out.status.success() {
            return pids;
        }
    }

    if let Ok(out) = Command::new("ss")
        .args(["-ltnp", &format!("sport = :{port}")])
        .output()
        && out.status.success()
    {
        return parse_ss_pids(&String::from_utf8_lossy(&out.stdout), port);
    }

    if let Ok(out) = Command::new("lsof")
        .args(["-ti", &format!(":{port}"), "-sTCP:LISTEN"])
        .output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter_map(|s| s.parse::<usize>().ok().map(Pid::from))
            .collect();
    }

    if let Ok(out) = Command::new("netstat").args(["-ano"]).output()
        && out.status.success()
    {
        return parse_netstat_pids(&String::from_utf8_lossy(&out.stdout), port);
    }

    vec![]
}

fn parse_ss_pids(output: &str, port: u16) -> Vec<Pid> {
    let suffix = format!(":{port}");
    let mut pids = Vec::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        if !cols[3].ends_with(&suffix) {
            continue;
        }
        if let Some(pid_str) = line
            .split("pid=")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.split(')').next())
            && let Ok(n) = pid_str.trim().parse::<usize>()
        {
            pids.push(Pid::from(n));
        }
    }
    pids
}

fn parse_netstat_pids(output: &str, port: u16) -> Vec<Pid> {
    let suffix = format!(":{port}");
    let mut pids = Vec::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 || cols[0] != "TCP" {
            continue;
        }
        if !cols[1].ends_with(&suffix) {
            continue;
        }
        // Locale-agnostic listening detection: foreign address ends with :0.
        if !cols[2].ends_with(":0") {
            continue;
        }
        if let Ok(n) = cols[4].parse::<usize>() {
            pids.push(Pid::from(n));
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Try increasingly aggressive kill strategies. Returns `true` if the process
/// is no longer alive when the function returns.
pub fn kill_process(pid: Pid, tag: &str) -> bool {
    warn!(tag, "killing PID {pid}");
    if cfg!(windows) {
        let result = Command::new("taskkill")
            .args(["/F", "/PID", &pid.as_u32().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if result.is_ok() {
            std::thread::sleep(Duration::from_millis(300));
            return !process_exists(pid);
        }
    }
    let _ = Command::new("kill").arg(pid.as_u32().to_string()).status();
    std::thread::sleep(Duration::from_millis(500));
    if !process_exists(pid) {
        return true;
    }
    let _ = Command::new("kill")
        .args(["-9", &pid.as_u32().to_string()])
        .status();
    std::thread::sleep(Duration::from_millis(500));
    !process_exists(pid)
}

pub fn process_exists(pid: Pid) -> bool {
    let sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.process(pid).is_some()
}

/// Find PIDs of all processes whose executable basename matches `package`.
///
/// Matches `<package>` and `<package>.exe` (case-insensitive on Windows).
/// Used to stop a running instance that still holds a file lock on
/// `target/debug/<package>.exe` before installing a replacement.
///
/// Excludes thread entries and the calling process's own PID: `install-ctl install all`
/// installs `install-ctl` itself, and a naive match would find and kill
/// the currently running `install-ctl` process before it finishes.
pub fn pids_by_image_name(package: &str) -> Vec<Pid> {
    let sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    let bare = package.to_ascii_lowercase();
    let exe = format!("{bare}.exe");
    let own_pid = Pid::from_u32(std::process::id());
    sys.processes()
        .iter()
        .filter_map(|(pid, proc)| {
            let name = proc.name().to_string_lossy().to_ascii_lowercase();
            (*pid != own_pid && proc.thread_kind().is_none() && (name == bare || name == exe))
                .then_some(*pid)
        })
        .collect()
}

/// Print best-effort identifying info for `pid` (image name, args).
pub fn print_process_info(pid: Pid, tag: &str) {
    if let Ok(out) = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid.as_u32()), "/FO", "LIST"])
        .output()
        && out.status.success()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.starts_with("Image Name")
                || line.starts_with("PID")
                || line.starts_with("Mem Usage")
            {
                warn!(tag, "  {line}");
            }
        }
        return;
    }
    if let Ok(out) = Command::new("ps")
        .args([
            "-p",
            &pid.as_u32().to_string(),
            "-o",
            "pid,comm,args",
            "--no-headers",
        ])
        .output()
        && out.status.success()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            warn!(tag, "  {line}");
        }
        return;
    }
    warn!(tag, "  PID: {}", pid.as_u32());
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::pids_by_image_name;
    use std::{
        fs,
        process::{Child, Command, Stdio},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    use sysinfo::Pid;

    #[test]
    fn image_discovery_excludes_own_process_and_tasks() {
        let own_pid = Pid::from_u32(std::process::id());
        let image = fs::read_to_string("/proc/self/comm").expect("read own process image");
        thread::scope(|scope| {
            let (ready_tx, ready_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let worker = thread::Builder::new()
                .name(image.trim().to_owned())
                .spawn_scoped(scope, move || {
                    let task = fs::read_link("/proc/thread-self").expect("resolve own worker task");
                    let tid = task
                        .file_name()
                        .expect("worker task ID")
                        .to_str()
                        .expect("numeric task name")
                        .parse::<u32>()
                        .expect("parse worker task ID");
                    ready_tx.send(Pid::from_u32(tid)).expect("announce task");
                    match release_rx.recv_timeout(Duration::from_secs(30)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                        Err(error) => panic!("worker release deadline: {error}"),
                    }
                })
                .expect("spawn same-image worker");
            let worker_pid = ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("worker readiness deadline");
            let own_tasks: Vec<Pid> = fs::read_dir("/proc/self/task")
                .expect("snapshot own tasks")
                .map(|entry| {
                    Pid::from_u32(
                        entry
                            .expect("task entry")
                            .file_name()
                            .to_str()
                            .expect("numeric task name")
                            .parse()
                            .expect("parse task ID"),
                    )
                })
                .collect();
            assert!(own_tasks.contains(&worker_pid), "worker must remain alive");
            let found = pids_by_image_name(image.trim());
            assert!(!found.contains(&own_pid), "own main PID was selected");
            let selected_tasks: Vec<Pid> = own_tasks
                .into_iter()
                .filter(|pid| found.contains(pid))
                .collect();
            assert!(
                selected_tasks.is_empty(),
                "own live task IDs were selected: {selected_tasks:?}; worker: {worker_pid}"
            );
            release_tx.send(()).expect("release worker");
            worker.join().expect("join own worker");
        });
    }

    struct OwnedChild(Child);

    impl OwnedChild {
        fn stop(&mut self) {
            self.0.kill().expect("terminate owned discovery fixture");
            self.0.wait().expect("reap owned discovery fixture");
            assert!(self.0.try_wait().expect("read reaped status").is_some());
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Err(error) = self.0.kill()
                && self.0.try_wait().ok().flatten().is_none()
            {
                eprintln!("owned discovery fixture kill failed: {error}");
            }
            if let Err(error) = self.0.wait() {
                eprintln!("owned discovery fixture reap failed: {error}");
            }
        }
    }

    #[test]
    fn image_discovery_retains_other_real_process() {
        const CHILD_READY: &str = "INSTALL_CTL_PROCESS_DISCOVERY_CHILD";
        if let Some(ready) = std::env::var_os(CHILD_READY) {
            fs::write(ready, b"ready").expect("announce discovery child");
            thread::sleep(Duration::from_secs(30));
            return;
        }
        let fixture = tempfile::tempdir().expect("owned discovery fixture directory");
        let ready = fixture.path().join("child-ready");
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().expect("real test executable"))
                .args([
                    "--exact",
                    "process::tests::image_discovery_retains_other_real_process",
                    "--nocapture",
                ])
                .env(CHILD_READY, &ready)
                .stdin(Stdio::null())
                .spawn()
                .expect("spawn real same-image child"),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                child.0.try_wait().expect("poll owned child").is_none(),
                "child exited before readiness"
            );
            assert!(Instant::now() < deadline, "child readiness deadline");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read(&ready).expect("read child readiness"), b"ready");
        let child_pid = Pid::from_u32(child.0.id());
        let image =
            fs::read_to_string(format!("/proc/{child_pid}/comm")).expect("read live child image");
        for package in [image.trim().to_owned(), image.trim().to_ascii_uppercase()] {
            assert!(
                pids_by_image_name(&package).contains(&child_pid),
                "real child {child_pid} missing for image {package}"
            );
        }
        assert!(
            !pids_by_image_name(&format!("not-child-{child_pid}")).contains(&child_pid),
            "nonmatching image selected owned child"
        );
        child.stop();
    }
}
