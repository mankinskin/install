//! Config loader for `viewer-ctl.toml` (ported from viewer-ctl).
//!
//! All paths in the file are relative to the repo root, which install-ctl
//! resolves via `registry::resolve_repo_root` (see that module's doc
//! comment for why the two resolvers were unified onto one root).
//!
//! See `viewer-ctl.toml` at the repo root for the schema and live data.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

// ── Top-level ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, rename = "server")]
    pub servers: Vec<Server>,
    #[serde(default, rename = "frontend")]
    pub frontends: Vec<Frontend>,
    #[serde(default, rename = "extension")]
    pub extensions: Vec<Extension>,
    #[serde(default, rename = "task")]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Defaults {
    /// Where installed frontend bundles live.  Supports a leading `~`.
    #[serde(default = "default_install_root")]
    pub frontend_install_root: String,
}

#[allow(dead_code)]
fn default_install_root() -> String {
    "~/.context-engine/static".to_string()
}

// ── Component types ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct Server {
    pub name: String,
    pub package: String,
    pub port: u16,
    pub source_dir: String,
    #[serde(default)]
    pub start_args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Frontend {
    pub name: String,
    /// Name of the [[server]] this frontend is served by, used to derive the
    /// `STATIC_DIR` env var passed to the server at start time.
    #[serde(default)]
    pub serves: Option<String>,
    pub source_dir: String,
    pub build_cmd: Vec<String>,
    pub build_output: String,
    /// Additional directories whose contents are copied into the install
    /// dir after the build (e.g. `public/` assets that trunk skipped).
    #[serde(default)]
    pub extra_assets: Vec<String>,
    #[serde(default)]
    pub prebuild: Vec<PrebuildStep>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PrebuildStep {
    pub dir: String,
    pub cmd: Vec<String>,
    /// Optional condition. Currently supported:
    ///   `missing:<relpath>`  → run only if `<dir>/<relpath>` does not exist.
    #[serde(default)]
    pub condition: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Extension {
    pub name: String,
    /// Built-in installer kind. Currently `"vscode"` is recognised.
    pub kind: String,
    pub source_dir: String,
    #[serde(default = "default_pkg_json")]
    pub package_json: String,
    pub build_cmd: Vec<String>,
}

#[allow(dead_code)]
fn default_pkg_json() -> String {
    "package.json".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct Task {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub steps: Vec<TaskStep>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TaskStep {
    pub dir: String,
    pub cmd: Vec<String>,
    #[serde(default)]
    pub allow_failure: bool,
}

// ── Lookup helpers ───────────────────────────────────────────────────────────

/// Resolved component reference returned by [`Config::lookup`].
pub enum Component<'a> {
    Server(&'a Server),
    Frontend(&'a Frontend),
    Extension(&'a Extension),
}

impl Config {
    /// Load the config from `<repo_root>/viewer-ctl.toml`.
    pub fn load(repo_root: &Path) -> Result<Self, String> {
        let path = repo_root.join("viewer-ctl.toml");
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", crate::paths::disp(&path)))?;
        let cfg: Config = toml::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", crate::paths::disp(&path)))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        // Each frontend.serves must reference a known server.
        for fe in &self.frontends {
            if let Some(s) = &fe.serves
                && !self.servers.iter().any(|sv| &sv.name == s)
            {
                return Err(format!(
                    "frontend `{}` declares serves=`{s}` but no [[server]] with that name exists",
                    fe.name
                ));
            }
        }
        Ok(())
    }

    /// Find a component by name. Searches servers, then frontends, then
    /// extensions. Returns `None` if no match.  Components of different
    /// kinds may share a name (e.g. server + frontend with the same name);
    /// callers needing to disambiguate should use the typed accessors.
    #[allow(dead_code)]
    pub fn lookup(&self, name: &str) -> Option<Component<'_>> {
        if let Some(s) = self.server(name) {
            return Some(Component::Server(s));
        }
        if let Some(f) = self.frontend(name) {
            return Some(Component::Frontend(f));
        }
        if let Some(e) = self.extension(name) {
            return Some(Component::Extension(e));
        }
        None
    }

    pub fn server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    pub fn frontend(&self, name: &str) -> Option<&Frontend> {
        self.frontends.iter().find(|f| f.name == name)
    }

    pub fn extension(&self, name: &str) -> Option<&Extension> {
        self.extensions.iter().find(|e| e.name == name)
    }

    pub fn task(&self, name: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.name == name)
    }

    /// Find the frontend (if any) whose `serves` field links it to `server_name`.
    pub fn frontend_for_server(&self, server_name: &str) -> Option<&Frontend> {
        self.frontends
            .iter()
            .find(|f| f.serves.as_deref() == Some(server_name))
    }

    /// Resolve the install root for frontends, expanding `~`.
    pub fn frontend_install_root(&self) -> PathBuf {
        expand_tilde(&self.defaults.frontend_install_root)
    }

    /// Resolve the install dir for one specific frontend.
    pub fn frontend_install_dir(&self, frontend_name: &str) -> PathBuf {
        self.frontend_install_root().join(frontend_name)
    }
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}
