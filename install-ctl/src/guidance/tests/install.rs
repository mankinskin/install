use super::*;

// -- plan is read-only (AC7) --------------------------------------------------

#[test]
fn plan_never_writes_to_source_or_destination() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "content");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
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
    let _plan = build_plan(&inputs).expect("plan should build");

    assert!(!explicit.exists(), "plan must not create the destination");
    // Source tree is unchanged: still exactly the two files we wrote.
    let mut entries: Vec<String> = walk(src.path());
    entries.sort();
    assert_eq!(entries, vec!["guidance.toml", "prompt.md"]);
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in walkdir(root, root) {
        out.push(entry);
    }
    out
}

fn walkdir(root: &Path, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(root, &path));
        } else {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    out
}

// -- install: refuses on diagnostics, atomic, idempotent (AC8, AC9) ----------

#[test]
fn install_refuses_to_write_a_blocking_plan() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [out](../../outside.md)");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
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
    assert!(plan.is_blocking());

    let result = install_plan(&plan, src.path());
    assert!(result.is_err());
    assert!(!explicit.exists(), "a blocked install must write nothing");
}

#[test]
fn install_is_idempotent_and_preserves_unrelated_files() {
    let src = TempDir::new().unwrap();
    write(src.path(), "prompt.md", "see [base](instructions/base.md)");
    write(src.path(), "instructions/base.md", "base content");
    write(
        src.path(),
        "guidance.toml",
        &direct_profile("explicit", Some("/unused")),
    );

    let target = TempDir::new().unwrap();
    let dest = TempDir::new().unwrap();
    let paths = test_destination_paths(&dest);
    let explicit = dest.path().join("out");
    // An unrelated pre-existing file under the destination must survive.
    write(&explicit, "unrelated.txt", "keep me");

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
    assert!(!plan.is_blocking());

    let first = install_plan(&plan, src.path()).expect("first install succeeds");
    assert_eq!(first.written.len(), 2);
    assert_eq!(first.unchanged.len(), 0);

    let second_plan = build_plan(&inputs).expect("second plan should build");
    let second = install_plan(&second_plan, src.path()).expect("second install succeeds");
    assert_eq!(second.written.len(), 0, "re-install must be a no-op");
    assert_eq!(second.unchanged.len(), 2);

    assert_eq!(
        fs::read_to_string(explicit.join("unrelated.txt")).unwrap(),
        "keep me"
    );
}
