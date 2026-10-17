//! Contract for actual apply evidence.  This imports the future shared report
//! module directly because install-ctl remains a binary crate.

#[path = "../src/apply_report.rs"]
mod apply_report;

use std::path::PathBuf;

use apply_report::{ActionStatus, ApplyAction, ApplyReport};

#[test]
fn report_keeps_actual_action_evidence_and_detects_failures() {
    let mut report = ApplyReport::default();
    report.push(ApplyAction {
        id: "tool-one".into(),
        target: PathBuf::from("C:/root/bin/tool-one.exe"),
        status: ActionStatus::Written,
        detail: "cargo install completed".into(),
    });
    report.push(ApplyAction {
        id: "tool-two".into(),
        target: PathBuf::from("C:/root/bin/tool-two.exe"),
        status: ActionStatus::Unchanged,
        detail: "up to date".into(),
    });
    report.push(ApplyAction {
        id: "hook-capture".into(),
        target: PathBuf::new(),
        status: ActionStatus::Skipped,
        detail: "hook entries are inspect-only".into(),
    });
    report.push(ApplyAction {
        id: "broken-tool".into(),
        target: PathBuf::from("C:/root/bin/broken-tool.exe"),
        status: ActionStatus::Failed,
        detail: "cargo install failed".into(),
    });

    assert!(report.has_failures());
    assert_eq!(report.actions.len(), 4);
    assert_eq!(report.actions[0].target, PathBuf::from("C:/root/bin/tool-one.exe"));
    assert_eq!(report.actions[1].status, ActionStatus::Unchanged);
    assert_eq!(report.actions[2].detail, "hook entries are inspect-only");
}
