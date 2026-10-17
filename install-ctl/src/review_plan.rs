use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    apply_report::{ActionStatus, ApplyAction, ApplyReport},
    guidance::{
        destination::DestinationPaths,
        install::install_plan_with_report,
        plan::{build_plan, GuidancePlan, PlanInputs},
        profile::DestinationScopeKind,
    },
    plan::InstallPlan,
};

#[derive(Clone)]
pub struct GuidanceInput {
    pub source_root: PathBuf,
    pub profile_path: PathBuf,
    pub select: Vec<String>,
    pub target_root: PathBuf,
    pub scope_override: Option<DestinationScopeKind>,
    pub explicit_override: Option<PathBuf>,
}
pub struct PlannedGuidance { pub source_root: PathBuf, pub plan: GuidancePlan }
pub struct ReviewPlan { pub install: InstallPlan, pub guidance: Vec<PlannedGuidance> }
impl ReviewPlan { pub fn is_blocking(&self) -> bool { self.guidance.iter().any(|g| g.plan.is_blocking()) } }

pub fn build_review_plan(install: InstallPlan, inputs: &[GuidanceInput], paths: &DestinationPaths) -> Result<ReviewPlan, String> {
    let mut guidance = Vec::new();
    for input in inputs {
        let plan = build_plan(&PlanInputs { source_root: &input.source_root, profile_path: &input.profile_path, select: &input.select, target_root: &input.target_root, scope_override: input.scope_override, explicit_override: input.explicit_override.as_deref(), destination_paths: paths })?;
        guidance.push(PlannedGuidance { source_root: input.source_root.clone(), plan });
    }
    let mut destinations: BTreeMap<PathBuf, Vec<(usize, String)>> = BTreeMap::new();
    for (plan_index, planned) in guidance.iter().enumerate() {
        let destination_root = PathBuf::from(&planned.plan.destination_root);
        for artifact in &planned.plan.artifacts {
            destinations
                .entry(destination_root.join(&artifact.final_path))
                .or_default()
                .push((plan_index, artifact.id.clone()));
        }
    }
    for (destination, artifacts) in destinations {
        if artifacts.len() > 1 {
            let description = artifacts
                .iter()
                .map(|(plan_index, id)| format!("{}:{id}", guidance[*plan_index].plan.profile_id))
                .collect::<Vec<_>>()
                .join(", ");
            for (plan_index, _) in artifacts {
                guidance[plan_index].plan.diagnostics.push(crate::guidance::graph::Diagnostic {
                    code: crate::guidance::graph::DiagnosticCode::DuplicateDestination,
                    subject: crate::paths::disp(&destination),
                    message: format!("guidance profiles map {description} to the same destination"),
                });
            }
        }
    }
    Ok(ReviewPlan { install, guidance })
}
pub fn apply_review_plan<F>(review: &ReviewPlan, mut apply_binary: F) -> ApplyReport
where
    F: FnMut(&InstallPlan) -> ApplyReport,
{
    let mut report = ApplyReport::default();
    if review.is_blocking() {
        report.push(ApplyAction {
            id: "review".into(),
            target: review.install.root.clone(),
            status: ActionStatus::Failed,
            detail: "review has blocking diagnostics".into(),
        });
        return report;
    }
    if !review.install.selected.is_empty() || !review.install.excluded.is_empty() {
        report = apply_binary(&review.install);
    }
    for planned in &review.guidance {
        if report.has_failures() {
            report.push(ApplyAction {
                id: planned.plan.profile_id.clone(),
                target: planned.plan.destination_root.clone().into(),
                status: ActionStatus::Skipped,
                detail: "not executed after an earlier failure".into(),
            });
            continue;
        }
        let mut outcome = crate::guidance::install::InstallReport::default();
        let result = install_plan_with_report(&planned.plan, &planned.source_root, &mut outcome);
        for (paths, written) in [(outcome.written, true), (outcome.unchanged, false)] {
            for path in paths {
                report.push(ApplyAction {
                    id: format!("{}:{path}", planned.plan.profile_id),
                    target: path.into(),
                    status: if written { ActionStatus::Written } else { ActionStatus::Unchanged },
                    detail: "guidance file apply completed".into(),
                });
            }
        }
        if let Err(error) = result {
            report.push(ApplyAction {
                id: planned.plan.profile_id.clone(),
                target: planned.plan.destination_root.clone().into(),
                status: ActionStatus::Failed,
                detail: format!("{error}; completed files are reported above; remaining files were not applied"),
            });
        }
    }
    report
}
