//! Guidance plan uninstallation: safely removes installed guidance files mapped
//! by a [`GuidancePlan`], and cleans up any parent directories that become empty.

use std::{fs, path::PathBuf};

use crate::guidance::plan::GuidancePlan;

pub struct UninstallReport {
    pub removed: Vec<String>,
    pub missing: Vec<String>,
}

/// Execute uninstallation for `plan`.
/// Safely removes installed guidance files mapped by `plan.artifacts` under
/// `plan.destination_root` and removes parent directories if they become empty.
pub fn uninstall_plan(plan: &GuidancePlan) -> Result<UninstallReport, String> {
    let destination_root = PathBuf::from(&plan.destination_root);

    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for artifact in &plan.artifacts {
        let final_path = destination_root.join(&artifact.final_path);
        if final_path.is_file() {
            fs::remove_file(&final_path).map_err(|e| {
                format!(
                    "failed to remove guidance file '{}': {e}",
                    final_path.display()
                )
            })?;
            removed.push(crate::paths::disp(&final_path));

            // Clean up empty parent directories up to destination_root
            let mut parent = final_path.parent();
            while let Some(dir) = parent {
                if dir == destination_root || !dir.starts_with(&destination_root) {
                    break;
                }
                if fs::read_dir(dir)
                    .map(|mut i| i.next().is_none())
                    .unwrap_or(false)
                {
                    let _ = fs::remove_dir(dir);
                    parent = dir.parent();
                } else {
                    break;
                }
            }
        } else {
            missing.push(crate::paths::disp(&final_path));
        }
    }

    Ok(UninstallReport { removed, missing })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guidance::plan::{ArtifactOrigin, GuidancePlan, PlannedArtifact};
    use tempfile::TempDir;

    #[test]
    fn uninstall_plan_removes_artifacts_and_empty_dirs() {
        let temp = TempDir::new().unwrap();
        let file_a = temp.path().join("nested/dir/a.md");
        let file_b = temp.path().join("nested/dir/b.md");

        fs::create_dir_all(file_a.parent().unwrap()).unwrap();
        fs::write(&file_a, "content a").unwrap();
        fs::write(&file_b, "content b").unwrap();

        let plan = GuidancePlan {
            profile_id: "test".to_string(),
            selected_roots: vec!["nested/dir/a.md".to_string(), "nested/dir/b.md".to_string()],
            destination_scope: "repo".to_string(),
            destination_root: temp.path().display().to_string(),
            artifacts: vec![
                PlannedArtifact {
                    id: "nested/dir/a.md".to_string(),
                    origin: ArtifactOrigin::Direct,
                    final_path: "nested/dir/a.md".to_string(),
                    rewrites: vec![],
                },
                PlannedArtifact {
                    id: "nested/dir/b.md".to_string(),
                    origin: ArtifactOrigin::Direct,
                    final_path: "nested/dir/b.md".to_string(),
                    rewrites: vec![],
                },
            ],
            edges: vec![],
            skipped_external: vec![],
            diagnostics: vec![],
        };

        let report = uninstall_plan(&plan).unwrap();
        assert_eq!(report.removed.len(), 2);
        assert!(!file_a.exists());
        assert!(!file_b.exists());
        assert!(!temp.path().join("nested/dir").exists());
        assert!(!temp.path().join("nested").exists());
    }
}
