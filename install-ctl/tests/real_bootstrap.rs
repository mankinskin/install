mod support;

use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use ratatui_testlib::KeyCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use support::bounded_pty::BoundedPty;

const IDS: [&str; 6] = [
    "ticket",
    "spec-cli",
    "session-cli",
    "test-cli",
    "feedback-cli",
    "audit-cli",
];

#[derive(Deserialize)]
struct Registry {
    artifact: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    id: String,
    category: String,
}

#[test]
#[ignore = "builds and installs six real workspace tools; requires INSTALL_CTL_ACCEPTANCE_DIR"]
fn documented_six_tools_match_between_scripted_and_native_pty() {
    let evidence = std::env::var_os("INSTALL_CTL_ACCEPTANCE_DIR")
        .expect("set INSTALL_CTL_ACCEPTANCE_DIR to a new evidence directory");
    let evidence = Path::new(&evidence);
    fs::create_dir_all(evidence).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let scripted_root = evidence.join("scripted");
    let tui_root = evidence.join("tui");
    assert!(
        !scripted_root.exists() && !tui_root.exists(),
        "use fresh installation roots"
    );
    let installer = env!("CARGO_BIN_EXE_install-ctl");
    let log = fs::File::create(evidence.join("scripted.log")).unwrap();
    let mut child = Command::new(installer)
        .arg("install")
        .args(IDS)
        .arg("--root")
        .arg(&scripted_root)
        .env("INSTALL_CTL_REPO_ROOT", repo)
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(360);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "scripted install failed; inspect scripted.log"
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("scripted install exceeded 360s; inspect scripted.log");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let registry: Registry =
        toml::from_str(&fs::read_to_string(repo.join("install/artifacts.toml")).unwrap()).unwrap();
    let cli: Vec<_> = registry
        .artifact
        .iter()
        .filter(|entry| entry.category == "cli")
        .collect();
    for id in IDS {
        assert_eq!(cli.iter().filter(|entry| entry.id == id).count(), 1);
    }
    let mut command = portable_pty::CommandBuilder::new(installer);
    command.arg("tui");
    command.arg("--root");
    command.arg(&tui_root);
    command.env("INSTALL_CTL_REPO_ROOT", repo);
    let mut pty = BoundedPty::spawn(command, 180, 60, Duration::from_secs(360)).unwrap();
    pty.wait_for_text("Artifact categories").unwrap();
    pty.send_key(KeyCode::Down).unwrap();
    pty.send_key(KeyCode::Char(' ')).unwrap();
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Artifacts").unwrap();
    pty.send_key(KeyCode::Char('a')).unwrap();
    for (index, entry) in cli.iter().enumerate() {
        if index > 0 {
            pty.send_key(KeyCode::Down).unwrap();
        }
        if IDS.contains(&entry.id.as_str()) {
            pty.send_key(KeyCode::Char(' ')).unwrap();
        }
    }
    pty.send_key(KeyCode::Enter).unwrap();
    pty.wait_for_text("Review").unwrap();
    let review = pty.screen_contents();
    fs::write(evidence.join("review.txt"), &review).unwrap();
    for id in IDS {
        assert!(
            review.contains(&format!("install: {id} [")),
            "review omitted {id}"
        );
    }
    assert_eq!(review.matches("install: ").count(), IDS.len());
    pty.send_key(KeyCode::Char('y')).unwrap();
    pty.wait_for_text("written: audit-cli").unwrap();
    for id in IDS {
        pty.wait_for_text(&format!("written: {id}")).unwrap();
    }
    let status = pty.wait_exit().unwrap();
    fs::write(evidence.join("tui-result.txt"), pty.screen_contents()).unwrap();
    pty.finish().unwrap();
    assert!(
        status.success(),
        "TUI install failed; inspect tui-result.txt"
    );

    let expected: BTreeSet<_> = ["ticket", "spec", "session", "test", "feedback", "audit"]
        .into_iter()
        .map(|stem| format!("{stem}{}", std::env::consts::EXE_SUFFIX))
        .collect();
    let mut records = Vec::new();
    for root in [&scripted_root, &tui_root] {
        let found: BTreeSet<_> = fs::read_dir(root.join("bin"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            found,
            expected,
            "unexpected artifact set at {}",
            root.display()
        );
    }
    for name in &expected {
        let mut help_outputs = Vec::new();
        for root in [&scripted_root, &tui_root] {
            let binary = root.join("bin").join(name);
            let bytes = fs::read(&binary).unwrap();
            let output = Command::new(&binary).arg("--help").output().unwrap();
            assert!(
                output.status.success(),
                "{} --help failed",
                binary.display()
            );
            let help = String::from_utf8(output.stdout).unwrap();
            assert!(
                help.contains("Usage:"),
                "{} omitted usage",
                binary.display()
            );
            help_outputs.push(help.clone());
            records.push(serde_json::json!({
                "path": binary,
                "sha256": format!("{:x}", Sha256::digest(&bytes)),
                "help_exit": output.status.code(),
                "help": help,
            }));
        }
        assert_eq!(
            help_outputs[0], help_outputs[1],
            "help contract mismatch: {name}"
        );
    }
    fs::write(
        evidence.join("readback.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "ids": IDS,
            "expected_files": expected,
            "artifacts": records,
            "parity": "passed",
        }))
        .unwrap(),
    )
    .unwrap();
}
