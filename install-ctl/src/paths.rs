//! Filesystem helpers (ported from viewer-ctl). Repo-root resolution stays
//! in `registry.rs::resolve_repo_root` — see that module's doc comment.

use std::{fs, path::Path};

/// Render a path for human-readable output with forward slashes regardless
/// of platform. Use everywhere instead of [`Path::display`] so log lines
/// don't mix `\` and `/` separators on Windows.
pub fn disp(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Recursively copy `src` directory contents into `dst`.
///
/// `tag` is currently unused but kept in the signature so callers may attach
/// per-step logging in the future.
pub fn copy_dir_contents(src: &Path, dst: &Path, _tag: &str) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            fs::create_dir_all(&dest_path).map_err(|e| e.to_string())?;
            copy_dir_contents(&entry.path(), &dest_path, _tag)?;
        } else {
            fs::copy(entry.path(), &dest_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Path to `<crate_path>/Cargo.toml` as a String.
pub fn crate_manifest_path_str(crate_path: &Path) -> Result<String, String> {
    let p = crate_path.join("Cargo.toml");
    p.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("non-UTF-8 path: {}", disp(&p)))
}
