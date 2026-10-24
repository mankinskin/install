use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=src/command_helper.rs");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets target OS");
    let helper = out_dir.join(if target_os == "windows" {
        "install-ctl-test-helper.exe"
    } else {
        "install-ctl-test-helper"
    });
    let status = Command::new(env::var("RUSTC").expect("Cargo sets RUSTC"))
        .args([
            "--edition=2024",
            "src/command_helper.rs",
            "--target",
            &target,
            "-o",
        ])
        .arg(&helper)
        .status()
        .expect("run Cargo-selected rustc for install-ctl test helper");
    assert!(
        status.success(),
        "Cargo-selected rustc failed while building test helper for {target}"
    );
    println!(
        "cargo::rustc-env=INSTALL_CTL_RAW_HELPER={}",
        helper.display()
    );
}
