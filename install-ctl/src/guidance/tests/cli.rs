use super::*;

#[test]
fn cargo_install_uses_the_built_lockfile_and_offline_remains_opt_in() {
    use clap::Parser;

    let cases: &[(&[&str], bool, bool)] = &[
        (&[], false, false),
        (&["--offline"], true, false),
        (&["--locked"], false, true),
        (&["--offline", "--locked"], true, true),
    ];
    for (flags, expected_offline, expected_locked) in cases {
        let parsed = crate::Cli::try_parse_from(
            ["install-ctl", "install", "install-ctl"]
                .into_iter()
                .chain(flags.iter().copied()),
        )
        .unwrap();
        let Some(crate::Command::Install {
            offline, locked, ..
        }) = parsed.command
        else {
            panic!("expected install command");
        };
        assert_eq!(offline, *expected_offline);
        assert_eq!(locked, *expected_locked);

        let args = crate::install_args(
            Path::new("repo"),
            Path::new("target"),
            "install/install-ctl",
            &["install-ctl"],
            &[],
            true,
            crate::CargoInstallOptions { offline, locked },
        );
        assert_eq!(args.iter().any(|arg| arg == "--offline"), offline);
        assert!(args.iter().any(|arg| arg == "--locked"));
    }
}

#[test]
fn warmup_build_does_not_inherit_install_options() {
    let plan = crate::RustBuildPlan {
        manifest_path: "Cargo.toml".to_string(),
        packages: vec![],
        bins: vec![],
        feature_flags: vec![],
    };
    let args = plan.build_args();
    assert!(!args.iter().any(|arg| arg == "--offline"));
    assert!(!args.iter().any(|arg| arg == "--locked"));
}

#[test]
fn guidance_link_cli_requires_exactly_one_mode_and_known_harness() {
    use clap::Parser;
    for command in ["link", "unlink"] {
        let base = [
            "install-ctl",
            "guidance",
            command,
            "--harness",
            "copilot-cli",
        ];
        assert!(crate::Cli::try_parse_from(base).is_err());
        assert!(crate::Cli::try_parse_from(base.into_iter().chain(["--plan", "--apply"])).is_err());
        assert!(crate::Cli::try_parse_from(base.into_iter().chain(["--plan", "--json"])).is_ok());
        assert!(crate::Cli::try_parse_from(base.into_iter().chain(["--apply"])).is_ok());
        assert!(
            crate::Cli::try_parse_from([
                "install-ctl",
                "guidance",
                command,
                "--harness",
                "unknown",
                "--plan"
            ])
            .is_err()
        );
    }
}

#[test]
fn guidance_link_cli_plan_does_not_create_harness_paths() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), ".agents/agents/example.agent.md", "original");
    let args = crate::guidance::GuidanceLinkArgs {
        repo: repo.path().to_owned(),
        harness: crate::guidance::links::Harness::CopilotCli,
        plan: true,
        apply: false,
        json: true,
        sources: vec![],
        migrate: false,
    };
    crate::guidance::run(crate::guidance::GuidanceCmd::Link(args)).unwrap();
    assert!(!repo.path().join(".github").exists());
    assert_eq!(
        fs::read_to_string(repo.path().join(".agents/agents/example.agent.md")).unwrap(),
        "original"
    );
}

#[test]
fn guidance_link_cli_vscode_requires_explicit_sources_and_plans_without_writes() {
    use clap::Parser;
    let base = [
        "install-ctl",
        "guidance",
        "link",
        "--harness",
        "vscode",
        "--plan",
    ];
    assert!(crate::Cli::try_parse_from(base).is_err());
    assert!(
        crate::Cli::try_parse_from(base.into_iter().chain(["--source", "root=.agents"])).is_ok()
    );

    let repo = TempDir::new().unwrap();
    write(repo.path(), ".agents/agents/demo.agent.md", "original");
    let args = crate::guidance::GuidanceLinkArgs {
        repo: repo.path().to_owned(),
        harness: crate::guidance::links::Harness::Vscode,
        plan: true,
        apply: false,
        json: true,
        sources: vec!["root=.agents".to_owned()],
        migrate: false,
    };
    crate::guidance::run(crate::guidance::GuidanceCmd::Link(args)).unwrap();
    assert!(!repo.path().join(".github").exists());
    assert!(!repo.path().join(".vscode").exists());
}

#[test]
#[cfg_attr(windows, ignore = "requires real directory symlink privilege")]
fn guidance_link_cli_vscode_apply_writes_complete_transaction() {
    let repo = TempDir::new().unwrap();
    write(repo.path(), ".agents/agents/demo.agent.md", "original");
    let args = crate::guidance::GuidanceLinkArgs {
        repo: repo.path().to_owned(),
        harness: crate::guidance::links::Harness::Vscode,
        plan: false,
        apply: true,
        json: true,
        sources: vec!["root=.agents".to_owned()],
        migrate: false,
    };
    crate::guidance::run(crate::guidance::GuidanceCmd::Link(args)).unwrap();
    assert!(
        repo.path()
            .join(".github/agents/root/demo.agent.md")
            .is_file()
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(repo.path().join(".vscode/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings["chat.agentFilesLocations"][".github/agents/root"],
        true
    );
    assert!(
        repo.path()
            .join(".vscode/install-ctl-guidance-links.json")
            .is_file()
    );
}
