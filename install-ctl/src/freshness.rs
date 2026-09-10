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

/// Directory `cargo install` copies binaries into: `$CARGO_HOME/bin`,
/// defaulting to `~/.cargo/bin` when `CARGO_HOME` is unset.
fn cargo_bin_dir() -> Option<std::path::PathBuf> {
    if let Ok(home) = std::env::var("CARGO_HOME") {
        return Some(std::path::PathBuf::from(home).join("bin"));
    }
    dirs::home_dir().map(|home| home.join(".cargo").join("bin"))
}

fn sha256_file(path: &Path) -> Option<[u8; 32]> {
    let bytes = fs::read(path).ok()?;
    Some(Sha256::digest(&bytes).into())
}

/// `true` when the binary the warm-up build just produced at
/// `<target_dir>/release/<bin>` is byte-identical to the one already
/// installed at `$CARGO_HOME/bin/<bin>`. When either file is missing (fresh
/// install, or the build didn't produce this binary) this returns `false`
/// so the caller falls back to installing.
pub fn is_up_to_date(target_dir: &Path, bin: &str) -> bool {
    let suffix = std::env::consts::EXE_SUFFIX;
    let built = target_dir.join("release").join(format!("{bin}{suffix}"));
    let Some(installed) = cargo_bin_dir().map(|dir| dir.join(format!("{bin}{suffix}"))) else {
        return false;
    };
    match (sha256_file(&built), sha256_file(&installed)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}
