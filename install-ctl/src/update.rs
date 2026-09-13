use std::{path::Path, process::Command};

const REPOSITORY_URL: &str = "https://github.com/mankinskin/workflow-tools";
const INSTALL_CTL_PATH: &str = "install/install-ctl";

pub fn print_plan() {
    let checkout = "<temporary checkout>";
    println!("git {}", git_clone_args(checkout).join(" "));
    println!("cargo {}", cargo_install_args(checkout).join(" "));
}

pub fn run() -> Result<(), String> {
    let checkout = tempfile::tempdir()
        .map_err(|error| format!("failed to create temporary checkout directory: {error}"))?;
    let checkout_path = checkout.path();
    run_command("git", &git_clone_args(checkout_path), None)?;
    run_command(
        "cargo",
        &cargo_install_args(checkout_path),
        Some(checkout_path),
    )?;
    println!("updated install-ctl from {REPOSITORY_URL}");
    Ok(())
}

fn git_clone_args(checkout: impl AsRef<Path>) -> Vec<String> {
    vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        REPOSITORY_URL.to_string(),
        checkout.as_ref().to_string_lossy().into_owned(),
    ]
}

fn cargo_install_args(checkout: impl AsRef<Path>) -> Vec<String> {
    vec![
        "install".to_string(),
        "--path".to_string(),
        checkout
            .as_ref()
            .join(INSTALL_CTL_PATH)
            .to_string_lossy()
            .into_owned(),
        "--bin".to_string(),
        "install-ctl".to_string(),
        "--force".to_string(),
    ]
}

fn run_command(program: &str, args: &[String], cwd: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{INSTALL_CTL_PATH, REPOSITORY_URL, cargo_install_args, git_clone_args};

    #[test]
    fn update_plan_fetches_the_canonical_repository_and_installs_install_ctl() {
        let checkout = "/tmp/install-ctl-update";
        let install_path = Path::new(checkout).join(INSTALL_CTL_PATH);
        assert_eq!(
            git_clone_args(checkout),
            vec!["clone", "--depth", "1", REPOSITORY_URL, checkout]
        );
        assert_eq!(
            cargo_install_args(checkout),
            vec![
                "install",
                "--path",
                install_path.to_str().unwrap(),
                "--bin",
                "install-ctl",
                "--force",
            ]
        );
    }
}
