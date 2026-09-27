use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::registry::{Artifact, load_registry};
use crate::selection::resolve_selection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceMcpConfigReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub path: PathBuf,
}

pub fn run_mcp_config(workspace: &Path, selection_tokens: &[String], dry_run: bool) -> Result<(), String> {
    let reg = load_registry()?;
    let mut selected = filter_workspace_mcp_artifacts(resolve_selection(&reg.artifacts, selection_tokens)?);

    if selected.is_empty() {
        return Err("selection matched no artifacts".to_string());
    }

    let workspace_entries = selected
        .iter()
        .filter_map(|artifact| build_server_entry(artifact).map(|entry| (artifact.id.clone(), entry)))
        .collect::<BTreeMap<_, _>>();

    if workspace_entries.is_empty() {
        return Err("selection matched no MCP artifacts to configure".to_string());
    }

    let config_path = workspace.join(".vscode").join("mcp.json");
    let mut existing: serde_json::Value = if config_path.exists() {
        let text = fs::read_to_string(&config_path)
            .map_err(|e| format!("failed to read {}: {e}", config_path.display()))?;
        serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", config_path.display()))?
    } else {
        serde_json::json!({ "servers": {} })
    };

    let servers = if existing.get("servers").is_none() {
        existing["servers"] = serde_json::json!({});
        existing.get_mut("servers").and_then(|v| v.as_object_mut()).unwrap()
    } else {
        existing
            .get_mut("servers")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| format!("{} does not contain a JSON object at top-level 'servers'", config_path.display()))?
    };

    let mut report = WorkspaceMcpConfigReport {
        added: Vec::new(),
        updated: Vec::new(),
        unchanged: Vec::new(),
        path: config_path.clone(),
    };

    for (id, entry) in workspace_entries {
        let before = servers.get(&id).cloned();
        match before {
            Some(current) if current == entry => {
                report.unchanged.push(id.clone());
            }
            _ => {
                servers.insert(id.clone(), entry.clone());
                if before.is_some() {
                    report.updated.push(id.clone());
                } else {
                    report.added.push(id.clone());
                }
            }
        }
    }

    let rendered = serde_json::to_string_pretty(&existing)
        .map_err(|e| format!("failed to serialize JSON for {}: {e}", config_path.display()))?;

    if dry_run {
        println!("workspace={}", workspace.display());
        println!("config={}", config_path.display());
        println!("added={}", report.added.join(", "));
        println!("updated={}", report.updated.join(", "));
        println!("unchanged={}", report.unchanged.join(", "));
        println!("dry-run: no files written");
        return Ok(());
    }

    if !config_path.parent().is_some_and(|parent| parent.exists()) {
        fs::create_dir_all(config_path.parent().unwrap())
            .map_err(|e| format!("failed to create {}: {e}", config_path.parent().unwrap().display()))?;
    }

    fs::write(&config_path, format!("{rendered}\n"))
        .map_err(|e| format!("failed to write {}: {e}", config_path.display()))?;

    println!("workspace={}", workspace.display());
    println!("config={}", config_path.display());
    println!("added={}", report.added.join(", "));
    println!("updated={}", report.updated.join(", "));
    println!("unchanged={}", report.unchanged.join(", "));
    Ok(())
}

fn filter_workspace_mcp_artifacts(mut artifacts: Vec<Artifact>) -> Vec<Artifact> {
    artifacts.retain(|artifact| artifact.category == "mcp" || artifact.id == "log-viewer");
    artifacts
}

fn build_server_entry(artifact: &Artifact) -> Option<serde_json::Value> {
    if artifact.category != "mcp" && artifact.id != "log-viewer" {
        return None;
    }

    let mut args = vec!["--", artifact.id.as_str()];
    if artifact.id == "log-viewer" {
        args = vec!["--", "log-viewer", "--mcp"]; 
    }

    Some(serde_json::json!({
        "type": "stdio",
        "command": "mcp-toolmon",
        "args": args,
        "env": {
            "COST_GATE_TABLE": "${workspaceFolder}/workflow-tools/session/crates/model-prices/model_prices.json",
            "COST_GATE_TOOL_METRICS": "${workspaceFolder}/.session/tool-metrics-rollup.json",
            "COST_GATE_GRANTS_DIR": "${workspaceFolder}/.session/grants",
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_registry_mcp_entries() {
        let registry = crate::registry::Registry {
            artifacts: vec![
                Artifact { id: "ticket-mcp".into(), category: "mcp".into(), kind: crate::registry::ArtifactKind::RustBinary, path: "ticket".into(), bin: Some("ticket-mcp".into()), features: vec!["mcp".into()], npm_script: None, extension_id: None },
                Artifact { id: "log-viewer".into(), category: "service".into(), kind: crate::registry::ArtifactKind::RustBinary, path: "log/crates/log-viewer".into(), bin: Some("log-viewer".into()), features: vec![], npm_script: None, extension_id: None },
                Artifact { id: "context-mcp".into(), category: "misc".into(), kind: crate::registry::ArtifactKind::RustBinary, path: "context".into(), bin: Some("context-mcp".into()), features: vec![], npm_script: None, extension_id: None },
            ],
        };

        let selected = filter_workspace_mcp_artifacts(resolve_selection(&registry.artifacts, &["all".to_string()]).unwrap());
        let ids: Vec<_> = selected.into_iter().map(|a| a.id).collect();

        assert_eq!(ids, vec!["ticket-mcp", "log-viewer"]);
    }

    #[test]
    fn build_server_entry_for_log_viewer_uses_mcp_proxy_args() {
        let artifact = Artifact {
            id: "log-viewer".into(),
            category: "service".into(),
            kind: crate::registry::ArtifactKind::RustBinary,
            path: "log/crates/log-viewer".into(),
            bin: Some("log-viewer".into()),
            features: vec![],
            npm_script: None,
            extension_id: None,
        };

        let entry = build_server_entry(&artifact).unwrap();
        assert_eq!(entry["args"], serde_json::json!(["--", "log-viewer", "--mcp"]));
    }

    #[test]
    fn builds_workspace_json_for_existing_servers() {
        let workspace = tempfile::tempdir().unwrap();
        let config_path = workspace.path().join(".vscode");
        fs::create_dir_all(&config_path).unwrap();
        fs::write(
            config_path.join("mcp.json"),
            r#"{
  "servers": {
    "existing": { "type": "stdio", "command": "echo", "args": ["hello"] }
  }
}"#,
        ).unwrap();

        let mut selected = BTreeMap::new();
        selected.insert("ticket-mcp".to_string(), build_server_entry(&Artifact {
            id: "ticket-mcp".into(),
            category: "mcp".into(),
            kind: crate::registry::ArtifactKind::RustBinary,
            path: "ticket".into(),
            bin: Some("ticket-mcp".into()),
            features: vec!["mcp".into()],
            npm_script: None,
            extension_id: None,
        }).unwrap());

        let mut existing: serde_json::Value = serde_json::from_str(&fs::read_to_string(config_path.join("mcp.json")).unwrap()).unwrap();
        let servers = existing.get_mut("servers").and_then(|v| v.as_object_mut()).unwrap();
        for (id, entry) in selected {
            servers.insert(id.clone(), entry.clone());
        }

        let rendered = serde_json::to_string_pretty(&existing).unwrap();
        assert!(rendered.contains("\"existing\""));
        assert!(rendered.contains("\"ticket-mcp\""));
    }
}
