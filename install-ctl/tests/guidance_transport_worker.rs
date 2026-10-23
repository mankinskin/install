mod guidance {
    pub mod remote {
        pub mod transport {
            use serde_json::{Value, json};
            use std::{
                fs,
                path::Path,
                process::{Command, Stdio},
                time::{Duration, Instant},
            };

            fn git(repo: &Path, args: &[&str]) -> String {
                let output = Command::new("git")
                    .current_dir(repo)
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap().trim().to_owned()
            }

            fn exchange(repo: &Path, operation: Value) -> Value {
                let directory = tempfile::tempdir().unwrap();
                fs::write(
                    directory.path().join("request.json"),
                    serde_json::to_vec(&json!({
                        "source": repo, "operation": operation,
                    }))
                    .unwrap(),
                )
                .unwrap();
                fs::write(directory.path().join("start"), []).unwrap();
                let mut child = Command::new(env!("CARGO_BIN_EXE_install-ctl"))
                    .arg("--__guidance-object-worker")
                    .env("INSTALL_CTL_OBJECT_WORKER", directory.path())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                let status = loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        break status;
                    }
                    if Instant::now() >= deadline {
                        #[cfg(windows)]
                        {
                            let cancelled = Command::new("taskkill")
                                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .status()
                                .unwrap();
                            assert!(
                                cancelled.success(),
                                "cannot cancel owned production-worker fixture"
                            );
                        }
                        #[cfg(not(windows))]
                        child.kill().unwrap();
                        child.wait().unwrap();
                        panic!("production worker exceeded ten-second local-fixture budget");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert!(status.success());
                let result: Value = serde_json::from_slice(
                    &fs::read(directory.path().join("response.json")).unwrap(),
                )
                .unwrap();
                assert!(result.get("Ok").is_some(), "{result}");
                result["Ok"].clone()
            }

            #[test]
            fn production_worker_routes_head_metadata_and_blob_without_clap() {
                let repository = tempfile::tempdir().unwrap();
                let repo = repository.path();
                git(repo, &["init", "--quiet"]);
                git(repo, &["config", "user.name", "Worker Fixture"]);
                git(repo, &["config", "user.email", "fixture@example.invalid"]);
                git(repo, &["config", "uploadpack.allowFilter", "true"]);
                git(repo, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
                fs::write(repo.join("selected.md"), "# Selected\n").unwrap();
                fs::write(repo.join("unrelated.bin"), vec![0x5a; 256 * 1024]).unwrap();
                git(repo, &["add", "."]);
                git(repo, &["commit", "--quiet", "-m", "fixture"]);
                let head = git(repo, &["rev-parse", "HEAD"]);
                let selected = git(repo, &["rev-parse", "HEAD:selected.md"]);
                assert_eq!(exchange(repo, json!("Head"))["Head"], head);
                let metadata = exchange(repo, json!({"Metadata": head}));
                let metadata = metadata["Objects"].as_array().unwrap();
                assert_eq!(metadata.len(), 2);
                assert!(metadata.iter().all(|object| object["kind"] != "blob"));
                let blob = exchange(repo, json!({"Blob": selected}));
                let objects = blob["Objects"].as_array().unwrap();
                assert_eq!(objects.len(), 1);
                assert_eq!(objects[0]["oid"], selected);
                assert_eq!(objects[0]["kind"], "blob");
                let bytes: Vec<u8> = serde_json::from_value(objects[0]["data"].clone()).unwrap();
                assert_eq!(bytes, b"# Selected\n");
            }
        }
    }
}
