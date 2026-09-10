//! `install-ctl guidance autofix`: explicit, opt-in remediation for blocking
//! guidance-audit findings.
//!
//! `--plan` is read-only and never touches the filesystem. `--apply` is the
//! only mutation path: it requires `--yes`, re-verifies the source file
//! hash recorded at plan time (refusing on a stale hash), rewrites only the
//! exact link destinations named by an explicit `--rewrite old=new` pair,
//! writes atomically (temp file + rename), reruns the audit, and rolls back
//! if the post-fix audit still blocks on the same destination.
//!
//! Autofix never invents a transformation: a blocking finding with no
//! matching `--rewrite` entry is reported as unresolved, which is the
//! correct, safe outcome for ambiguous/unsafe/unsupported/non-guidance
//! findings.

use std::{collections::BTreeMap, fs, path::PathBuf};

use audit_api::trials::markdown_links::{LinkClass, evaluate};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const TRANSFORMATION_REWRITE_LINK_DESTINATION: &str = "rewrite_link_destination";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutofixOperation {
    pub finding_id: String,
    pub category: String,
    pub source_path: String,
    pub source_sha256: String,
    pub raw_destination: String,
    pub new_destination: String,
    pub transformation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedFinding {
    pub finding_id: String,
    pub category: String,
    pub source_path: String,
    pub raw_destination: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutofixPlan {
    pub repo_root: String,
    pub scope: Option<String>,
    pub operations: Vec<AutofixOperation>,
    pub unresolved: Vec<UnresolvedFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedOperation {
    pub source_path: String,
    pub raw_destination: String,
    pub new_destination: String,
    pub transformation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefusedOperation {
    pub source_path: String,
    pub raw_destination: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutofixApplyResult {
    pub applied: Vec<AppliedOperation>,
    pub refused: Vec<RefusedOperation>,
    pub rolled_back: bool,
    pub post_audit_blocking_findings: usize,
}

/// Builds a read-only plan from the current guidance-audit findings and an
/// explicit rewrite map (`old relative destination` -> `new destination`).
/// Only findings whose raw destination exactly matches a rewrite key become
/// operations; every other blocking finding is reported as unresolved with
/// a specific refusal reason. Never writes to disk.
pub fn build_plan(
    repo_root: &std::path::Path,
    scope: Option<&str>,
    rewrites: &BTreeMap<String, String>,
) -> Result<AutofixPlan, String> {
    let result = evaluate(repo_root, &[]);
    let mut operations = Vec::new();
    let mut unresolved = Vec::new();

    for finding in &result.findings {
        if !is_blocking_category(&finding.category) {
            continue;
        }
        let Some(source_path) = finding.path.clone() else {
            continue;
        };
        if let Some(scope) = scope
            && !(source_path == scope || source_path.starts_with(&format!("{scope}/")))
        {
            continue;
        }

        let raw_destination = finding
            .evidence
            .get("target")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();

        let Some(new_destination) = rewrites.get(&raw_destination) else {
            unresolved.push(UnresolvedFinding {
                finding_id: finding.id.clone(),
                category: finding.category.clone(),
                source_path,
                raw_destination,
                reason: format!(
                    "no registered safe transformation for category '{}'; supply an explicit --rewrite {{destination}}={{replacement}} to resolve this finding",
                    finding.category
                ),
            });
            continue;
        };

        let absolute_source = repo_root.join(&source_path);
        let source_sha256 = match fs::read(&absolute_source) {
            Ok(bytes) => hash_bytes(&bytes),
            Err(error) => {
                unresolved.push(UnresolvedFinding {
                    finding_id: finding.id.clone(),
                    category: finding.category.clone(),
                    source_path,
                    raw_destination,
                    reason: format!("source could not be read: {error}"),
                });
                continue;
            }
        };

        operations.push(AutofixOperation {
            finding_id: finding.id.clone(),
            category: finding.category.clone(),
            source_path,
            source_sha256,
            raw_destination,
            new_destination: new_destination.clone(),
            transformation: TRANSFORMATION_REWRITE_LINK_DESTINATION.to_string(),
        });
    }

    Ok(AutofixPlan {
        repo_root: repo_root.to_string_lossy().replace('\\', "/"),
        scope: scope.map(str::to_string),
        operations,
        unresolved,
    })
}

fn is_blocking_category(category: &str) -> bool {
    [
        LinkClass::MissingTarget.category(),
        LinkClass::NonGuidanceTarget.category(),
        LinkClass::UnsupportedDependency.category(),
        LinkClass::UnsafePath.category(),
        LinkClass::UnreadableArtifact.category(),
    ]
    .contains(&category)
}

/// Applies a previously generated plan. Requires explicit confirmation
/// (`confirmed`). Re-verifies each operation's source hash before writing;
/// a mismatch (the source changed since the plan was generated) refuses
/// that operation with zero writes. Writes atomically (temp file + rename).
/// Reruns the audit after applying; if any touched source still has a
/// blocking finding for the same destination, every applied write in this
/// call is rolled back from its captured backup and the result reports
/// `rolled_back: true`.
pub fn apply_plan(
    repo_root: &std::path::Path,
    plan: &AutofixPlan,
    confirmed: bool,
) -> Result<AutofixApplyResult, String> {
    if !confirmed {
        return Err(
            "guidance autofix --apply requires explicit --yes confirmation; refusing to write"
                .to_string(),
        );
    }

    let mut applied = Vec::new();
    let mut refused = Vec::new();
    let mut backups: Vec<(PathBuf, Vec<u8>)> = Vec::new();

    for operation in &plan.operations {
        let absolute_source = repo_root.join(&operation.source_path);
        let current_bytes = match fs::read(&absolute_source) {
            Ok(bytes) => bytes,
            Err(error) => {
                refused.push(RefusedOperation {
                    source_path: operation.source_path.clone(),
                    raw_destination: operation.raw_destination.clone(),
                    reason: format!("source could not be read at apply time: {error}"),
                });
                continue;
            }
        };
        let current_hash = hash_bytes(&current_bytes);
        if current_hash != operation.source_sha256 {
            refused.push(RefusedOperation {
                source_path: operation.source_path.clone(),
                raw_destination: operation.raw_destination.clone(),
                reason: "stale plan: source hash changed since the plan was generated; \
                         regenerate the plan with a fresh `guidance autofix --plan`"
                    .to_string(),
            });
            continue;
        }

        let Ok(content) = String::from_utf8(current_bytes.clone()) else {
            refused.push(RefusedOperation {
                source_path: operation.source_path.clone(),
                raw_destination: operation.raw_destination.clone(),
                reason: "source is not valid UTF-8; refusing to apply a text rewrite".to_string(),
            });
            continue;
        };

        let old_link = format!("]({})", operation.raw_destination);
        let new_link = format!("]({})", operation.new_destination);
        if !content.contains(&old_link) {
            refused.push(RefusedOperation {
                source_path: operation.source_path.clone(),
                raw_destination: operation.raw_destination.clone(),
                reason: format!(
                    "link destination '{}' was not found verbatim in the source; refusing an ambiguous rewrite",
                    operation.raw_destination
                ),
            });
            continue;
        }

        let rewritten = content.replace(&old_link, &new_link);
        let temp_path = absolute_source.with_extension("autofix.tmp");
        if let Err(error) = fs::write(&temp_path, &rewritten) {
            refused.push(RefusedOperation {
                source_path: operation.source_path.clone(),
                raw_destination: operation.raw_destination.clone(),
                reason: format!("failed to write atomic temp file: {error}"),
            });
            continue;
        }
        if let Err(error) = fs::rename(&temp_path, &absolute_source) {
            let _ = fs::remove_file(&temp_path);
            refused.push(RefusedOperation {
                source_path: operation.source_path.clone(),
                raw_destination: operation.raw_destination.clone(),
                reason: format!("failed to atomically replace source: {error}"),
            });
            continue;
        }

        backups.push((absolute_source.clone(), current_bytes));
        applied.push(AppliedOperation {
            source_path: operation.source_path.clone(),
            raw_destination: operation.raw_destination.clone(),
            new_destination: operation.new_destination.clone(),
            transformation: operation.transformation.clone(),
        });
    }

    if applied.is_empty() {
        return Ok(AutofixApplyResult {
            applied,
            refused,
            rolled_back: false,
            post_audit_blocking_findings: 0,
        });
    }

    let post_audit = evaluate(repo_root, &[]);
    let still_blocking = post_audit
        .findings
        .iter()
        .filter(|finding| is_blocking_category(&finding.category))
        .filter(|finding| {
            applied.iter().any(|operation| {
                finding.path.as_deref() == Some(operation.source_path.as_str())
                    && finding
                        .evidence
                        .get("target")
                        .and_then(|value| value.as_str())
                        == Some(operation.new_destination.as_str())
            })
        })
        .count();

    if still_blocking > 0 {
        for (path, original_bytes) in &backups {
            let _ = fs::write(path, original_bytes);
        }
        return Ok(AutofixApplyResult {
            applied,
            refused,
            rolled_back: true,
            post_audit_blocking_findings: still_blocking,
        });
    }

    Ok(AutofixApplyResult {
        applied,
        refused,
        rolled_back: false,
        post_audit_blocking_findings: 0,
    })
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn plan_to_json(plan: &AutofixPlan) -> serde_json::Value {
    json!(plan)
}

pub fn apply_result_to_json(result: &AutofixApplyResult) -> serde_json::Value {
    json!(result)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::{apply_plan, build_plan};

    fn rewrites(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(old, new)| (old.to_string(), new.to_string()))
            .collect()
    }

    #[test]
    fn plan_is_read_only_and_reports_matching_operation() {
        let repo = tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
        std::fs::write(
            repo.path().join(".agents/links.md"),
            "[moved](old-name.md)\n",
        )
        .unwrap();

        let before = std::fs::read(repo.path().join(".agents/links.md")).unwrap();
        let plan = build_plan(
            repo.path(),
            None,
            &rewrites(&[("old-name.md", "README.md")]),
        )
        .expect("plan");
        let after = std::fs::read(repo.path().join(".agents/links.md")).unwrap();

        assert_eq!(before, after, "plan must not write to disk");
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].new_destination, "README.md");
        assert!(plan.unresolved.is_empty());
    }

    #[test]
    fn plan_leaves_findings_without_a_rewrite_unresolved() {
        let repo = tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::write(repo.path().join(".agents/links.md"), "[gone](missing.md)\n").unwrap();

        let plan = build_plan(repo.path(), None, &BTreeMap::new()).expect("plan");

        assert!(plan.operations.is_empty());
        assert_eq!(plan.unresolved.len(), 1);
        assert!(
            plan.unresolved[0]
                .reason
                .contains("no registered safe transformation")
        );
    }

    #[test]
    fn apply_requires_explicit_confirmation() {
        let repo = tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
        std::fs::write(
            repo.path().join(".agents/links.md"),
            "[moved](old-name.md)\n",
        )
        .unwrap();

        let plan = build_plan(
            repo.path(),
            None,
            &rewrites(&[("old-name.md", "README.md")]),
        )
        .expect("plan");

        let error = apply_plan(repo.path(), &plan, false).expect_err("must refuse without --yes");
        assert!(error.contains("--yes"));
    }

    #[test]
    fn apply_writes_atomically_and_reruns_audit() {
        let repo = tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
        std::fs::write(
            repo.path().join(".agents/links.md"),
            "[moved](old-name.md)\n",
        )
        .unwrap();

        let plan = build_plan(
            repo.path(),
            None,
            &rewrites(&[("old-name.md", "README.md")]),
        )
        .expect("plan");
        let result = apply_plan(repo.path(), &plan, true).expect("apply");

        assert_eq!(result.applied.len(), 1);
        assert!(!result.rolled_back);
        assert_eq!(result.post_audit_blocking_findings, 0);
        let rewritten = std::fs::read_to_string(repo.path().join(".agents/links.md")).unwrap();
        assert!(rewritten.contains("[moved](README.md)"));
    }

    #[test]
    fn apply_refuses_stale_hash_with_zero_writes() {
        let repo = tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::write(repo.path().join(".agents/README.md"), "# guidance\n").unwrap();
        std::fs::write(
            repo.path().join(".agents/links.md"),
            "[moved](old-name.md)\n",
        )
        .unwrap();

        let plan = build_plan(
            repo.path(),
            None,
            &rewrites(&[("old-name.md", "README.md")]),
        )
        .expect("plan");

        // Source changes after the plan was generated.
        std::fs::write(
            repo.path().join(".agents/links.md"),
            "[moved](old-name.md)\nextra content\n",
        )
        .unwrap();
        let before_apply = std::fs::read(repo.path().join(".agents/links.md")).unwrap();

        let result = apply_plan(repo.path(), &plan, true).expect("apply call succeeds");

        assert!(result.applied.is_empty());
        assert_eq!(result.refused.len(), 1);
        assert!(result.refused[0].reason.contains("stale plan"));
        let after_apply = std::fs::read(repo.path().join(".agents/links.md")).unwrap();
        assert_eq!(
            before_apply, after_apply,
            "stale-hash refusal must not write"
        );
    }

    #[test]
    fn apply_rolls_back_when_post_audit_still_blocks() {
        let repo = tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        // Deliberately rewrite to another missing target so the post-audit
        // still blocks and the apply must roll back to the original file.
        std::fs::write(
            repo.path().join(".agents/links.md"),
            "[moved](old-name.md)\n",
        )
        .unwrap();

        let plan = build_plan(
            repo.path(),
            None,
            &rewrites(&[("old-name.md", "still-missing.md")]),
        )
        .expect("plan");
        let original = std::fs::read(repo.path().join(".agents/links.md")).unwrap();

        let result = apply_plan(repo.path(), &plan, true).expect("apply");

        assert!(result.rolled_back);
        assert_eq!(result.post_audit_blocking_findings, 1);
        let after = std::fs::read(repo.path().join(".agents/links.md")).unwrap();
        assert_eq!(original, after, "rollback must restore original content");
    }
}
