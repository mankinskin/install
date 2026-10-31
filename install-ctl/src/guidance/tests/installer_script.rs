use super::*;

#[test]
fn installer_oneliner_updates_existing_installation_with_force_flag() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_script = manifest_dir.join("../../install.sh");
    assert!(
        relative_script.is_file(),
        "install.sh must exist at {}",
        relative_script.display()
    );

    let output = std::process::Command::new("bash")
        .current_dir(&manifest_dir)
        .arg("../../install.sh")
        .arg("--root")
        .arg("/tmp/mock-install-root")
        .arg("--dry-run")
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                out.status.success(),
                "install.sh --dry-run must succeed, status: {:?}, stdout: {stdout}, stderr: {stderr}",
                out.status
            );
            assert!(
                stdout.contains("cargo install --force"),
                "installer command must pass --force to ensure re-install/update overwrites an existing install-ctl binary: got {stdout}"
            );
            assert!(
                stdout.contains("--branch main"),
                "installer command must target --branch main by default to pull latest revision: got {stdout}"
            );
        }
        Err(e) => {
            eprintln!("skipping install.sh test: bash process launch failed: {e}");
        }
    }
}

#[test]
fn installer_oneliner_fetches_and_builds_latest_branch_revision_in_sandbox() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_script = manifest_dir.join("../../install.sh");
    if !relative_script.is_file() {
        return;
    }

    let repo_temp = TempDir::new().unwrap();
    let repo_dir = repo_temp.path();

    let git_init = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_dir)
        .output();
    let Ok(init_out) = git_init else {
        eprintln!("skipping test: git command not available");
        return;
    };
    if !init_out.status.success() {
        eprintln!("skipping test: git init failed");
        return;
    }

    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_dir)
        .output();

    write(
        repo_dir,
        "Cargo.toml",
        r#"[package]
name = "install-ctl"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "install-ctl"
path = "src/main.rs"
"#,
    );
    write(repo_dir, "src/main.rs", r#"fn main() { println!("v1"); }"#);

    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo_dir)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "v1"])
        .current_dir(repo_dir)
        .output();

    let raw_repo_path = repo_dir.to_string_lossy().replace('\\', "/");
    let clean_repo_path = raw_repo_path
        .trim_start_matches("//?/")
        .trim_start_matches("\\\\?\\")
        .trim_start_matches('/');
    let repo_url = format!("file:///{clean_repo_path}");

    let install_target = TempDir::new().unwrap();
    let install_root = install_target.path().to_string_lossy().replace('\\', "/");

    let mut path_env = std::env::var("PATH").unwrap_or_default();
    if let Ok(cargo_home) = std::env::var("CARGO_HOME").or_else(|_| {
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(|h| format!("{h}/.cargo"))
    }) {
        let bin_dir = format!("{cargo_home}/bin");
        path_env = format!("{bin_dir};{bin_dir}:{path_env}");
    }

    let run1 = std::process::Command::new("bash")
        .current_dir(&manifest_dir)
        .env("PATH", &path_env)
        .arg("../../install.sh")
        .arg("--root")
        .arg(&install_root)
        .arg("--repository")
        .arg(&repo_url)
        .arg("--branch")
        .arg("main")
        .output();

    let Ok(out1) = run1 else {
        eprintln!("skipping test: bash or cargo not available");
        return;
    };
    assert!(
        out1.status.success(),
        "first install.sh run must succeed: stdout: {}, stderr: {}",
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr)
    );

    let bin_path = if cfg!(windows) {
        install_target.path().join("bin/install-ctl.exe")
    } else {
        install_target.path().join("bin/install-ctl")
    };
    assert!(
        bin_path.is_file(),
        "installed binary must exist after first run"
    );

    let run_bin1 = std::process::Command::new(&bin_path).output().unwrap();
    let output_v1 = String::from_utf8_lossy(&run_bin1.stdout).trim().to_string();
    assert_eq!(output_v1, "v1", "first install must produce v1 binary");

    write(repo_dir, "src/main.rs", r#"fn main() { println!("v2"); }"#);
    let _ = std::process::Command::new("git")
        .args(["commit", "-am", "v2"])
        .current_dir(repo_dir)
        .output();

    let run2 = std::process::Command::new("bash")
        .current_dir(&manifest_dir)
        .env("PATH", &path_env)
        .arg("../../install.sh")
        .arg("--root")
        .arg(&install_root)
        .arg("--repository")
        .arg(&repo_url)
        .arg("--branch")
        .arg("main")
        .output()
        .unwrap();

    assert!(
        run2.status.success(),
        "second install.sh run must succeed: stdout: {}, stderr: {}",
        String::from_utf8_lossy(&run2.stdout),
        String::from_utf8_lossy(&run2.stderr)
    );

    let run_bin2 = std::process::Command::new(&bin_path).output().unwrap();
    let output_v2 = String::from_utf8_lossy(&run_bin2.stdout).trim().to_string();
    assert_eq!(
        output_v2, "v2",
        "second install must update installed binary to v2 from latest main branch"
    );
}
