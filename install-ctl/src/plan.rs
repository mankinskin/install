use std::path::PathBuf;

use crate::registry::{
    Artifact, ArtifactKind, EntryCategory, EntryKind, LifecycleAction, RegistryEntryV2,
};
#[cfg(test)]
use crate::selection::resolve_selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallOptions {
    pub force: bool,
    pub offline: bool,
    pub locked: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            force: true,
            offline: false,
            locked: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanExclusion {
    pub id: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct InstallPlan {
    pub selected: Vec<Artifact>,
    pub root: PathBuf,
    pub options: InstallOptions,
    pub excluded: Vec<PlanExclusion>,
}

impl InstallPlan {
    #[cfg(test)]
    #[allow(dead_code)] // Imported by several focused integration fixtures.
    pub fn from_tokens(
        artifacts: &[Artifact],
        tokens: &[String],
        root: PathBuf,
        options: InstallOptions,
    ) -> Result<Self, String> {
        let selected = resolve_selection(artifacts, tokens)?;
        if selected.is_empty() {
            return Err("selection matched no artifacts".into());
        }
        Ok(Self {
            selected,
            root,
            options,
            excluded: vec![],
        })
    }

    pub fn from_inventory(
        entries: &[RegistryEntryV2],
        tokens: &[String],
        root: PathBuf,
        options: InstallOptions,
    ) -> Result<Self, String> {
        let mut selected = Vec::new();
        let mut excluded = Vec::new();
        for raw in tokens {
            for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                let matches: Vec<&RegistryEntryV2> = if token == "all" {
                    entries.iter().collect()
                } else {
                    let found: Vec<_> = entries
                        .iter()
                        .filter(|e| e.id == token || category_token(e.category) == token)
                        .collect();
                    if found.is_empty() {
                        return Err(format!("unknown artifact or category: {token}"));
                    }
                    found
                };
                for entry in matches {
                    if let Some(reason) = exclusion_reason(entry) {
                        if token == "all" {
                            if !excluded.iter().any(|x: &PlanExclusion| x.id == entry.id) {
                                excluded.push(PlanExclusion {
                                    id: entry.id.clone(),
                                    reason,
                                });
                            }
                        } else {
                            return Err(format!("cannot install '{}': {reason}", entry.id));
                        }
                    } else if !selected.iter().any(|a: &Artifact| a.id == entry.id) {
                        selected.push(Artifact {
                            id: entry.id.clone(),
                            category: category_token(entry.category).into(),
                            kind: ArtifactKind::RustBinary,
                            path: entry.source_path.clone(),
                            bin: entry.bin.clone(),
                            features: entry.features.clone(),
                            npm_script: entry.npm_script.clone(),
                            extension_id: None,
                        });
                    }
                }
            }
        }
        if selected.is_empty() {
            return Err("selection matched no artifacts".into());
        }
        Ok(Self {
            selected,
            root,
            options,
            excluded,
        })
    }
}

pub fn category_token(category: EntryCategory) -> &'static str {
    match category {
        EntryCategory::Cli => "cli",
        EntryCategory::Mcp => "mcp",
        EntryCategory::Service => "service",
        EntryCategory::VscodeExtension => "vscode-extension",
        EntryCategory::Hook => "hook",
        EntryCategory::Misc => "misc",
    }
}
pub fn exclusion_reason(entry: &RegistryEntryV2) -> Option<String> {
    match entry.kind {
        EntryKind::RustBinary if entry.lifecycle.contains(&LifecycleAction::Install) => None,
        EntryKind::Hook => Some("hook entries are inspect-only".into()),
        EntryKind::Script => Some("unsupported artifact kind: script".into()),
        EntryKind::ManagedService => Some("unsupported artifact kind: managed-service".into()),
        EntryKind::VscodeExtension => Some("unsupported artifact kind: vscode-extension".into()),
        EntryKind::RustBinary => Some("does not declare install lifecycle".into()),
    }
}
