//! Minimal shallow-clone helper shelling out to the system `git` binary,
//! styled after `workflow-tools/session/crates/worktree-ctl/src/git.rs`:
//! no `git2`/`gix` dependency, same Windows extended-length-path handling.

use std::{path::Path, process::Command as ProcessCommand};

/// Shallow-clone `url` into `dest`. `dest` must not already exist; git
/// creates it. Returns git's stderr (trimmed) as the error on failure.
pub fn clone_shallow(url: &str, dest: &Path) -> Result<(), String> {
    let dest_str = normalize_git_path(dest);
    let output = ProcessCommand::new("git")
        .args(["clone", "--depth", "1", url, &dest_str])
        .output()
        .map_err(|error| format!("failed to start git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    // Best-effort submodule initialization for monorepos/superprojects.
    // First initialize top-level submodules.
    let _ = ProcessCommand::new("git")
        .current_dir(dest)
        .args(["submodule", "update", "--init", "--depth", "1"])
        .output();

    // Recursively initialize each top-level submodule individually so a failure
    // in one nested submodule does not block remaining submodules.
    if let Ok(status_output) = ProcessCommand::new("git")
        .current_dir(dest)
        .args(["submodule", "status"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&status_output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let sub_path = parts[1];
                let _ = ProcessCommand::new("git")
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
                    .output();
            }
        }
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
