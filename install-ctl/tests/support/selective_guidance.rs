use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub fn write(root: &Path, path: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

pub fn init_repo(repo: &Path, filter: bool) {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "--quiet"]);
    for (key, value) in [
        ("user.name", "CLI Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("uploadpack.allowAnySHA1InWant", "true"),
    ] {
        git(repo, &["config", key, value]);
    }
    git(
        repo,
        &[
            "config",
            "uploadpack.allowFilter",
            if filter { "true" } else { "false" },
        ],
    );
    write(repo, "selected.md", "[dep](deps/base.md)\n");
    write(repo, "deps/base.md", "# Base\n");
    write(repo, "unrelated.bin", vec![0x5a; 256 * 1024]);
    commit(repo);
}

pub fn commit(repo: &Path) {
    git(repo, &["add", "-A"]);
    git(
        repo,
        &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
    );
}

pub fn stage_link(repo: &Path, path: &str, target: &str) {
    write(repo, "link-input", target);
    let oid = git(repo, &["hash-object", "-w", "link-input"]);
    fs::remove_file(repo.join("link-input")).unwrap();
    git(
        repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{oid},{path}"),
        ],
    );
}

pub struct Run {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn assert_success(&self) {
        assert!(
            self.status.success(),
            "stdout: {}\nstderr: {}",
            self.stdout,
            self.stderr
        );
    }
    pub fn kept_path(&self) -> PathBuf {
        self.stdout
            .lines()
            .chain(self.stderr.lines())
            .find_map(|line| line.strip_prefix("checkout kept at: "))
            .map(PathBuf::from)
            .expect("CLI must name retained snapshot")
    }
}

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub repo: PathBuf,
    pub url: String,
    pub log: PathBuf,
    _server: install_ctl_test_support::HttpFixture,
    caches: Vec<(PathBuf, bool)>,
}

impl Fixture {
    pub fn new(filter: bool, mutable_access: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo.git");
        init_repo(&repo, filter);
        let log = root.path().join("requests.jsonl");
        let server = install_ctl_test_support::HttpFixture::new(
            root.path(),
            log.clone(),
            if mutable_access { "mutable-access" } else { "none" },
        );
        let url = server.url.clone();
        let mut fixture = Self {
            root,
            repo,
            url: url.clone(),
            log,
            _server: server,
            caches: Vec::new(),
        };
        fixture.cache(&url);
        fixture
    }
    pub fn cache(&mut self, url: &str) -> PathBuf {
        let directory = dirs::cache_dir()
            .unwrap()
            .join("workflow-tools/install-ctl/guidance/v1")
            .join(format!("{:x}", Sha256::digest(url.as_bytes())));
        if !self.caches.iter().any(|(path, _)| *path == directory) {
            let existed = directory.exists();
            self.caches.push((directory.clone(), existed));
        }
        directory
    }
    pub fn invoke(&self, args: &[&str]) -> Run {
        let target = self.root.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_install-ctl"));
        command
            .args(["guidance", "get", &self.url, "--target"])
            .arg(target)
            .args(args);
        self.execute(command)
    }
    pub fn execute(&self, mut command: Command) -> Run {
        let temporary = self.root.path().join("temporary");
        fs::create_dir_all(&temporary).unwrap();
        let stdout = self.root.path().join("cli.stdout");
        let stderr = self.root.path().join("cli.stderr");
        command
            .env("TMP", &temporary)
            .env("TEMP", &temporary)
            .env("TMPDIR", &temporary)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_GLOBAL", self.root.path().join("absent.config"))
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap());
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(45);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                #[cfg(windows)]
                {
                    assert!(
                        Command::new("taskkill")
                            .args(["/PID", &child.id().to_string(), "/T", "/F"])
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                #[cfg(not(windows))]
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("owned CLI fixture exceeded its 45-second budget");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Run {
            status,
            stdout: fs::read_to_string(stdout).unwrap(),
            stderr: fs::read_to_string(stderr).unwrap(),
        }
    }
    pub fn requests(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub fn wants(&self, oid: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| {
                request["body"]
                    .as_str()
                    .is_some_and(|body| body.contains(&format!("want {oid}")))
            })
            .count()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (directory, existed) in &self.caches {
            if !existed && directory.is_dir() {
                fs::remove_dir_all(directory).unwrap();
            }
        }
    }
}
