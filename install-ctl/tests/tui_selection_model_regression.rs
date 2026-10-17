//! Pure selection-state contract for the terminal TUI. This is intentionally
//! not a substitute for the later ratatui-testlib/PTY acceptance coverage.

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
#[path = "../src/apply_report.rs"]
mod apply_report;

#[path = "../src/guidance/destination.rs"]
pub mod guidance_destination;
#[path = "../src/guidance/graph.rs"]
pub mod guidance_graph;
#[path = "../src/guidance/profile.rs"]
pub mod guidance_profile;
#[path = "../src/guidance/rewrite.rs"]
pub mod guidance_rewrite;
#[path = "../src/guidance/plan.rs"]
pub mod guidance_plan;
#[path = "../src/guidance/install.rs"]
pub mod guidance_install;

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
#[path = "../src/tui.rs"]
mod tui;

use std::{
    fs,
    path::{Path, PathBuf},
};

use guidance::destination::DestinationPaths;
use plan::{InstallOptions, PlanExclusion};
use registry::{EntryCategory, EntryKind, LifecycleAction, RegistryEntryV2, Safety};
use review_plan::GuidanceInput;
use tui::SelectionState;

fn entry(id: &str, category: EntryCategory, kind: EntryKind) -> RegistryEntryV2 {
    RegistryEntryV2 {
        id: id.into(),
        category,
        kind,
        source_path: format!("tools/{id}"),
        owner: "tooling".into(),
        lifecycle: match kind {
            EntryKind::Hook => vec![LifecycleAction::Inspect],
            _ => vec![LifecycleAction::Install],
        },
        safety: Safety::ApprovalRequired,
        bin: Some(id.into()),
        features: vec![],
        npm_script: None,
    }
}

fn inventory() -> Vec<RegistryEntryV2> {
    vec![
        entry("hook-capture", EntryCategory::Hook, EntryKind::Hook),
        entry("doc-viewer", EntryCategory::Service, EntryKind::RustBinary),
        entry("bootstrap-script", EntryCategory::Misc, EntryKind::Script),
    ]
}

fn state(root: PathBuf) -> SelectionState {
    SelectionState::new(
        &inventory(),
        root,
        InstallOptions {
            force: false,
            offline: true,
            locked: true,
        },
    )
}

fn guidance_input() -> (tempfile::TempDir, GuidanceInput, DestinationPaths) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let profile = temp.path().join("profile.toml");
    let target = temp.path().join("guidance-target");
    fs::create_dir_all(source.join(".agents")).unwrap();
    fs::write(source.join(".agents/AGENTS.md"), "guidance fixture\n").unwrap();
    fs::write(
        &profile,
        "[profile]\nid = \"profile-a\"\n\n[[corpus]]\nid = \"root\"\npaths = [\".agents/AGENTS.md\"]\n\n[destination]\nscope = \"user\"\n",
    )
    .unwrap();
    (
        temp,
        GuidanceInput {
            source_root: source,
            profile_path: profile,
            select: vec!["root".into()],
            target_root: target,
            scope_override: None,
            explicit_override: None,
        },
        DestinationPaths {
            user_root: Path::new("C:/guidance-user-root").into(),
            system_root: Path::new("C:/guidance-system-root").into(),
        },
    )
}

#[test]
fn all_reaches_review_with_installable_selection_and_exclusion_diagnostics() {
    let root = PathBuf::from("C:/installer-root");
    let mut state = state(root.clone());

    state.toggle_all_categories();
    state.enter_artifacts();
    let review = state.prepare_review().unwrap();

    assert_eq!(
        review.selected.iter().map(|artifact| artifact.id.as_str()).collect::<Vec<_>>(),
        ["doc-viewer"]
    );
    assert_eq!(review.root, root);
    assert_eq!(
        review.options,
        InstallOptions {
            force: false,
            offline: true,
            locked: true,
        }
    );
    assert_eq!(
        review.excluded,
        vec![
            PlanExclusion {
                id: "hook-capture".into(),
                reason: "hook entries are inspect-only".into(),
            },
            PlanExclusion {
                id: "bootstrap-script".into(),
                reason: "unsupported artifact kind: script".into(),
            },
        ]
    );
}

#[test]
fn filtered_cursor_toggles_the_visible_inventory_entry_not_a_hidden_row() {
    let mut state = state(PathBuf::from("C:/installer-root"));
    assert!(state.toggle_category(EntryCategory::Service));
    state.enter_artifacts();
    assert_eq!(state.visible_indices(), vec![1]);

    // Enter preserves the existing TUI behavior: a selected category
    // preselects its visible artifacts.  The first toggle must therefore
    // deselect the service, not mutate the hidden hook at inventory index 0.
    state.toggle_visible(0).unwrap();
    assert_eq!(
        state.prepare_review().unwrap_err(),
        "selection matched no artifacts",
        "the visible service row must be the row that was toggled"
    );
    state.toggle_visible(0).unwrap();
    let review = state.prepare_review().unwrap();
    assert_eq!(
        review.selected.iter().map(|artifact| artifact.id.as_str()).collect::<Vec<_>>(),
        ["doc-viewer"]
    );
    assert!(review.excluded.is_empty(), "hidden hook must remain unselected");
}

#[test]
fn empty_category_is_disabled_and_all_toggles_categories_and_visible_entries_both_ways() {
    let mut state = state(PathBuf::from("C:/installer-root"));
    assert!(
        !state.toggle_category(EntryCategory::VscodeExtension),
        "empty vscode-extension category must remain disabled"
    );

    state.toggle_all_categories();
    state.toggle_all_categories();
    state.toggle_all_categories();
    state.enter_artifacts();
    state.toggle_all_visible();
    assert_eq!(
        state.prepare_review().unwrap_err(),
        "selection matched no artifacts",
        "`a` must deselect an already fully selected visible list"
    );
    state.toggle_all_visible();
    assert!(state.prepare_review().is_ok(), "`a` must select it again");
}

#[test]
fn cancellation_discards_selection_and_never_reuses_a_prepared_plan_or_writes() {
    let root = tempfile::tempdir().unwrap().path().join("not-created");
    let mut state = state(root.clone());
    state.toggle_all_categories();
    state.enter_artifacts();
    assert!(state.prepare_review().is_ok());

    state.cancel();
    assert!(!root.exists(), "selection/review/cancellation must not write");
    assert_eq!(
        state.prepare_review().unwrap_err(),
        "selection matched no artifacts",
        "a later confirmation must not retrieve a stale review plan"
    );
}

#[test]
fn composed_review_preserves_explicit_unsupported_artifact_error_with_guidance() {
    let (_temp, input, paths) = guidance_input();
    let mut state = state(PathBuf::from("C:/installer-root"));
    assert!(state.toggle_category(EntryCategory::Hook));
    state.enter_artifacts();

    assert_eq!(
        state.prepare_composed_review(&[input], &paths).err().unwrap(),
        "cannot install 'hook-capture': hook entries are inspect-only",
        "guidance composition must not replace an explicit artifact diagnostic"
    );
}

#[test]
fn composed_review_accepts_guidance_only_but_rejects_empty_everything() {
    let (_temp, input, paths) = guidance_input();
    let state = state(PathBuf::from("C:/installer-root"));

    let review = state.prepare_composed_review(&[input], &paths).unwrap();
    assert!(review.install.selected.is_empty());
    assert_eq!(review.guidance.len(), 1);

    assert_eq!(
        state.prepare_composed_review(&[], &paths).err().unwrap(),
        "selection matched no artifacts"
    );
}

#[test]
fn all_with_exclusions_and_valid_guidance_is_a_nonblocking_composed_review() {
    let (_temp, input, paths) = guidance_input();
    let mut state = state(PathBuf::from("C:/installer-root"));
    state.toggle_all_categories();
    state.enter_artifacts();

    let review = state.prepare_composed_review(&[input], &paths).unwrap();
    assert_eq!(
        review.install.selected.iter().map(|artifact| artifact.id.as_str()).collect::<Vec<_>>(),
        ["doc-viewer"]
    );
    assert_eq!(review.install.excluded.len(), 2);
    assert_eq!(review.guidance.len(), 1);
    assert!(!review.is_blocking(), "all exclusions are informational, not guidance diagnostics");
}
