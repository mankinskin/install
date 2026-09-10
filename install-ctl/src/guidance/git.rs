//! Minimal shallow-clone helper shelling out to the system `git` binary,
//! styled after `workflow-tools/session/crates/worktree-ctl/src/git.rs`:
//! no `git2`/`gix` dependency, same Windows extended-length-path handling.

use std::{path::Path, process::Command as ProcessCommand};

/// Shallow-clone `url` into `dest`. `dest` must not already exist; git
/// creates it. Returns git's stderr (trimmed) as the error on failure.
pub fn clone_shallow(url: &str, dest: &Path) -> Result<(), String> {
    let output = ProcessCommand::new("git")
        .args(["clone", "--depth", "1", url, &normalize_git_path(dest)])
        .output()
        .map_err(|error| format!("failed to start git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
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
