//! Lifecycle-aware contract for the shared installer plan.  This remains a
//! binary crate, so the integration test imports the production modules by
//! path until a library target is intentionally introduced.

#[path = "../src/installable.rs"]
mod installable;
#[path = "../src/paths.rs"]
mod paths;
#[path = "../src/registry.rs"]
mod registry;
#[path = "../src/selection.rs"]
mod selection;
#[path = "../src/plan.rs"]
mod plan;

use std::path::PathBuf;

use plan::{InstallOptions, InstallPlan, PlanExclusion};
use registry::{
    EntryCategory, EntryKind, LifecycleAction, RegistryEntryV2, Safety, INSTALL_CATEGORIES,
};

fn entry(
    id: &str,
    category: EntryCategory,
    kind: EntryKind,
    lifecycle: Vec<LifecycleAction>,
) -> RegistryEntryV2 {
    RegistryEntryV2 {
        id: id.into(),
        category,
        kind,
        source_path: format!("tools/{id}"),
        owner: "tooling".into(),
        lifecycle,
        safety: Safety::ApprovalRequired,
        bin: Some(id.into()),
        features: vec![],
        npm_script: None,
    }
}

fn inventory() -> Vec<RegistryEntryV2> {
    vec![
        entry(
            "doc-viewer",
            EntryCategory::Service,
            EntryKind::RustBinary,
            vec![LifecycleAction::Install, LifecycleAction::Start],
        ),
        entry(
            "hook-capture",
            EntryCategory::Hook,
            EntryKind::Hook,
            vec![LifecycleAction::Inspect],
        ),
        entry(
            "bootstrap-script",
            EntryCategory::Misc,
            EntryKind::Script,
            vec![LifecycleAction::Install],
        ),
        entry(
            "managed-daemon",
            EntryCategory::Service,
            EntryKind::ManagedService,
            vec![LifecycleAction::Install, LifecycleAction::Start],
        ),
    ]
}

#[test]
fn inventory_exposes_all_six_literal_categories_including_empty_vscode_extension() {
    assert_eq!(
        INSTALL_CATEGORIES,
        &[
            EntryCategory::Cli,
            EntryCategory::Mcp,
            EntryCategory::Service,
            EntryCategory::VscodeExtension,
            EntryCategory::Hook,
            EntryCategory::Misc,
        ]
    );
    assert!(
        !inventory()
            .iter()
            .any(|entry| entry.category == EntryCategory::VscodeExtension),
        "the category inventory must retain the empty vscode-extension row"
    );
}

#[test]
fn rust_binary_service_is_installable_without_planning_a_service_start() {
    let plan = InstallPlan::from_inventory(
        &inventory(),
        &["doc-viewer".into()],
        PathBuf::from("C:/installer-root"),
        InstallOptions::default(),
    )
    .unwrap();

    assert_eq!(
        plan.selected.iter().map(|artifact| artifact.id.as_str()).collect::<Vec<_>>(),
        ["doc-viewer"]
    );
    assert!(plan.excluded.is_empty());
    assert_eq!(plan.selected[0].category, "service");
    assert_eq!(plan.selected[0].kind, registry::ArtifactKind::RustBinary);
}

#[test]
fn explicit_hook_script_and_managed_service_requests_block_before_apply() {
    for (token, diagnostic) in [
        (
            "hook-capture",
            "cannot install 'hook-capture': hook entries are inspect-only",
        ),
        (
            "bootstrap-script",
            "cannot install 'bootstrap-script': unsupported artifact kind: script",
        ),
        (
            "managed-daemon",
            "cannot install 'managed-daemon': unsupported artifact kind: managed-service",
        ),
    ] {
        assert_eq!(
            InstallPlan::from_inventory(
                &inventory(),
                &[token.into()],
                PathBuf::from("C:/installer-root"),
                InstallOptions::default(),
            )
            .unwrap_err(),
            diagnostic
        );
    }
}

#[test]
fn all_selects_only_installable_entries_and_reports_every_exclusion_reason() {
    let plan = InstallPlan::from_inventory(
        &inventory(),
        &["all".into()],
        PathBuf::from("C:/installer-root"),
        InstallOptions::default(),
    )
    .unwrap();

    assert_eq!(
        plan.selected.iter().map(|artifact| artifact.id.as_str()).collect::<Vec<_>>(),
        ["doc-viewer"]
    );
    assert_eq!(
        plan.excluded,
        vec![
            PlanExclusion {
                id: "hook-capture".into(),
                reason: "hook entries are inspect-only".into(),
            },
            PlanExclusion {
                id: "bootstrap-script".into(),
                reason: "unsupported artifact kind: script".into(),
            },
            PlanExclusion {
                id: "managed-daemon".into(),
                reason: "unsupported artifact kind: managed-service".into(),
            },
        ]
    );
}
