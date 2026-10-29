//! Detect whether an installed binary is already byte-identical to what a
//! rebuild would produce, so `install-ctl` can skip killing/reinstalling
//! binaries that don't actually need it.
//!
//! Reconstructing "would this rebuild produce a different binary" from
//! sources, dependency versions, and build config directly would mean
//! reimplementing cargo's own fingerprinting (rustc version, env vars,
//! `RUSTFLAGS`, registry checksums, codegen settings, ...), which cargo
//! already does more reliably than we could. So instead of hashing inputs,
//! we let the warm-up `cargo build` run first (cheap: cargo itself skips
//! recompilation when its fingerprint is unchanged) and then compare the
//! actual output binary's content hash against what's currently installed.
//! Identical hashes mean `cargo install` would be a no-op.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

pub fn installed_path(install_root: &Path, bin: &str) -> std::path::PathBuf {
    install_root
        .join("bin")
        .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX))
}

pub fn is_installed(install_root: &Path, bin: &str) -> Result<bool, String> {
    let path = installed_path(install_root, bin);
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(format!(
            "installed binary is not a file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("failed to inspect {}: {error}", path.display())),
    }
}

/// Directory `cargo install` copies binaries into: `$CARGO_HOME/bin`,
/// defaulting to `~/.cargo/bin` when `CARGO_HOME` is unset.
fn sha256_file(path: &Path) -> Option<[u8; 32]> {
    let bytes = fs::read(path).ok()?;
    Some(Sha256::digest(&bytes).into())
}

/// `true` when the binary the warm-up build just produced at
/// `<target_dir>/release/<bin>` is byte-identical to the one already
/// installed at `$CARGO_HOME/bin/<bin>`. When either file is missing (fresh
/// install, or the build didn't produce this binary) this returns `false`
/// so the caller falls back to installing.
pub fn is_up_to_date(target_dir: &Path, install_root: &Path, bin: &str) -> bool {
    let suffix = std::env::consts::EXE_SUFFIX;
    let built = target_dir.join("release").join(format!("{bin}{suffix}"));
    let installed = install_root.join("bin").join(format!("{bin}{suffix}"));
    match (sha256_file(&built), sha256_file(&installed)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_comparison_distinguishes_missing_identical_and_different_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let root = temp.path().join("root");
        assert!(!is_installed(&root, "tool").unwrap());
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(target.join("release")).unwrap();
        let installed = installed_path(&root, "tool");
        let built = target.join("release").join(installed.file_name().unwrap());
        fs::write(&installed, b"old").unwrap();
        assert!(!is_up_to_date(&target, &root, "tool"));
        fs::write(&built, b"old").unwrap();
        assert!(is_up_to_date(&target, &root, "tool"));
        fs::write(&built, b"new").unwrap();
        assert!(!is_up_to_date(&target, &root, "tool"));
    }

    #[test]
    fn status_rejects_non_file_installation_targets() {
        let temp = tempfile::tempdir().unwrap();
        let path = installed_path(temp.path(), "tool");
        fs::create_dir_all(&path).unwrap();
        let error = is_installed(temp.path(), "tool").unwrap_err();
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("not a file"), "{error}");
    }
}
