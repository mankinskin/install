//! Plan execution: stage every artifact's final bytes in memory first, then
//! perform per-file atomic renames only after every artifact staged
//! successfully. A blocking plan, or any staging failure, writes nothing.

use std::{fs, path::PathBuf};

use crate::guidance::plan::GuidancePlan;

#[derive(Default)]
pub struct InstallReport {
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Execute `plan` against `source_root`. Refuses to write anything when the
/// plan has a blocking diagnostic. Content is staged fully in memory before
/// any filesystem mutation, so a read failure on one artifact leaves the
/// destination untouched.
pub fn install_plan(
    plan: &GuidancePlan,
    source_root: &std::path::Path,
) -> Result<InstallReport, String> {
    let mut report = InstallReport::default();
    install_plan_with_report(plan, source_root, &mut report)?;
    Ok(report)
}

/// Retain completed file outcomes in `report`, including when a later write fails.
pub fn install_plan_with_report(
    plan: &GuidancePlan,
    source_root: &std::path::Path,
    report: &mut InstallReport,
) -> Result<(), String> {
    if plan.is_blocking() {
        return Err(format!(
            "refusing to install: plan has {} blocking diagnostic(s)",
            plan.diagnostics.len()
        ));
    }

    let destination_root = PathBuf::from(&plan.destination_root);

    // Stage: read/rewrite every artifact's final bytes before any write.
    let mut staged: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(plan.artifacts.len());
    for artifact in &plan.artifacts {
        let read_id = match &artifact.origin {
            crate::guidance::plan::ArtifactOrigin::Direct => artifact.id.clone(),
            crate::guidance::plan::ArtifactOrigin::RecipeCopy { from } => from.clone(),
        };
        let mut bytes = fs::read(source_root.join(&read_id))
            .map_err(|e| format!("failed to read '{}': {e}", read_id))?;

        if !artifact.rewrites.is_empty() {
            let mut text = String::from_utf8(bytes)
                .map_err(|e| format!("'{}' is not valid UTF-8: {e}", artifact.id))?;
            for rewrite in &artifact.rewrites {
                text = text.replace(
                    &format!("]({}", rewrite.old_link),
                    &format!("]({}", rewrite.new_link),
                );
            }
            bytes = text.into_bytes();
        }

        let final_path = destination_root.join(&artifact.final_path);
        staged.push((final_path, bytes));
    }

    for (final_path, bytes) in staged {
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create '{}': {e}", parent.display()))?;
        }
        if fs::read(&final_path)
            .map(|existing| existing == bytes)
            .unwrap_or(false)
        {
            report.unchanged.push(crate::paths::disp(&final_path));
            continue;
        }
        let tmp_name = format!(
            "{}.install-ctl-tmp",
            final_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("artifact")
        );
        let tmp_path = final_path.with_file_name(tmp_name);
        fs::write(&tmp_path, &bytes)
            .map_err(|e| format!("failed to stage '{}': {e}", tmp_path.display()))?;
        fs::rename(&tmp_path, &final_path)
            .map_err(|e| format!("failed to install '{}': {e}", final_path.display()))?;
        report.written.push(crate::paths::disp(&final_path));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guidance::plan::{ArtifactOrigin, PlannedArtifact};

    #[test]
    fn reporting_retains_written_file_when_a_later_destination_is_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("first.md"), "first contents").unwrap();
        fs::write(source.join("second.md"), "second contents").unwrap();
        fs::write(target.join("blocked"), "not a directory").unwrap();
        let plan = GuidancePlan {
            profile_id: "partial-report".into(),
            selected_roots: vec!["first.md".into(), "second.md".into()],
            destination_scope: "repo".into(),
            destination_root: target.display().to_string(),
            artifacts: vec![
                PlannedArtifact {
                    id: "first.md".into(),
                    origin: ArtifactOrigin::Direct,
                    final_path: "first.md".into(),
                    rewrites: vec![],
                },
                PlannedArtifact {
                    id: "second.md".into(),
                    origin: ArtifactOrigin::Direct,
                    final_path: "blocked/second.md".into(),
                    rewrites: vec![],
                },
            ],
            edges: vec![],
            skipped_external: vec![],
            diagnostics: vec![],
        };
        let mut report = InstallReport::default();

        let error = install_plan_with_report(&plan, &source, &mut report).unwrap_err();

        assert!(error.contains("failed to create"), "{error}");
        assert_eq!(
            report.written,
            vec![crate::paths::disp(&target.join("first.md"))]
        );
        assert!(report.unchanged.is_empty());
        assert_eq!(
            fs::read_to_string(target.join("first.md")).unwrap(),
            "first contents"
        );
        assert!(!target.join("blocked/second.md").exists());
    }
}
