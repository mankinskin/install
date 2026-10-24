use super::SelectiveTransport;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::TempDir;

#[path = "contract_tests.rs"]
mod contracts;
#[path = "test_server.rs"]
mod server;
pub(crate) use server::Server;

#[test]
fn transport_worker() {
    if std::env::var_os("INSTALL_CTL_OBJECT_WORKER").is_some() {
        super::process::worker_entry();
    }
}

pub(crate) struct Repository {
    pub(crate) root: TempDir,
    pub(crate) repo: PathBuf,
    pub(crate) head: String,
    pub(crate) selected: String,
    pub(crate) unrelated: String,
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("start fixture Git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Repository {
    pub(crate) fn new(filter: bool) -> Self {
        Self::with_format(filter, "sha1")
    }

    pub(crate) fn with_format(filter: bool, format: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo.git");
        fs::create_dir(&repo).unwrap();
        git(
            &repo,
            &["init", "--quiet", &format!("--object-format={format}")],
        );
        git(&repo, &["config", "user.name", "Transport Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(
            &repo,
            &[
                "config",
                "uploadpack.allowFilter",
                if filter { "true" } else { "false" },
            ],
        );
        git(&repo, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        fs::write(repo.join("selected.md"), b"# Selected\n").unwrap();
        fs::write(repo.join("unrelated.bin"), vec![0x5a; 256 * 1024]).unwrap();
        git(&repo, &["add", "selected.md", "unrelated.bin"]);
        git(&repo, &["commit", "--quiet", "-m", "fixture"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let selected = git(&repo, &["rev-parse", "HEAD:selected.md"]);
        let unrelated = git(&repo, &["rev-parse", "HEAD:unrelated.bin"]);
        Self {
            root,
            repo,
            head,
            selected,
            unrelated,
        }
    }

    fn probe(&self, url: &str, config: Option<&Path>, ssh: Option<&str>, success: bool) {
        let result = self.root.path().join("objects.json");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "guidance::remote::transport::tests::transport_probe",
            "--nocapture",
        ]);
        command
            .env("GUIDANCE_TRANSPORT_FIXTURE_URL", url)
            .env("GUIDANCE_TRANSPORT_FIXTURE_HEAD", &self.head)
            .env("GUIDANCE_TRANSPORT_FIXTURE_BLOB", &self.selected)
            .env("GUIDANCE_TRANSPORT_FIXTURE_UNRELATED", &self.unrelated)
            .env("GUIDANCE_TRANSPORT_FIXTURE_RESULT", &result)
            .env(
                "GUIDANCE_TRANSPORT_FIXTURE_SUCCESS",
                if success { "1" } else { "0" },
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env(
                "GIT_CONFIG_GLOBAL",
                config
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.root.path().join("absent.config")),
            );
        if let Some(ssh) = ssh {
            command
                .env("GIT_SSH_COMMAND", ssh)
                .env("GIT_SSH_VARIANT", "ssh");
        }
        let output = command.output().expect("start isolated fixture client");
        if let Some(root) = std::env::var_os("GUIDANCE_TRANSPORT_EVIDENCE_ROOT") {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("fixture").replace(':', "_");
            let evidence = PathBuf::from(root).join(name);
            fs::create_dir_all(&evidence).unwrap();
            for filename in [
                "objects.json",
                "requests.jsonl",
                "ssh.jsonl",
                "credentials.jsonl",
            ] {
                let source = self.root.path().join(filename);
                if source.exists() {
                    fs::copy(source, evidence.join(filename)).unwrap();
                }
            }
            fs::write(evidence.join("client-stderr.txt"), &output.stderr).unwrap();
        }
        assert!(
            output.status.success(),
            "client stdout: {}\nclient stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(&result).expect("read actual received-object inventory");
        let evidence: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if success {
            assert_eq!(evidence["head"], self.head);
            assert_eq!(evidence["blob"]["oid"], self.selected);
            assert_eq!(evidence["blob"]["kind"], "blob");
            assert_eq!(evidence["blob"]["bytes"], "# Selected\n");
            assert!(!String::from_utf8(bytes).unwrap().contains(&self.unrelated));
        } else {
            assert_eq!(evidence["unsupported"], true);
        }
    }
}

#[test]
fn transport_probe() {
    let Ok(url) = std::env::var("GUIDANCE_TRANSPORT_FIXTURE_URL") else {
        return;
    };
    let result = PathBuf::from(std::env::var_os("GUIDANCE_TRANSPORT_FIXTURE_RESULT").unwrap());
    let success = std::env::var("GUIDANCE_TRANSPORT_FIXTURE_SUCCESS").unwrap() == "1";
    let attempt = SelectiveTransport::connect(&url).and_then(|mut remote| {
        let head = remote.head_oid().to_owned();
        assert_eq!(head, std::env::var("GUIDANCE_TRANSPORT_FIXTURE_HEAD").unwrap());
        let metadata = remote.fetch_metadata(&head)?;
        let selected = std::env::var("GUIDANCE_TRANSPORT_FIXTURE_BLOB").unwrap();
        let unrelated = std::env::var("GUIDANCE_TRANSPORT_FIXTURE_UNRELATED").unwrap();
        assert!(!metadata.is_empty(), "metadata pack must contain real objects");
        assert!(metadata.iter().any(|object| object.oid == head));
        assert!(metadata.iter().any(|object| object.kind == "tree"));
        for object in &metadata {
            assert_ne!(object.kind, "blob");
            assert_ne!(object.oid, selected);
            assert_ne!(object.oid, unrelated);
            assert!(!object.data.is_empty());
        }
        let distinct: BTreeSet<_> = metadata.iter().map(|object| &object.oid).collect();
        assert_eq!(distinct.len(), metadata.len());
        let blob = remote.fetch_blob(&selected)?;
        assert_eq!(blob.oid, selected);
        assert_eq!(blob.kind, "blob");
        assert_eq!(blob.data, b"# Selected\n");
        let tree = metadata.iter().find(|object| object.kind == "tree").unwrap();
        assert!(remote.fetch_blob(&tree.oid).is_err(), "non-blob object accepted as selected content");
        assert!(
            remote.fetch_blob("1111111111111111111111111111111111111111").is_err(),
            "rejected object want must fail without fallback"
        );
        Ok(serde_json::json!({
            "head": head,
            "metadata": metadata.iter().map(|object| serde_json::json!({
                "oid": object.oid, "kind": object.kind, "size": object.data.len()
            })).collect::<Vec<_>>(),
            "blob": {"oid": blob.oid, "kind": blob.kind, "bytes": String::from_utf8(blob.data).unwrap()}
        }))
    });
    match attempt {
        Ok(evidence) => {
            assert!(success, "filter-disabled peer unexpectedly accepted");
            fs::write(result, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
        Err(error) => {
            assert!(!success, "selective transport failed: {error}");
            let message = error.to_string();
            assert!(
                message.contains("filter") || message.contains("selective"),
                "unsupported source needs actionable diagnostics: {message}"
            );
            fs::write(
                result,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "unsupported": true, "error": message,
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }
}

pub(crate) fn http_server(
    repository: &Repository,
    auth: bool,
) -> (install_ctl_test_support::HttpFixture, String, PathBuf) {
    http_server_mode(repository, if auth { "auth" } else { "anonymous" })
}

pub(crate) fn http_server_mode(
    repository: &Repository,
    mode: &str,
) -> (install_ctl_test_support::HttpFixture, String, PathBuf) {
    let log = repository.root.path().join("requests.jsonl");
    let server = install_ctl_test_support::HttpFixture::new(repository.root.path(), log.clone(), mode);
    let url = server.url.clone();
    (server, url, log)
}

#[test]
fn local_upload_pack_transfers_metadata_and_only_requested_blob() {
    let repository = Repository::new(true);
    repository.probe(repository.repo.to_str().unwrap(), None, None, true);
}

#[test]
fn local_upload_pack_rejects_missing_filter_support() {
    let repository = Repository::new(false);
    repository.probe(repository.repo.to_str().unwrap(), None, None, false);
}

#[test]
fn git_protocol_transfers_only_metadata_and_selected_blob() {
    let repository = Repository::new(true);
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = Command::new("git")
        .args([
            "daemon",
            "--export-all",
            "--reuseaddr",
            "--listen=127.0.0.1",
        ])
        .arg(format!("--port={port}"))
        .arg(format!("--base-path={}", repository.root.path().display()))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start real Git protocol fixture");
    let _server = Server::new(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "git daemon did not become responsive"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    repository.probe(
        &format!("git://127.0.0.1:{port}/repo.git"),
        None,
        None,
        true,
    );
}

#[test]
fn smart_http_records_filtered_metadata_and_explicit_blob_want() {
    let repository = Repository::new(true);
    let (_server, url, log) = http_server(&repository, false);
    repository.probe(&url, None, None, true);
    let requests = fs::read_to_string(log).unwrap();
    assert!(requests.contains("filter blob:none"));
    assert!(requests.contains(&format!("want {}", repository.selected)));
    assert!(!requests.contains(&format!("want {}", repository.unrelated)));
}

#[test]
fn smart_http_without_filter_fails_before_any_pack_request() {
    let repository = Repository::new(false);
    let (_server, url, log) = http_server(&repository, false);
    repository.probe(&url, None, None, false);
    let requests = fs::read_to_string(log).unwrap();
    assert!(
        !requests.contains("want "),
        "unsupported peer received a pack request"
    );
}

#[test]
fn smart_http_uses_credential_helper_without_logging_credentials() {
    let repository = Repository::new(true);
    let (_server, url, log) = http_server(&repository, true);
    let config = repository.root.path().join("client.config");
    let helper = format!(
        "!{} credential {}",
        install_ctl_test_support::command_path(&install_ctl_test_support::helper_path()),
        install_ctl_test_support::command_path(&repository.root.path().join("credentials.jsonl"))
    );
    fs::write(&config, format!("[credential]\n\thelper = \"{helper}\"\n")).unwrap();
    repository.probe(&url, Some(&config), None, true);
    let requests = fs::read_to_string(log).unwrap();
    assert!(requests.contains("\"authorized\": true"));
    assert!(!requests.contains("fixture-password"));
    assert!(!requests.contains("Zml4dHVyZ"));
    let actions = fs::read_to_string(repository.root.path().join("credentials.jsonl")).unwrap();
    assert!(actions.contains("\"action\": \"get\""));
    assert!(actions.contains("\"action\": \"store\""));
    assert!(!actions.contains("fixture-password"));
}

#[test]
fn ssh_command_wrapper_forwards_to_real_upload_pack() {
    let repository = Repository::new(true);
    let log = repository.root.path().join("ssh.jsonl");
    let ssh = format!(
        "{} ssh {}",
        install_ctl_test_support::command_path(&install_ctl_test_support::helper_path()),
        install_ctl_test_support::command_path(&log)
    );
    let path = repository.repo.display().to_string().replace('\\', "/");
    repository.probe(
        &format!("ssh://fixture.invalid/{path}"),
        None,
        Some(&ssh),
        true,
    );
    let requests = fs::read_to_string(log).unwrap();
    assert!(requests.contains("git-upload-pack"));
    assert!(!requests.contains("fixture-password"));
}
