//! Span-based install rewrites: only located link destinations are edited,
//! and unlocatable ones warn without blocking.

use super::*;
use crate::guidance::graph::{Diagnostic, DiagnosticCode};
use crate::guidance::plan::{GuidancePlan, render_text};

const FOO: &str = "a/.agents/instructions/foo.md";
const THING_LINK: &str = "../../../b/.agents/instructions/thing.md";

/// Plans and installs `foo.md` (which links across owning directories, so the
/// link is rewritten to a sibling name) into an explicit destination. Returns
/// the plan and the installed `foo.md` text.
fn install_foo(foo_text: &str, thing_name: &str) -> (GuidancePlan, String) {
    let src = TempDir::new().unwrap();
    write(src.path(), FOO, foo_text);
    write(
        src.path(),
        &format!("b/.agents/instructions/{thing_name}"),
        "thing",
    );
    write(
        src.path(),
        "guidance.toml",
        &format!(
            "[profile]\nid = \"span\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"{FOO}\"]\n\n[destination]\nscope = \"explicit\"\npath = \"/unused\"\n"
        ),
    );
    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    let inputs = PlanInputs {
        source_root: src.path(),
        profile_path: &src.path().join("guidance.toml"),
        select: &["root".to_string()],
        target_root: target.path(),
        scope_override: Some(DestinationScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &paths,
    };
    let plan = build_plan(&inputs).expect("plan should build");
    assert!(!plan.is_blocking(), "diagnostics: {:?}", plan.diagnostics);
    install_plan(&plan, src.path()).expect("install must succeed");
    let text = fs::read_to_string(explicit.join("instructions/foo.md")).unwrap();
    (plan, text)
}

fn unlocatable(plan: &GuidancePlan) -> usize {
    plan.diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnlocatableRewrite)
        .count()
}

#[test]
fn install_leaves_code_fences_and_spans_byte_identical() {
    let text = format!(
        "[t]({THING_LINK})\n\n```\n[t]({THING_LINK})\n```\n\nInline `[t]({THING_LINK})`.\n"
    );
    let (_, out) = install_foo(&text, "thing.md");
    let expected = text.replacen(THING_LINK, "thing.md", 1);
    assert_eq!(out, expected);
}

#[test]
fn install_rewrites_reference_definition_destination() {
    let text = format!("[t][r]\n\n[r]: {THING_LINK}\n");
    let (_, out) = install_foo(&text, "thing.md");
    assert_eq!(out, "[t][r]\n\n[r]: thing.md\n");
}

#[test]
fn install_encodes_space_and_preserves_fragment() {
    let text = "[t](../../../b/.agents/instructions/my%20thing.md#sec)";
    let (plan, out) = install_foo(text, "my thing.md");
    assert_eq!(out, "[t](my%20thing.md#sec)");
    assert_eq!(unlocatable(&plan), 0);
}

#[test]
fn unlocatable_destination_warns_and_install_leaves_link_unchanged() {
    let text = "[t](../../../b/.agents/instructions/thing\\.md)";
    let (plan, out) = install_foo(text, "thing.md");
    assert_eq!(unlocatable(&plan), 1, "{:?}", plan.diagnostics);
    assert!(!plan.is_blocking());
    assert!(plan.artifacts.iter().all(|a| a.rewrites.is_empty()));
    assert_eq!(out, text);
}

#[test]
fn mixed_located_and_unlocatable_occurrences_rewrite_only_the_located_one() {
    let text = format!("[a]({THING_LINK}) and [b](../../../b/.agents/instructions/thing\\.md)");
    let (plan, out) = install_foo(&text, "thing.md");
    assert_eq!(unlocatable(&plan), 1, "{:?}", plan.diagnostics);
    assert!(!plan.is_blocking());
    assert_eq!(
        out,
        "[a](thing.md) and [b](../../../b/.agents/instructions/thing\\.md)"
    );
}

fn diagnostic(code: DiagnosticCode) -> Diagnostic {
    let message = format!("{code:?} message");
    Diagnostic {
        code,
        subject: "s".into(),
        message,
    }
}

fn mixed_plan() -> GuidancePlan {
    GuidancePlan {
        profile_id: "mixed".into(),
        selected_roots: vec![],
        destination_scope: "explicit".into(),
        destination_root: "/unused".into(),
        artifacts: vec![],
        edges: vec![],
        skipped_external: vec![],
        diagnostics: vec![
            diagnostic(DiagnosticCode::DuplicateDestination),
            diagnostic(DiagnosticCode::UnlocatableRewrite),
            diagnostic(DiagnosticCode::MissingDependency),
        ],
    }
}

#[test]
fn only_blocking_codes_count_as_blocking_diagnostics() {
    let plan = mixed_plan();
    assert_eq!(plan.blocking_diagnostics().count(), 1);
    assert!(plan.is_blocking());
}

#[test]
fn render_text_lists_unlocatable_rewrite_under_warnings() {
    let text = render_text(&mixed_plan());
    let (blocking, warnings) = text
        .split_once("warnings:")
        .expect("a warnings section is rendered");
    assert!(blocking.contains("diagnostics (blocking):"));
    assert!(blocking.contains("DuplicateDestination"));
    assert!(!blocking.contains("UnlocatableRewrite"));
    assert!(warnings.contains("UnlocatableRewrite"));
}
