use super::super::{Operation, ReceivedObject, Request, objects, process};
use super::{Repository, SelectiveTransport, fs, http_server, http_server_mode};
use std::time::{Duration, Instant};

fn assert_handshake_error_is_visible(auth: bool) {
    let repository = Repository::new(true);
    let (_server, url, _log) = http_server(&repository, auth);
    let url = url.replace("/repo.git", "/missing.git");
    let config = repository.root.path().join("client.config");
    let helper = format!(
        "!{} credential {}",
        install_ctl_test_support::command_path(&install_ctl_test_support::helper_path()),
        install_ctl_test_support::command_path(&repository.root.path().join("credentials.jsonl"))
    );
    fs::write(&config, format!("[credential]\n\thelper = \"{helper}\"\n")).unwrap();
    repository.probe(&url, Some(&config), None, false);
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(repository.root.path().join("objects.json")).unwrap())
            .unwrap();
    let error = result["error"].as_str().unwrap();
    assert!(error.contains("404"), "HTTP cause was discarded: {error}");
    assert!(!error.contains("fixture-password"), "{error}");
}

#[test]
fn initial_handshake_error_preserves_http_status() {
    assert_handshake_error_is_visible(false);
}

#[test]
fn authenticated_handshake_error_preserves_http_status() {
    assert_handshake_error_is_visible(true);
}

#[test]
fn sha256_upload_pack_verifies_metadata_and_selected_blob() {
    let repository = Repository::with_format(true, "sha256");
    repository.probe(repository.repo.to_str().unwrap(), None, None, true);
}

#[test]
fn unsupported_tls_constraints_fail_before_connecting_instead_of_being_ignored() {
    let repository = Repository::new(true);
    let (_server, url, log) = http_server(&repository, false);
    let config = repository.root.path().join("client.config");
    fs::write(&config, "[http]\n\tpinnedPubkey = fixture-pin\n").unwrap();
    repository.probe(&url, Some(&config), None, false);
    assert!(
        !log.exists(),
        "unsupported TLS configuration contacted the server"
    );
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(repository.root.path().join("objects.json")).unwrap())
            .unwrap();
    assert!(result["error"].as_str().unwrap().contains("pinnedpubkey"));
    assert!(!result["error"].as_str().unwrap().contains("fixture-pin"));
}

#[test]
fn smart_http_honors_url_rewriting_and_scoped_extra_headers() {
    let repository = Repository::new(true);
    let (_server, url, log) = http_server(&repository, false);
    let config = repository.root.path().join("client.config");
    fs::write(&config, format!(
        "[url \"{url}\"]\n\tinsteadOf = fixture://source\n[http]\n\textraHeader = X-Transport-Fixture: wrong-scope\n[http \"{url}\"]\n\textraHeader = X-Transport-Fixture: discarded\n\textraHeader =\n\textraHeader = X-Transport-Fixture: configured\n\textraHeader = X-Transport-Second: second\n"
    )).unwrap();
    repository.probe("fixture://source", Some(&config), None, true);
    let requests = fs::read_to_string(log).unwrap();
    assert!(requests.lines().all(|line| {
        let request: serde_json::Value = serde_json::from_str(line).unwrap();
        request.get("cgi_exit").is_some()
            || (request["fixture_header"] == "configured" && request["second_header"] == "second")
    }));
}

#[test]
fn handshake_timeout_cancels_the_owned_worker() {
    let repository = Repository::new(true);
    let (_server, url, log) = http_server_mode(&repository, "stall-handshake");
    let started = Instant::now();
    let error = process::run_with_limits(
        Request {
            source: url,
            operation: Operation::Head,
        },
        Duration::from_secs(1),
        Duration::from_secs(5),
    )
    .err()
    .expect("stalled handshake must fail");
    assert!(error.contains("connection exceeded"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        fs::read_to_string(log)
            .unwrap()
            .contains("\"method\": \"GET\"")
    );
}

#[test]
fn fetch_timeout_cancels_the_owned_worker() {
    let repository = Repository::new(true);
    let (_server, url, log) = http_server_mode(&repository, "stall-fetch");
    let started = Instant::now();
    let error = process::run_with_limits(
        Request {
            source: url,
            operation: Operation::Metadata(repository.head.clone()),
        },
        Duration::from_secs(5),
        Duration::from_secs(1),
    )
    .err()
    .expect("stalled fetch must fail");
    assert!(error.contains("fetch interaction exceeded"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(fs::read_to_string(log).unwrap().contains("command=fetch"));
}

#[test]
fn received_object_hash_mismatch_is_rejected() {
    let repository = Repository::new(true);
    let object = ReceivedObject {
        oid: repository.selected,
        kind: "blob".into(),
        data: b"not the selected content".to_vec(),
    };
    assert!(
        objects::verify(&object)
            .unwrap_err()
            .contains("hash mismatch")
    );
}

#[test]
fn metadata_rejects_missing_trees_duplicates_and_unsolicited_objects() {
    let repository = Repository::new(true);
    let mut remote = SelectiveTransport::connect(repository.repo.to_str().unwrap()).unwrap();
    let metadata = remote.fetch_metadata(&repository.head).unwrap();
    let mut duplicate = metadata.clone();
    duplicate.push(metadata[0].clone());
    assert!(objects::validate_metadata(&duplicate, &repository.head).is_err());
    let mut missing = metadata.clone();
    missing.retain(|object| object.kind != "tree");
    assert!(objects::validate_metadata(&missing, &repository.head).is_err());
    let mut unsolicited = metadata;
    unsolicited.push(remote.fetch_blob(&repository.selected).unwrap());
    assert!(objects::validate_metadata(&unsolicited, &repository.head).is_err());
}
