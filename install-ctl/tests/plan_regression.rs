//! Contract tests for the first shared scripted/TUI installer-plan seam.
//! `install-ctl` is still a binary crate, so import the extracted module and
//! its direct source dependencies by path until it grows a library target.

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

use plan::{InstallOptions, InstallPlan};
use registry::{Artifact, ArtifactKind};

fn artifacts() -> Vec<Artifact> {
    vec![
        artifact("install-ctl", "cli"),
        artifact("ticket", "cli"),
        artifact("doc-viewer", "service"),
        artifact("peek-mcp", "mcp"),
    ]
}

fn artifact(id: &str, category: &str) -> Artifact {
    Artifact {
        id: id.into(),
        category: category.into(),
        kind: ArtifactKind::RustBinary,
        path: format!("tools/{id}"),
        bin: Some(id.into()),
        features: vec![],
        npm_script: None,
        extension_id: None,
    }
}

fn ids(plan: &InstallPlan) -> Vec<&str> {
    plan.selected.iter().map(|artifact| artifact.id.as_str()).collect()
}

#[test]
fn plan_preserves_explicit_root_and_all_cargo_options_without_writing() {
    let root = tempfile::tempdir()
        .unwrap()
        .path()
        .join("does-not-exist-until-apply");
    let options = InstallOptions {
        force: false,
        offline: true,
        locked: true,
    };

    let plan = InstallPlan::from_tokens(
        &artifacts(),
        &["doc-viewer".into()],
        root.clone(),
        options,
    )
    .unwrap();

    assert_eq!(ids(&plan), ["doc-viewer"]);
    assert_eq!(plan.root, root);
    assert_eq!(plan.options, options);
    assert!(
        !plan.root.exists(),
        "planning must validate only; apply owns target-root writes"
    );
}

#[test]
fn plan_uses_existing_id_category_all_and_dedup_resolution_in_token_order() {
    let plan = InstallPlan::from_tokens(
        &artifacts(),
        &[
            "ticket,service".into(),
            "ticket".into(),
            "all".into(),
            "cli".into(),
        ],
        PathBuf::from("C:/installer-root"),
        InstallOptions::default(),
    )
    .unwrap();

    assert_eq!(
        ids(&plan),
        ["ticket", "doc-viewer", "install-ctl", "peek-mcp"],
        "the plan must retain resolve_selection's token order while deduplicating IDs"
    );
    assert_eq!(
        plan.options,
        InstallOptions {
            force: true,
            offline: false,
            locked: false,
        },
        "scripted defaults must remain the TUI plan defaults"
    );
}

#[test]
fn plan_rejects_unknown_or_empty_selection_before_apply() {
    let root = PathBuf::from("C:/installer-root");

    assert_eq!(
        InstallPlan::from_tokens(
            &artifacts(),
            &["not-registered".into()],
            root.clone(),
            InstallOptions::default(),
        )
        .unwrap_err(),
        "unknown artifact or category: not-registered"
    );
    assert_eq!(
        InstallPlan::from_tokens(&artifacts(), &[], root, InstallOptions::default()).unwrap_err(),
        "selection matched no artifacts"
    );
}
