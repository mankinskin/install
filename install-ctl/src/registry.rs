use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::installable::{InstallReport, Installable, UninstallReport};

#[derive(Debug, Deserialize)]
pub struct Registry {
    #[serde(rename = "artifact", default)]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Artifact {
    pub id: String,
    pub category: String,
    pub kind: ArtifactKind,
    pub path: String,
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub npm_script: Option<String>,
    // Not read yet: installation for vscode-extension artifacts is not wired up in this unit.
    #[serde(default)]
    #[allow(dead_code)]
    pub extension_id: Option<String>,
}

impl Installable for Artifact {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_installed(&self, target_root: &Path) -> Result<bool, String> {
        if let Some(bin_name) = &self.bin {
            let bin_dir = target_root.join("bin");
            let primary = bin_dir.join(bin_name);
            let exe = bin_dir.join(format!("{bin_name}.exe"));
            Ok(primary.is_file() || exe.is_file())
        } else {
            Ok(false)
        }
    }

    fn install(&self, target_root: &Path) -> Result<InstallReport, String> {
        let repo_root = resolve_repo_root()?;
        let source_dir = repo_root.join(&self.path);

        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("install")
            .arg("--force")
            .arg("--path")
            .arg(&source_dir)
            .arg("--root")
            .arg(target_root);

        if !self.features.is_empty() {
            cmd.arg("--features").arg(self.features.join(","));
        }
        if let Some(bin) = &self.bin {
            cmd.arg("--bin").arg(bin);
        }

        let status = cmd
            .status()
            .map_err(|e| format!("failed to execute cargo install for '{}': {e}", self.id))?;

        if !status.success() {
            return Err(format!("cargo install failed for artifact '{}'", self.id));
        }

        let bin_name = self.bin.as_deref().unwrap_or(&self.id);
        let installed_path = target_root.join("bin").join(bin_name);

        Ok(InstallReport {
            written: vec![crate::paths::disp(&installed_path)],
            unchanged: vec![],
        })
    }

    fn uninstall(&self, target_root: &Path) -> Result<UninstallReport, String> {
        let mut removed = Vec::new();
        let mut missing = Vec::new();

        if let Some(bin_name) = &self.bin {
            let bin_dir = target_root.join("bin");
            let candidates = [
                bin_dir.join(bin_name),
                bin_dir.join(format!("{bin_name}.exe")),
            ];

            let mut found = false;
            for candidate in &candidates {
                if candidate.is_file() {
                    found = true;
                    if let Err(e) = std::fs::remove_file(candidate) {
                        return Err(format!(
                            "failed to remove artifact binary '{}': {e}",
                            candidate.display()
                        ));
                    }
                    removed.push(crate::paths::disp(candidate));
                }
            }

            if !found {
                missing.push(crate::paths::disp(&bin_dir.join(bin_name)));
            }
        } else {
            missing.push(self.id.clone());
        }

        Ok(UninstallReport { removed, missing })
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    RustBinary,
    VscodeExtension,
}

const REGISTRY_RELATIVE_PATH: &str = "install/artifacts.toml";
const REPO_ROOT_ENV_VAR: &str = "INSTALL_CTL_REPO_ROOT";

/// Resolve the repo root without relying on the process cwd: an explicit env
/// override wins, otherwise walk up from the running executable's own path.
///
/// This is the single repo-root resolver for install-ctl: `resolve_repo_root`
/// below derives the root from this path, and `config::Config::load` (ported
/// from viewer-ctl, which used to search for `viewer-ctl.toml` independently)
/// reuses the same root to find `<root>/viewer-ctl.toml`.
pub fn resolve_registry_path() -> Result<PathBuf, String> {
    if let Ok(root) = std::env::var(REPO_ROOT_ENV_VAR) {
        let candidate = PathBuf::from(&root).join(REGISTRY_RELATIVE_PATH);
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(format!(
            "{REPO_ROOT_ENV_VAR}={root} does not contain {REGISTRY_RELATIVE_PATH}"
        ));
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(found) = find_upwards(&exe)
    {
        return Ok(found);
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = find_upwards(&cwd)
    {
        return Ok(found);
    }

    Err(format!(
        "could not locate {REGISTRY_RELATIVE_PATH} from the executable path, the current \
         directory, or {REPO_ROOT_ENV_VAR}"
    ))
}

fn find_upwards(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    loop {
        let candidate = dir.join(REGISTRY_RELATIVE_PATH);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

pub fn load_registry() -> Result<Registry, String> {
    let path = resolve_registry_path()?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let registry_v2 = parse_registry_v2(&text)
        .map_err(|errors| format!("failed to parse {}: {errors:?}", path.display()))?;
    Ok(adapt_registry_v2(registry_v2))
}

/// Render the canonical command catalog from the runtime registry and either
/// write it to the repository root or verify the committed projection.
pub fn sync_catalog(check: bool) -> Result<(), String> {
    let registry_path = resolve_registry_path()?;
    let text = std::fs::read_to_string(&registry_path)
        .map_err(|error| format!("failed to read {}: {error}", registry_path.display()))?;
    let registry = parse_registry_v2(&text)
        .map_err(|errors| format!("failed to parse {}: {errors:?}", registry_path.display()))?;
    let catalog_path = resolve_repo_root()?.join("COMMANDS.md");
    let rendered = render_catalog(&registry);
    if check {
        let committed = std::fs::read_to_string(&catalog_path)
            .map_err(|error| format!("failed to read {}: {error}", catalog_path.display()))?;
        if committed != rendered {
            return Err(format!(
                "{} is stale; run 'install-ctl catalog'",
                catalog_path.display()
            ));
        }
    } else {
        std::fs::write(&catalog_path, rendered)
            .map_err(|error| format!("failed to write {}: {error}", catalog_path.display()))?;
    }
    Ok(())
}

/// Migration adapter (spec 97322185-2bda-4f25-85f7-f975a0b3fbad): project the
/// v2 registry down to the legacy `Registry`/`Artifact` shape that `main.rs`
/// still consumes for install/list. Entries whose `EntryKind` has no legacy
/// counterpart (script, managed-service, hook) are simply absent from the
/// legacy view — they were never installable through this path before either.
fn adapt_registry_v2(registry: RegistryV2) -> Registry {
    let artifacts = registry
        .entries
        .into_iter()
        .filter_map(|entry| {
            let kind = match entry.kind {
                EntryKind::RustBinary => ArtifactKind::RustBinary,
                EntryKind::VscodeExtension => ArtifactKind::VscodeExtension,
                EntryKind::Script | EntryKind::ManagedService | EntryKind::Hook => return None,
            };
            Some(Artifact {
                id: entry.id,
                category: entry_category_str(entry.category).to_string(),
                kind,
                path: entry.source_path,
                bin: entry.bin,
                features: entry.features,
                npm_script: entry.npm_script,
                extension_id: None,
            })
        })
        .collect();
    Registry { artifacts }
}

/// Resolve the repo root as the parent of `install/artifacts.toml`.
pub fn resolve_repo_root() -> Result<PathBuf, String> {
    let registry_path = resolve_registry_path()?;
    registry_path
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "could not derive repo root from {}",
                registry_path.display()
            )
        })
}

// ===========================================================================
// Versioned registry schema (spec 97322185-2bda-4f25-85f7-f975a0b3fbad).
//
// `load_registry` above adapts a parsed `RegistryV2` down to the legacy
// `Registry`/`Artifact` shape for existing `main.rs` consumers.
// ===========================================================================

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EntryCategory {
    Cli,
    Mcp,
    Service,
    VscodeExtension,
    Hook,
    Misc,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EntryKind {
    RustBinary,
    Script,
    ManagedService,
    VscodeExtension,
    Hook,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleAction {
    Install,
    Start,
    Stop,
    Restart,
    Uninstall,
    Enable,
    Disable,
    Inspect,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Safety {
    Safe,
    ApprovalRequired,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RegistryEntryV2 {
    pub id: String,
    pub category: EntryCategory,
    pub kind: EntryKind,
    pub source_path: String,
    pub owner: String,
    pub lifecycle: Vec<LifecycleAction>,
    pub safety: Safety,
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub npm_script: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RegistryV2 {
    pub version: u32,
    #[serde(rename = "artifact", default)]
    pub entries: Vec<RegistryEntryV2>,
}

/// Every way `parse_registry_v2` can reject a registry document (AC1, AC4, AC5).
#[derive(Debug, PartialEq, Eq)]
pub enum RegistryValidationError {
    MalformedToml(String),
    UnsupportedVersion(u32),
    DuplicateId(String),
    InvalidSourcePath { id: String, path: String },
    MissingCommandMetadata { id: String },
    EmptyOwner { id: String },
    EmptyLifecycle { id: String },
    SafeEntryHasMutatingAction { id: String, action: LifecycleAction },
    HookEntryHasNonInspectAction { id: String, action: LifecycleAction },
}

/// Parse and fully validate the v2 registry schema against spec
/// 97322185-2bda-4f25-85f7-f975a0b3fbad's acceptance criteria.
pub fn parse_registry_v2(text: &str) -> Result<RegistryV2, Vec<RegistryValidationError>> {
    let registry: RegistryV2 = toml::from_str(text)
        .map_err(|e| vec![RegistryValidationError::MalformedToml(e.to_string())])?;

    let mut errors = Vec::new();

    if registry.version != REGISTRY_SCHEMA_VERSION {
        errors.push(RegistryValidationError::UnsupportedVersion(
            registry.version,
        ));
    }

    let mut seen_ids = std::collections::HashSet::new();
    for entry in &registry.entries {
        if !seen_ids.insert(entry.id.clone()) {
            errors.push(RegistryValidationError::DuplicateId(entry.id.clone()));
        }

        if !is_valid_source_path(&entry.source_path) {
            errors.push(RegistryValidationError::InvalidSourcePath {
                id: entry.id.clone(),
                path: entry.source_path.clone(),
            });
        }

        if entry.bin.is_none() && entry.npm_script.is_none() {
            errors.push(RegistryValidationError::MissingCommandMetadata {
                id: entry.id.clone(),
            });
        }

        if entry.owner.trim().is_empty() {
            errors.push(RegistryValidationError::EmptyOwner {
                id: entry.id.clone(),
            });
        }

        if entry.lifecycle.is_empty() {
            errors.push(RegistryValidationError::EmptyLifecycle {
                id: entry.id.clone(),
            });
        }

        for action in &entry.lifecycle {
            if entry.safety == Safety::Safe && *action != LifecycleAction::Inspect {
                errors.push(RegistryValidationError::SafeEntryHasMutatingAction {
                    id: entry.id.clone(),
                    action: *action,
                });
            }
            if (entry.kind == EntryKind::Hook || entry.category == EntryCategory::Hook)
                && *action != LifecycleAction::Inspect
            {
                errors.push(RegistryValidationError::HookEntryHasNonInspectAction {
                    id: entry.id.clone(),
                    action: *action,
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(registry)
    } else {
        Err(errors)
    }
}

/// A `source_path` is repository-local: relative, non-empty, and free of any
/// `..` traversal segment or platform-absolute prefix.
fn is_valid_source_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return false; // Windows drive-letter absolute path, e.g. `C:/...`.
    }
    path.split(['/', '\\']).all(|segment| segment != "..")
}

fn entry_category_str(category: EntryCategory) -> &'static str {
    match category {
        EntryCategory::Cli => "cli",
        EntryCategory::Mcp => "mcp",
        EntryCategory::Service => "service",
        EntryCategory::VscodeExtension => "vscode-extension",
        EntryCategory::Hook => "hook",
        EntryCategory::Misc => "misc",
    }
}

/// Deterministically render every entry and each of its declared lifecycle
/// actions into the root `COMMANDS.md` Markdown catalog (AC2, AC3). Entries
/// are rendered in registry (TOML) order, so re-rendering an unchanged
/// registry is always byte-identical.
pub fn render_catalog(registry: &RegistryV2) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "# Command & Hook Registry");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Generated from `install/artifacts.toml` (schema version {}). Do not edit by hand.",
        registry.version
    );

    for entry in &registry.entries {
        let _ = writeln!(out);
        let _ = writeln!(out, "## {}", entry.id);
        let _ = writeln!(out);
        let _ = writeln!(out, "- Category: {:?}", entry.category);
        let _ = writeln!(out, "- Kind: {:?}", entry.kind);
        let _ = writeln!(out, "- Source: `{}`", entry.source_path);
        let _ = writeln!(out, "- Owner: {}", entry.owner);
        let _ = writeln!(out, "- Safety: {:?}", entry.safety);
        if let Some(bin) = &entry.bin {
            let _ = writeln!(out, "- Bin: `{bin}`");
        }
        if let Some(npm_script) = &entry.npm_script {
            let _ = writeln!(out, "- Npm script: `{npm_script}`");
        }
        let actions = entry
            .lifecycle
            .iter()
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "- Lifecycle: {actions}");
    }

    out
}

#[cfg(test)]
mod registry_v2_tests {
    use super::*;

    // -- fixture builders ---------------------------------------------------

    fn valid_registry_toml() -> String {
        r#"
version = 1

[[artifact]]
id = "example-cli"
category = "cli"
kind = "rust-binary"
source_path = "tools/example/example-cli"
owner = "tooling"
lifecycle = ["inspect"]
safety = "safe"
bin = "example"

[[artifact]]
id = "example-service"
category = "service"
kind = "managed-service"
source_path = "tools/example/example-service"
owner = "tooling"
lifecycle = ["install", "start", "stop", "restart", "uninstall"]
safety = "approval-required"
bin = "example-service"
"#
        .to_string()
    }

    fn sample_registry_v2() -> RegistryV2 {
        RegistryV2 {
            version: REGISTRY_SCHEMA_VERSION,
            entries: vec![
                RegistryEntryV2 {
                    id: "example-cli".into(),
                    category: EntryCategory::Cli,
                    kind: EntryKind::RustBinary,
                    source_path: "tools/example/example-cli".into(),
                    owner: "tooling".into(),
                    lifecycle: vec![LifecycleAction::Inspect],
                    safety: Safety::Safe,
                    bin: Some("example".into()),
                    features: vec![],
                    npm_script: None,
                },
                RegistryEntryV2 {
                    id: "example-hook".into(),
                    category: EntryCategory::Hook,
                    kind: EntryKind::Hook,
                    source_path: "tools/hooks/example-hook".into(),
                    owner: "tooling".into(),
                    lifecycle: vec![LifecycleAction::Inspect],
                    safety: Safety::ApprovalRequired,
                    bin: None,
                    features: vec![],
                    npm_script: None,
                },
            ],
        }
    }

    // -- registry_schema_* (AC1) ---------------------------------------------

    #[test]
    fn registry_schema_parses_valid_v1_registry() {
        let registry = parse_registry_v2(&valid_registry_toml()).expect("valid registry parses");
        assert_eq!(registry.version, 1);
        assert_eq!(registry.entries.len(), 2);
        assert!(registry.entries.iter().any(|e| e.id == "example-cli"));
    }

    #[test]
    fn registry_schema_rejects_unsupported_version() {
        let text = valid_registry_toml().replacen("version = 1", "version = 2", 1);
        let errors = parse_registry_v2(&text).expect_err("unsupported version must be rejected");
        assert!(errors.contains(&RegistryValidationError::UnsupportedVersion(2)));
    }

    #[test]
    fn registry_schema_rejects_duplicate_ids() {
        let mut text = valid_registry_toml();
        text.push_str(
            r#"
[[artifact]]
id = "example-cli"
category = "cli"
kind = "rust-binary"
source_path = "tools/example/other-cli"
owner = "tooling"
lifecycle = ["inspect"]
safety = "safe"
bin = "other"
"#,
        );
        let errors = parse_registry_v2(&text).expect_err("duplicate id must be rejected");
        assert!(errors.contains(&RegistryValidationError::DuplicateId("example-cli".into())));
    }

    #[test]
    fn registry_schema_rejects_unsupported_category() {
        let text = valid_registry_toml().replacen(
            r#"category = "cli""#,
            r#"category = "not-a-real-category""#,
            1,
        );
        let errors = parse_registry_v2(&text).expect_err("unsupported category must be rejected");
        assert!(matches!(
            errors[0],
            RegistryValidationError::MalformedToml(_)
        ));
    }

    #[test]
    fn registry_schema_rejects_unsupported_kind() {
        let text = valid_registry_toml().replacen(
            r#"kind = "rust-binary""#,
            r#"kind = "not-a-real-kind""#,
            1,
        );
        let errors = parse_registry_v2(&text).expect_err("unsupported kind must be rejected");
        assert!(matches!(
            errors[0],
            RegistryValidationError::MalformedToml(_)
        ));
    }

    #[test]
    fn registry_schema_rejects_unsupported_lifecycle_action() {
        let text = valid_registry_toml().replacen(
            r#"lifecycle = ["inspect"]"#,
            r#"lifecycle = ["obliterate"]"#,
            1,
        );
        let errors =
            parse_registry_v2(&text).expect_err("unsupported lifecycle action must be rejected");
        assert!(matches!(
            errors[0],
            RegistryValidationError::MalformedToml(_)
        ));
    }

    #[test]
    fn registry_schema_rejects_unsupported_safety() {
        let text = valid_registry_toml().replacen(r#"safety = "safe""#, r#"safety = "yolo""#, 1);
        let errors =
            parse_registry_v2(&text).expect_err("unsupported safety value must be rejected");
        assert!(matches!(
            errors[0],
            RegistryValidationError::MalformedToml(_)
        ));
    }

    #[test]
    fn registry_schema_rejects_absolute_path() {
        let text = valid_registry_toml().replacen(
            r#"source_path = "tools/example/example-cli""#,
            r#"source_path = "/etc/example-cli""#,
            1,
        );
        let errors = parse_registry_v2(&text).expect_err("absolute path must be rejected");
        assert!(errors.iter().any(|e| matches!(e, RegistryValidationError::InvalidSourcePath { id, .. } if id == "example-cli")));
    }

    #[test]
    fn registry_schema_rejects_external_traversal_path() {
        let text = valid_registry_toml().replacen(
            r#"source_path = "tools/example/example-cli""#,
            r#"source_path = "../outside-repo/example-cli""#,
            1,
        );
        let errors = parse_registry_v2(&text).expect_err("traversal path must be rejected");
        assert!(errors.iter().any(|e| matches!(e, RegistryValidationError::InvalidSourcePath { id, .. } if id == "example-cli")));
    }

    #[test]
    fn registry_schema_rejects_missing_command_metadata() {
        let text = valid_registry_toml().replacen("bin = \"example\"\n", "", 1);
        let errors =
            parse_registry_v2(&text).expect_err("absent command metadata must be rejected");
        assert!(errors
            .iter()
            .any(|e| matches!(e, RegistryValidationError::MissingCommandMetadata { id } if id == "example-cli")));
    }

    #[test]
    fn registry_schema_rejects_empty_owner() {
        let text = valid_registry_toml().replacen(r#"owner = "tooling""#, r#"owner = """#, 1);
        let errors = parse_registry_v2(&text).expect_err("empty owner must be rejected");
        assert!(errors.iter().any(
            |e| matches!(e, RegistryValidationError::EmptyOwner { id } if id == "example-cli")
        ));
    }

    #[test]
    fn registry_schema_rejects_empty_lifecycle() {
        let text =
            valid_registry_toml().replacen(r#"lifecycle = ["inspect"]"#, "lifecycle = []", 1);
        let errors = parse_registry_v2(&text).expect_err("empty lifecycle must be rejected");
        assert!(errors.iter().any(
            |e| matches!(e, RegistryValidationError::EmptyLifecycle { id } if id == "example-cli")
        ));
    }

    // -- registry_catalog_render_* (AC2) --------------------------------------

    #[test]
    fn registry_catalog_render_includes_every_entry_and_action() {
        let registry = sample_registry_v2();
        let catalog = render_catalog(&registry);
        for entry in &registry.entries {
            assert!(
                catalog.contains(&entry.id),
                "catalog missing entry id {}",
                entry.id
            );
            for action in &entry.lifecycle {
                assert!(
                    catalog.contains(&format!("{action:?}"))
                        || catalog
                            .to_lowercase()
                            .contains(&format!("{action:?}").to_lowercase()),
                    "catalog missing action {action:?} for entry {}",
                    entry.id
                );
            }
        }
    }

    #[test]
    fn registry_catalog_render_is_deterministic() {
        let registry = sample_registry_v2();
        let first = render_catalog(&registry);
        let second = render_catalog(&registry);
        assert_eq!(
            first, second,
            "re-rendering an unchanged registry must be byte-identical"
        );
    }

    // -- registry_catalog_freshness_* (AC3) -----------------------------------

    #[test]
    fn registry_catalog_freshness_matches_committed_file() {
        let repo_root = resolve_repo_root().expect("repo root resolves");
        let registry_text = std::fs::read_to_string(repo_root.join(REGISTRY_RELATIVE_PATH))
            .expect("real artifacts.toml is readable");
        let registry =
            parse_registry_v2(&registry_text).expect("real artifacts.toml parses under v2 schema");
        let rendered = render_catalog(&registry);
        let committed = std::fs::read_to_string(repo_root.join("COMMANDS.md"))
            .expect("committed root COMMANDS.md must exist and be readable");
        assert_eq!(
            rendered, committed,
            "COMMANDS.md must byte-equal the freshly rendered catalog"
        );
    }

    #[test]
    fn registry_catalog_freshness_detects_mutation() {
        let base = sample_registry_v2();
        let mut mutated = sample_registry_v2();
        mutated.entries[0].lifecycle.push(LifecycleAction::Start);
        mutated.entries[0].safety = Safety::ApprovalRequired;

        let base_rendered = render_catalog(&base);
        let mutated_rendered = render_catalog(&mutated);
        assert_ne!(
            base_rendered, mutated_rendered,
            "a controlled registry mutation must change the rendered catalog"
        );
    }

    // -- registry_safety_* (AC4) -----------------------------------------------

    #[test]
    fn registry_safety_safe_entry_with_inspect_only_is_valid() {
        let registry = parse_registry_v2(&valid_registry_toml()).expect("valid registry parses");
        let safe_entry = registry
            .entries
            .iter()
            .find(|e| e.id == "example-cli")
            .unwrap();
        assert_eq!(safe_entry.safety, Safety::Safe);
        assert_eq!(safe_entry.lifecycle, vec![LifecycleAction::Inspect]);
    }

    #[test]
    fn registry_safety_safe_entry_with_mutating_action_is_rejected() {
        let text = valid_registry_toml().replacen(
            r#"lifecycle = ["inspect"]
safety = "safe""#,
            r#"lifecycle = ["install"]
safety = "safe""#,
            1,
        );
        let errors =
            parse_registry_v2(&text).expect_err("safe entries may not declare mutating actions");
        assert!(errors.iter().any(|e| matches!(
            e,
            RegistryValidationError::SafeEntryHasMutatingAction { id, action }
                if id == "example-cli" && *action == LifecycleAction::Install
        )));
    }

    #[test]
    fn registry_safety_approval_required_entry_with_mutating_actions_is_valid() {
        let registry = parse_registry_v2(&valid_registry_toml()).expect("valid registry parses");
        let approval_entry = registry
            .entries
            .iter()
            .find(|e| e.id == "example-service")
            .unwrap();
        assert_eq!(approval_entry.safety, Safety::ApprovalRequired);
        assert!(approval_entry.lifecycle.contains(&LifecycleAction::Install));
        assert!(
            approval_entry
                .lifecycle
                .contains(&LifecycleAction::Uninstall)
        );
    }

    // -- registry_hook_* (AC5) ---------------------------------------------------

    #[test]
    fn registry_hook_entry_with_inspect_only_is_valid() {
        let registry = sample_registry_v2();
        let hook = registry
            .entries
            .iter()
            .find(|e| e.category == EntryCategory::Hook)
            .unwrap();
        assert_eq!(hook.lifecycle, vec![LifecycleAction::Inspect]);
    }

    #[test]
    fn registry_hook_entry_with_install_is_rejected() {
        let text = valid_registry_toml()
            + r#"
[[artifact]]
id = "example-hook"
category = "hook"
kind = "hook"
source_path = "tools/hooks/example-hook"
owner = "tooling"
lifecycle = ["install"]
safety = "approval-required"
"#;
        let errors = parse_registry_v2(&text).expect_err("hook entries may not declare install");
        assert!(errors.iter().any(|e| matches!(
            e,
            RegistryValidationError::HookEntryHasNonInspectAction { id, action }
                if id == "example-hook" && *action == LifecycleAction::Install
        )));
    }

    #[test]
    fn registry_hook_kind_with_non_hook_category_rejects_mutating_action() {
        let text = valid_registry_toml()
            + r#"
[[artifact]]
id = "hook-kind-misc-category"
category = "misc"
kind = "hook"
source_path = "tools/hooks/hook-kind-misc-category"
owner = "tooling"
lifecycle = ["install"]
safety = "approval-required"
"#;
        let errors = parse_registry_v2(&text).expect_err("hook kinds may not declare install");
        assert!(errors.iter().any(|e| matches!(
            e,
            RegistryValidationError::HookEntryHasNonInspectAction { id, action }
                if id == "hook-kind-misc-category" && *action == LifecycleAction::Install
        )));
    }

    #[test]
    fn registry_hook_entry_with_enable_disable_is_rejected() {
        let text = valid_registry_toml()
            + r#"
[[artifact]]
id = "example-hook"
category = "hook"
kind = "hook"
source_path = "tools/hooks/example-hook"
owner = "tooling"
lifecycle = ["enable", "disable"]
safety = "approval-required"
"#;
        let errors =
            parse_registry_v2(&text).expect_err("hook entries may not declare enable/disable");
        assert!(errors.iter().any(|e| matches!(
            e,
            RegistryValidationError::HookEntryHasNonInspectAction { id, .. } if id == "example-hook"
        )));
    }
}
