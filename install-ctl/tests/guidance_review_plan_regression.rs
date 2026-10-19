//! Contract for optional guidance composition in the installer review.
//! This is deliberately a plan/apply gate, not terminal or PTY coverage.

#[path = "../src/apply_report.rs"]
mod apply_report;
#[path = "../src/installable.rs"]
mod installable;
#[path = "../src/paths.rs"]
mod paths;
#[path = "../src/plan.rs"]
mod plan;
#[path = "../src/registry.rs"]
mod registry;
#[path = "../src/selection.rs"]
mod selection;

#[path = "../src/guidance/destination.rs"]
pub mod guidance_destination;
#[path = "../src/guidance/graph.rs"]
pub mod guidance_graph;
#[path = "../src/guidance/install.rs"]
pub mod guidance_install;
#[path = "../src/guidance/plan.rs"]
pub mod guidance_plan;
#[path = "../src/guidance/profile.rs"]
pub mod guidance_profile;
#[path = "../src/guidance/rewrite.rs"]
pub mod guidance_rewrite;

mod guidance {
    pub use crate::guidance_destination as destination;
    pub use crate::guidance_graph as graph;
    pub use crate::guidance_install as install;
    pub use crate::guidance_plan as plan;
    pub use crate::guidance_profile as profile;
    pub use crate::guidance_rewrite as rewrite;
}
#[path = "../src/review_plan.rs"]
mod review_plan;

use std::{
    fs,
    path::{Path, PathBuf},
};

use guidance::{
    destination::DestinationPaths,
    plan::{PlanInputs, build_plan},
};
use plan::{InstallOptions, InstallPlan};
use registry::{Artifact, ArtifactKind};
use review_plan::{GuidanceInput, apply_review_plan, build_review_plan};

fn artifact_plan(root: PathBuf) -> InstallPlan {
    InstallPlan {
        selected: vec![Artifact {
            id: "doc-viewer".into(),
            category: "service".into(),
            kind: ArtifactKind::RustBinary,
            path: "doc".into(),
            bin: Some("doc-viewer".into()),
            features: vec![],
            npm_script: None,
            extension_id: None,
        }],
        root,
        options: InstallOptions::default(),
        excluded: vec![],
    }
}

fn guidance_only_plan(root: PathBuf) -> InstallPlan {
    InstallPlan {
        selected: vec![],
        root,
        options: InstallOptions::default(),
        excluded: vec![],
    }
}

fn write_profile(path: &Path, destination: &str) {
    fs::write(path, format!(
        "[profile]\nid = \"profile-a\"\n\n[[corpus]]\nid = \"root\"\npaths = [\".agents/AGENTS.md\"]\n\n[destination]\nscope = \"{destination}\"\n{}",
        if destination == "explicit" { "path = \"../escape\"\n" } else { "" }
    )).unwrap();
}

fn input(source: &Path, profile: &Path, target: &Path) -> GuidanceInput {
    GuidanceInput {
        source_root: source.into(),
        profile_path: profile.into(),
        select: vec!["root".into()],
        target_root: target.into(),
        scope_override: None,
        explicit_override: None,
    }
}

#[test]
fn no_profile_input_leaves_review_guidance_empty() {
    let root = tempfile::tempdir().unwrap();
    let paths = DestinationPaths {
        user_root: root.path().join("user"),
        system_root: root.path().join("system"),
    };
    let review = build_review_plan(artifact_plan(root.path().join("bin")), &[], &paths).unwrap();
    assert!(review.guidance.is_empty());
}

#[test]
fn composed_guidance_reuses_existing_builder_without_scope_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let profile = temp.path().join("profile.toml");
    let target = temp.path().join("guidance-target");
    fs::create_dir_all(source.join(".agents")).unwrap();
    fs::write(source.join(".agents/AGENTS.md"), "guidance").unwrap();
    write_profile(&profile, "user");
    let paths = DestinationPaths {
        user_root: temp.path().join("user-destination"),
        system_root: temp.path().join("system-destination"),
    };

    let direct = build_plan(&PlanInputs {
        source_root: &source,
        profile_path: &profile,
        select: &["root".into()],
        target_root: &target,
        scope_override: None,
        explicit_override: None,
        destination_paths: &paths,
    })
    .unwrap();
    let review = build_review_plan(
        artifact_plan(temp.path().join("binary-root")),
        &[input(&source, &profile, &target)],
        &paths,
    )
    .unwrap();
    let composed = &review.guidance[0].plan;

    assert_eq!(composed.profile_id, direct.profile_id);
    assert_eq!(composed.destination_scope, "user");
    assert_eq!(composed.destination_root, direct.destination_root);
    assert_eq!(
        composed
            .artifacts
            .iter()
            .map(|a| (&a.id, &a.final_path))
            .collect::<Vec<_>>(),
        direct
            .artifacts
            .iter()
            .map(|a| (&a.id, &a.final_path))
            .collect::<Vec<_>>()
    );
}

#[test]
fn guidance_only_review_applies_existing_guidance_plan_without_binary_apply() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let profile = temp.path().join("profile.toml");
    let target = temp.path().join("guidance-target");
    fs::create_dir_all(source.join(".agents")).unwrap();
    fs::write(source.join(".agents/AGENTS.md"), "fixture guidance\n").unwrap();
    write_profile(&profile, "user");
    let paths = DestinationPaths {
        user_root: temp.path().join("user-destination"),
        system_root: temp.path().join("system-destination"),
    };

    let review = build_review_plan(
        guidance_only_plan(temp.path().join("binary-root")),
        &[input(&source, &profile, &target)],
        &paths,
    )
    .unwrap();
    assert!(review.install.selected.is_empty());
    assert_eq!(review.guidance.len(), 1);

    let mut binary_apply_called = false;
    let report = apply_review_plan(&review, |_| {
        binary_apply_called = true;
        apply_report::ApplyReport::default()
    });

    assert!(
        !binary_apply_called,
        "guidance-only confirmation must not invoke binary installation"
    );
    assert!(!report.has_failures());
    assert_eq!(
        fs::read_to_string(paths.user_root.join("AGENTS.md")).unwrap(),
        "fixture guidance\n",
        "the composition apply path must delegate to existing guidance::install::install_plan"
    );
}

#[test]
fn blocking_guidance_prevents_both_apply_paths_and_cancellation_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let profile = temp.path().join("profile.toml");
    let target = temp.path().join("guidance-target");
    fs::create_dir_all(source.join(".agents")).unwrap();
    fs::write(source.join(".agents/AGENTS.md"), "guidance").unwrap();
    write_profile(&profile, "explicit");
    let paths = DestinationPaths {
        user_root: temp.path().join("user"),
        system_root: temp.path().join("system"),
    };
    let review = build_review_plan(
        artifact_plan(temp.path().join("binary-root")),
        &[input(&source, &profile, &target)],
        &paths,
    )
    .unwrap();
    assert!(review.is_blocking());
    assert!(
        !target.exists(),
        "planning/review cancellation must not write"
    );

    let mut artifact_apply_called = false;
    let report = apply_review_plan(&review, |_| {
        artifact_apply_called = true;
        apply_report::ApplyReport::default()
    });
    assert!(report.has_failures());
    assert!(
        !artifact_apply_called,
        "blocking guidance must gate binary apply"
    );
    assert!(
        !target.exists(),
        "blocking guidance must gate guidance apply"
    );
}

#[test]
fn cross_profile_destination_collision_blocks_all_apply_paths() {
    let temp = tempfile::tempdir().unwrap();
    let first_source = temp.path().join("first-source");
    let second_source = temp.path().join("second-source");
    let profile = temp.path().join("profile.toml");
    let target = temp.path().join("guidance-target");
    for source in [&first_source, &second_source] {
        fs::create_dir_all(source.join(".agents")).unwrap();
        fs::write(source.join(".agents/AGENTS.md"), "guidance").unwrap();
    }
    write_profile(&profile, "user");
    let paths = DestinationPaths {
        user_root: temp.path().join("user"),
        system_root: temp.path().join("system"),
    };
    let review = build_review_plan(
        artifact_plan(temp.path().join("binary-root")),
        &[
            input(&first_source, &profile, &target),
            input(&second_source, &profile, &target),
        ],
        &paths,
    )
    .unwrap();

    assert!(review.is_blocking());
    assert!(
        review
            .guidance
            .iter()
            .all(|planned| planned.plan.is_blocking())
    );
    let mut binary_apply_called = false;
    let report = apply_review_plan(&review, |_| {
        binary_apply_called = true;
        apply_report::ApplyReport::default()
    });
    assert!(report.has_failures());
    assert!(!binary_apply_called);
    assert!(
        !paths.user_root.exists(),
        "collision gate must run before writes"
    );
}
