use super::get::{fake_clone_from, get_args};
use crate::guidance::profile::DestinationScopeKind as ScopeKind;
use crate::guidance::run_get;
use super::*;

#[test]
fn guidance_uninstall_removes_installed_plan() {
    let source = TempDir::new().unwrap();
    write(source.path(), "prompt.md", "content");

    let target = TempDir::new().unwrap();
    let explicit = target.path().join("out");

    let mut args = get_args(
        "unused",
        vec!["prompt.md".to_string()],
        target.path().to_path_buf(),
    );
    args.destination_path = Some(explicit.clone());

    let (result, _) = run_get(&args, fake_clone_from(source.path().to_path_buf()));
    result.expect("install should succeed");
    assert!(explicit.join("prompt.md").is_file());

    let plan_inputs = crate::guidance::PlanInputs {
        source_root: source.path(),
        profile_path: source.path(),
        select: &["prompt.md".to_string()],
        target_root: target.path(),
        scope_override: Some(ScopeKind::Explicit),
        explicit_override: Some(&explicit),
        destination_paths: &crate::guidance::DestinationPaths::platform_default(),
    };
    let profile = crate::guidance::profile::synthesize_direct_profile(&["prompt.md".to_string()]).unwrap();
    let plan = crate::guidance::plan::build_plan_with_profile(profile, &plan_inputs).unwrap();

    let report = crate::guidance::uninstall::uninstall_plan(&plan).unwrap();
    assert_eq!(report.removed.len(), 1);
    assert!(!explicit.join("prompt.md").exists());
}
