//! Minimal shallow-clone helper shelling out to the system `git` binary,
//! styled after `workflow-tools/session/crates/worktree-ctl/src/git.rs`:
//! no `git2`/`gix` dependency, same Windows extended-length-path handling.

use std::{path::Path, process::Command as ProcessCommand};

/// Shallow-clone `url` into `dest`. `dest` must not already exist; git
/// creates it. Returns git's stderr (trimmed) as the error on failure.
pub fn clone_shallow(url: &str, dest: &Path) -> Result<(), String> {
    let dest_str = normalize_git_path(dest);
    eprintln!("guidance get: running git clone --depth 1");
    let status = ProcessCommand::new("git")
        .args(["clone", "--depth", "1", url, &dest_str])
        .status()
        .map_err(|error| format!("failed to start git: {error}"))?;
    if !status.success() {
        return Err(format!("git clone exited with {status}"));
    }

    // Best-effort submodule initialization for monorepos/superprojects.
    // First initialize top-level submodules.
    eprintln!("guidance get: initializing top-level submodules");
    match ProcessCommand::new("git")
        .current_dir(dest)
        .args(["submodule", "update", "--init", "--depth", "1"])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "guidance get: top-level submodule initialization exited with {status}; continuing"
        ),
        Err(error) => eprintln!(
            "guidance get: could not start top-level submodule initialization: {error}; continuing"
        ),
    }

    // Recursively initialize each top-level submodule individually so a failure
    // in one nested submodule does not block remaining submodules.
    if let Ok(status_output) = ProcessCommand::new("git")
        .current_dir(dest)
        .args(["submodule", "status"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&status_output.stdout);
        if !stdout.is_empty() {
            eprint!("{stdout}");
        }
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        if !stderr.is_empty() {
            eprint!("{stderr}");
        }
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let sub_path = parts[1];
                eprintln!("guidance get: initializing submodule {sub_path}");
                match ProcessCommand::new("git")
                    .current_dir(dest)
                    .args([
                        "submodule",
                        "update",
                        "--init",
                        "--recursive",
                        "--depth",
                        "1",
                        sub_path,
                    ])
                    .status()
                {
                    Ok(status) if status.success() => {}
                    Ok(status) => eprintln!(
                        "guidance get: submodule {sub_path} initialization exited with {status}; continuing"
                    ),
                    Err(error) => eprintln!(
                        "guidance get: could not start submodule {sub_path} initialization: {error}; continuing"
                    ),
                }
            }
        }
    } else {
        eprintln!("guidance get: could not inspect submodule status; continuing");
    }

    Ok(())
}

fn normalize_git_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let path = path
        .strip_prefix(r"\\?\UNC\")
        .map(|path| format!("//{path}"))
        .or_else(|| path.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| path.into_owned());
    path.replace('\\', "/")
}
