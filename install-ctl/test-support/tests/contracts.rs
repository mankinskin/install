use install_ctl_test_support::helper_path;
use std::{fs, process::Command};
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use install_ctl_test_support::HttpFixture;

fn repository(root: &Path) {
    assert!(
        Command::new("git")
            .args(["init", "--bare", "--quiet"])
            .arg(root.join("repo.git"))
            .status()
            .expect("initialize real fixture repository")
            .success()
    );
}

fn address(fixture: &HttpFixture) -> &str {
    fixture
        .url
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
}

fn request(fixture: &HttpFixture, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(address(fixture))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(bytes)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(response)
}

#[test]
fn credential_helper_preserves_raw_protocol_and_action_only_evidence() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("credentials.jsonl");
    for action in ["get", "store", "erase"] {
        let mut child = Command::new(helper_path())
            .arg("credential")
            .arg(&log)
            .arg(action)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                b"protocol=http\nhost=localhost\nusername=private-user\npassword=private-value\n\n",
            )
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{:?}", output);
        let expected: &[u8] = if action == "get" {
            b"username=fixture-user\npassword=fixture-password\n\n"
        } else {
            b""
        };
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty());
    }
    let evidence = fs::read_to_string(log).unwrap();
    let actions: Vec<_> = evidence
        .lines()
        .map(|line| {
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(record.as_object().unwrap().len(), 1);
            record["action"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(actions, ["get", "store", "erase"]);
    assert!(evidence.contains("\"action\": \"get\""));
    assert!(!evidence.contains("private-"));
    assert!(!evidence.contains("fixture-password"));
}

#[test]
fn ssh_helper_logs_control_characters_as_valid_json() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("ssh.jsonl");
    let command = format!(
        "git-upload-pack '{}'",
        root.path().join("missing\u{1b}repository").display()
    );
    let output = Command::new(helper_path())
        .arg("ssh")
        .arg(&log)
        .arg("fixture-host")
        .arg(&command)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "missing repository must not succeed"
    );
    let evidence = fs::read_to_string(log).unwrap();
    let record: serde_json::Value =
        serde_json::from_str(evidence.trim()).expect("SSH evidence must be valid JSON");
    assert_eq!(
        record["arguments"],
        serde_json::json!(["fixture-host", command])
    );
}

#[test]
fn http_fixture_preserves_request_body_bytes_in_structured_evidence() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("requests.jsonl");
    let fixture = HttpFixture::new(root.path(), log.clone(), "auth");
    let body = r#"{"key":"value","other":"left:right"}"#;
    let bytes = format!(
        "POST /repo.git/git-upload-pack HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let response = request(&fixture, bytes.as_bytes()).unwrap();
    assert!(response.starts_with(b"HTTP/1.1 401"));
    drop(fixture);
    let evidence = fs::read_to_string(log).unwrap();
    let record: serde_json::Value = serde_json::from_str(evidence.trim()).unwrap();
    assert_eq!(record["body"], body, "logging must not rewrite payloads");
}

#[test]
fn http_auth_requires_the_fixture_credentials_not_just_a_header() {
    let root = tempfile::tempdir().unwrap();
    repository(root.path());
    let fixture = HttpFixture::new(root.path(), root.path().join("requests.jsonl"), "auth");
    for authorization in ["Bearer arbitrary", "Basic aW52YWxpZDppbnZhbGlk"] {
        let bytes = format!(
            "GET /repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {authorization}\r\n\r\n"
        );
        let response = request(&fixture, bytes.as_bytes()).unwrap();
        assert!(
            response.starts_with(b"HTTP/1.1 401"),
            "invalid credentials must produce an authentication challenge"
        );
        assert!(
            String::from_utf8_lossy(&response)
                .to_ascii_lowercase()
                .contains("www-authenticate: basic")
        );
    }
}

#[test]
fn stalled_fetch_does_not_block_handshakes_or_owned_shutdown() {
    let root = tempfile::tempdir().unwrap();
    repository(root.path());
    let log = root.path().join("requests.jsonl");
    let fixture = HttpFixture::new(root.path(), log.clone(), "stall-fetch");
    let endpoint = address(&fixture).to_owned();
    let mut stalled = TcpStream::connect(&endpoint).unwrap();
    let body = "command=fetch\n";
    write!(
        stalled,
        "POST /repo.git/git-upload-pack HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("command=fetch")
    {
        assert!(
            Instant::now() < deadline,
            "fixture did not enter fetch stall"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let response = request(
        &fixture,
        b"GET /repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    let shutdown = Instant::now();
    drop(fixture);
    assert!(shutdown.elapsed() < Duration::from_secs(2));
    drop(stalled);
    assert!(
        response.is_ok(),
        "a stalled fetch must not block a concurrent handshake: {response:?}"
    );
    assert!(response.unwrap().starts_with(b"HTTP/1.1 200"));
    assert!(
        TcpStream::connect(endpoint).is_err(),
        "fixture listener survived"
    );
}

#[test]
fn git_backend_receives_protocol_headers_and_decoded_chunked_body() {
    let root = tempfile::tempdir().unwrap();
    repository(root.path());
    let log = root.path().join("requests.jsonl");
    let fixture = HttpFixture::new(root.path(), log.clone(), "normal");
    let advertisement = request(
        &fixture,
        b"GET /repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nGit-Protocol: version=2\r\n\r\n",
    )
    .unwrap();
    assert!(advertisement.starts_with(b"HTTP/1.1 200"));
    assert!(String::from_utf8_lossy(&advertisement).contains("version 2\n"));
    let command = "command=ls-refs\n";
    let body = format!("{:04x}{command}00010000", command.len() + 4);
    let bytes = format!(
        "POST /repo.git/git-upload-pack HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nGit-Protocol: version=2\r\nX-Transport-Fixture: sentinel\r\nContent-Type: application/x-git-upload-pack-request\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
        body.len()
    );
    let response = request(&fixture, bytes.as_bytes()).unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    assert!(
        String::from_utf8_lossy(&response)
            .to_ascii_lowercase()
            .contains("content-type: application/x-git-upload-pack-result")
    );
    drop(fixture);
    let evidence = fs::read_to_string(log).unwrap();
    let record: serde_json::Value = serde_json::from_str(evidence.lines().last().unwrap()).unwrap();
    assert_eq!(record["body"], body);
    assert_eq!(record["fixture_header"], "sentinel");
}
