use super::{
    GuidanceGetArgs, ProgressReporter,
    destination::DestinationPaths,
    graph, install,
    plan::{PlanInputs, build_plan_with_profile},
    print_plan,
    profile::Profile,
    remote,
};
use std::path::{Path, PathBuf};

pub(super) fn run(args: &GuidanceGetArgs) -> Result<(), String> {
    let reporter = |message: &str| eprintln!("guidance get: {message}");
    let (result, snapshot) = with_acquirer(
        args,
        |snapshot| {
            remote::acquire::acquire(
                &args.repository_url,
                &args.select,
                args.profile.as_deref(),
                snapshot,
                &remote::cache::default_root()?,
            )
        },
        &reporter,
    );
    if args.keep_checkout && snapshot.is_dir() {
        let message = format!("checkout kept at: {}", snapshot.display());
        if args.json {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
    }
    result
}

fn with_acquirer(
    args: &GuidanceGetArgs,
    acquire: impl FnOnce(&Path) -> Result<Profile, String>,
    reporter: &ProgressReporter<'_>,
) -> (Result<(), String>, PathBuf) {
    reporter("creating managed checkout");
    let snapshot = match tempfile::Builder::new()
        .prefix("install-ctl-guidance-get-")
        .tempdir()
    {
        Ok(directory) => directory,
        Err(error) => {
            return (
                Err(format!(
                    "failed to create a managed checkout directory: {error}"
                )),
                PathBuf::new(),
            );
        }
    };
    let snapshot_path = snapshot.path().to_owned();
    let result = (|| {
        reporter("acquiring selected source files");
        let profile = acquire(&snapshot_path)
            .map_err(|error| format!("failed to acquire selected guidance: {error}"))?;
        reporter("validated selected guidance profile");
        let destination_paths = DestinationPaths::platform_default();
        let inputs = PlanInputs {
            source_root: &snapshot_path,
            profile_path: &snapshot_path,
            select: &args.select,
            target_root: &args.target,
            scope_override: args.destination_scope,
            explicit_override: args.destination_path.as_deref(),
            destination_paths: &destination_paths,
        };
        reporter("building installation plan");
        let plan = build_plan_with_profile(profile, &inputs)?;
        print_plan(&plan, args.json);
        if plan.is_blocking() {
            let blocking: Vec<_> = plan
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    matches!(
                        diagnostic.code,
                        graph::DiagnosticCode::MissingDependency
                            | graph::DiagnosticCode::DuplicateDestination
                            | graph::DiagnosticCode::AmbiguousOwnership
                            | graph::DiagnosticCode::PathTraversal
                            | graph::DiagnosticCode::AbsoluteSourcePath
                            | graph::DiagnosticCode::SymlinkEscape
                            | graph::DiagnosticCode::UnsupportedRecipeStep
                            | graph::DiagnosticCode::UnsafeTarget
                    )
                })
                .collect();
            for diagnostic in &blocking {
                eprintln!("BLOCKING: [{:?}] {}", diagnostic.code, diagnostic.message);
            }
            return Err(format!(
                "plan has {} blocking diagnostic(s); see above",
                blocking.len()
            ));
        }
        reporter("installing guidance artifacts");
        let report = install::install_plan(&plan, &snapshot_path)?;
        let message = format!(
            "installed {} artifact(s), {} unchanged",
            report.written.len(),
            report.unchanged.len()
        );
        if args.json {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
        Ok(())
    })();
    if args.keep_checkout {
        let _ = snapshot.keep();
        reporter(&format!(
            "keeping managed checkout at {}",
            snapshot_path.display()
        ));
    } else {
        reporter("cleaning up managed checkout");
    }
    (result, snapshot_path)
}

#[cfg(test)]
pub(super) fn run_get(
    args: &GuidanceGetArgs,
    prepare: impl Fn(&str, &Path) -> Result<(), String>,
) -> (Result<(), String>, PathBuf) {
    run_get_with_reporter(args, prepare, &|message| {
        eprintln!("guidance get: {message}")
    })
}

#[cfg(test)]
pub(super) fn run_get_with_reporter(
    args: &GuidanceGetArgs,
    prepare: impl Fn(&str, &Path) -> Result<(), String>,
    reporter: &ProgressReporter<'_>,
) -> (Result<(), String>, PathBuf) {
    with_acquirer(
        args,
        |snapshot| {
            prepare(&args.repository_url, snapshot)?;
            match &args.profile {
                Some(path) => super::profile::load_profile(&snapshot.join(path)),
                None => super::profile::synthesize_direct_profile(&args.select),
            }
        },
        reporter,
    )
}
