//! `install-ctl guidance` command family: profile-driven planning and
//! installation of guidance corpora (see `.agents/instructions/**`) across
//! repo, user, system, or explicit destinations.
//!
//! Pure planning (`plan`) lives in [`plan::build_plan`] and never writes;
//! [`install::install_plan`] is the only module that mutates the
//! filesystem, and only ever from an already-validated, non-blocking plan.

pub mod autofix;
pub mod destination;
pub mod git;
pub mod graph;
pub mod install;
pub mod plan;
pub mod profile;
pub mod rewrite;
pub mod uninstall;

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};

use self::destination::DestinationPaths;
use self::plan::{PlanInputs, build_plan, build_plan_with_profile, render_text};
use self::profile::{DestinationScopeKind, load_profile, synthesize_direct_profile};

#[derive(Subcommand)]
pub enum GuidanceCmd {
    /// Compute a read-only installation plan for a guidance profile: no
    /// writes, no recipe execution, no network access.
    Plan(GuidanceArgs),
    /// Execute a validated guidance plan, writing artifacts to the
    /// destination. Refuses to write anything if the plan has a blocking
    /// diagnostic.
    Install(GuidanceArgs),
    /// Execute uninstallation for a guidance plan: removes installed
    /// guidance files mapped by the plan.
    Uninstall(GuidanceArgs),
    /// Explicit, opt-in remediation for blocking guidance-audit findings.
    /// `--plan` is read-only; `--apply` is the only mutation path and
    /// requires `--yes`.
    Autofix(GuidanceAutofixArgs),
    /// One-command fetch + select + install: clones `<repository-url>` with
    /// the system `git` binary, resolves an explicit or synthesized
    /// profile, plans, and installs into `--target` (default `.`), then
    /// deletes the managed checkout unless `--keep-checkout` is given.
    Get(GuidanceGetArgs),
}

#[derive(Args)]
pub struct GuidanceAutofixArgs {
    /// Repository root to audit and fix.
    #[arg(long, default_value = ".")]
    pub repo_root: PathBuf,
    /// Restrict autofix to findings whose source path equals or is nested
    /// under this repo-relative path.
    #[arg(long)]
    pub scope: Option<String>,
    /// Explicit `old-destination=new-destination` rewrite pair (repeatable).
    /// Only a finding whose raw link destination exactly matches an `old`
    /// value becomes a candidate operation; autofix never invents a
    /// transformation.
    #[arg(long = "rewrite", value_parser = parse_rewrite_pair)]
    pub rewrites: Vec<(String, String)>,
    /// Compute and print the plan; never writes to disk. Mutually exclusive
    /// with `--apply`.
    #[arg(long, conflicts_with = "apply")]
    pub plan: bool,
    /// Apply the plan. Requires `--yes`. Refuses stale-hash, ambiguous,
    /// unsafe, unsupported, or non-guidance operations; rolls back if the
    /// post-fix audit still blocks.
    #[arg(long, conflicts_with = "plan")]
    pub apply: bool,
    /// Explicit confirmation required alongside `--apply`.
    #[arg(long)]
    pub yes: bool,
    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

fn parse_rewrite_pair(value: &str) -> Result<(String, String), String> {
    let (old, new) = value.split_once('=').ok_or_else(|| {
        format!("--rewrite must be `old-destination=new-destination`, got '{value}'")
    })?;
    if old.is_empty() || new.is_empty() {
        return Err(format!(
            "--rewrite must have non-empty old and new destinations, got '{value}'"
        ));
    }
    Ok((old.to_string(), new.to_string()))
}

#[derive(Args)]
pub struct GuidanceArgs {
    /// Root of the repository providing the corpus (source repository).
    #[arg(long)]
    pub source: PathBuf,
    /// Path to the guidance profile TOML file.
    #[arg(long)]
    pub profile: PathBuf,
    /// Corpus item ids to select from the profile (repeatable).
    #[arg(long = "select", required = true)]
    pub select: Vec<String>,
    /// Root of the repository receiving installed guidance (used for the
    /// repo destination scope and submodule detection).
    #[arg(long)]
    pub target: PathBuf,
    /// Override the profile's declared destination scope: repo, user,
    /// system, or explicit.
    #[arg(long, value_enum)]
    pub destination_scope: Option<DestinationScopeKind>,
    /// Override the profile's declared explicit destination path (required
    /// when the effective scope is explicit and the profile sets none).
    #[arg(long)]
    pub destination_path: Option<PathBuf>,
    /// Emit the plan as JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct GuidanceGetArgs {
    /// Source repository to clone (any URL `git clone` accepts).
    pub repository_url: String,
    /// Corpus item ids to select. Resolved against `--profile`'s declared
    /// corpus when given; otherwise each value is a bare repo-relative
    /// source path, installed directly (no profile file required).
    #[arg(long = "select", required = true)]
    pub select: Vec<String>,
    /// Path to a guidance profile TOML file, resolved inside the freshly
    /// cloned checkout (not the caller's local filesystem). Omit to install
    /// `--select` paths directly without authoring a profile.
    #[arg(long)]
    pub profile: Option<PathBuf>,
    /// Root of the repository receiving installed guidance. Defaults to the
    /// current directory so the short command form only strictly requires
    /// the repository URL and a selection.
    #[arg(long, default_value = ".")]
    pub target: PathBuf,
    /// Override the profile's declared destination scope: repo, user,
    /// system, or explicit.
    #[arg(long, value_enum)]
    pub destination_scope: Option<DestinationScopeKind>,
    /// Override the profile's declared explicit destination path.
    #[arg(long)]
    pub destination_path: Option<PathBuf>,
    /// Keep the managed clone directory after the command finishes instead
    /// of deleting it, and print its path.
    #[arg(long)]
    pub keep_checkout: bool,
    /// Emit the plan as JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

pub fn run(command: GuidanceCmd) -> Result<(), String> {
    match command {
        GuidanceCmd::Plan(args) => {
            let plan = build_plan_from_args(&args)?;
            print_plan(&plan, args.json);
            if plan.is_blocking() {
                return Err(format!(
                    "plan has {} blocking diagnostic(s); see above",
                    plan.diagnostics.len()
                ));
            }
            Ok(())
        }
        GuidanceCmd::Install(args) => {
            let plan = build_plan_from_args(&args)?;
            print_plan(&plan, args.json);
            let report = install::install_plan(&plan, &args.source)?;
            println!(
                "installed {} artifact(s), {} unchanged",
                report.written.len(),
                report.unchanged.len()
            );
            Ok(())
        }
        GuidanceCmd::Uninstall(args) => {
            let plan = build_plan_from_args(&args)?;
            let report = uninstall::uninstall_plan(&plan)?;
            println!(
                "uninstalled {} guidance file(s), {} missing",
                report.removed.len(),
                report.missing.len()
            );
            Ok(())
        }
        GuidanceCmd::Autofix(args) => run_autofix(&args),
        GuidanceCmd::Get(args) => {
            let (result, checkout_path) = run_get(&args, git::clone_shallow);
            if args.keep_checkout {
                println!("checkout kept at: {}", checkout_path.display());
            }
            result
        }
    }
}

/// Fetch `args.repository_url`, resolve/synthesize a profile, plan, and
/// install — cleaning up the managed checkout on every exit path unless
/// `--keep-checkout` is given. `clone_fn` is injectable so tests never
/// require a real network call or the `git` binary. Always returns the
/// checkout path alongside the result: when `--keep-checkout` was not
/// given, that path no longer exists on disk by the time this returns,
/// which is itself the cleanup-happened assertion tests rely on.
fn run_get(
    args: &GuidanceGetArgs,
    clone_fn: impl Fn(&str, &Path) -> Result<(), String>,
) -> (Result<(), String>, PathBuf) {
    let checkout = match tempfile::Builder::new()
        .prefix("install-ctl-guidance-get-")
        .tempdir()
    {
        Ok(dir) => dir,
        Err(e) => {
            return (
                Err(format!(
                    "failed to create a managed checkout directory: {e}"
                )),
                PathBuf::new(),
            );
        }
    };
    let checkout_path = checkout.path().to_path_buf();
    // `git clone` accepts an existing empty directory as its destination.

    let result = (|| -> Result<(), String> {
        clone_fn(&args.repository_url, &checkout_path)
            .map_err(|e| format!("failed to clone '{}': {e}", args.repository_url))?;

        let profile = match &args.profile {
            Some(relative) => load_profile(&checkout_path.join(relative))?,
            None => synthesize_direct_profile(&args.select)?,
        };

        let destination_paths = DestinationPaths::platform_default();
        let inputs = PlanInputs {
            source_root: &checkout_path,
            // Unread by `build_plan_with_profile`; `checkout_path` is a
            // harmless placeholder to satisfy the shared struct literal.
            profile_path: &checkout_path,
            select: &args.select,
            target_root: &args.target,
            scope_override: args.destination_scope,
            explicit_override: args.destination_path.as_deref(),
            destination_paths: &destination_paths,
        };
        let plan = build_plan_with_profile(profile, &inputs)?;
        print_plan(&plan, args.json);
        if plan.is_blocking() {
            return Err(format!(
                "plan has {} blocking diagnostic(s); see above",
                plan.diagnostics.len()
            ));
        }

        let report = install::install_plan(&plan, &checkout_path)?;
        println!(
            "installed {} artifact(s), {} unchanged",
            report.written.len(),
            report.unchanged.len()
        );
        Ok(())
    })();

    if args.keep_checkout {
        // `checkout` (a `TempDir`) would delete this on drop; `keep`
        // disarms that so the directory survives past this function.
        let _ = checkout.keep();
    }
    (result, checkout_path)
}

fn run_autofix(args: &GuidanceAutofixArgs) -> Result<(), String> {
    if !args.plan && !args.apply {
        return Err(
            "guidance autofix requires --plan (read-only) or --apply (mutating)".to_string(),
        );
    }

    let repo_root = args
        .repo_root
        .canonicalize()
        .map_err(|error| format!("repository root does not exist: {error}"))?;
    let rewrites: BTreeMap<String, String> = args.rewrites.iter().cloned().collect();
    let plan = autofix::build_plan(&repo_root, args.scope.as_deref(), &rewrites)?;

    if args.plan {
        print_autofix_plan(&plan, args.json);
        return Ok(());
    }

    if !args.yes {
        return Err("guidance autofix --apply requires --yes to confirm the mutation".to_string());
    }

    let result = autofix::apply_plan(&repo_root, &plan, args.yes)?;
    print_autofix_result(&result, args.json);

    if result.rolled_back {
        return Err(format!(
            "autofix applied {} change(s) but the post-fix audit still had {} blocking finding(s); rolled back",
            result.applied.len(),
            result.post_audit_blocking_findings
        ));
    }
    if !result.refused.is_empty() {
        return Err(format!(
            "autofix refused {} operation(s); see above",
            result.refused.len()
        ));
    }
    Ok(())
}

fn print_autofix_plan(plan: &autofix::AutofixPlan, json: bool) {
    if json {
        match serde_json::to_string_pretty(&autofix::plan_to_json(plan)) {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("failed to serialize plan: {e}"),
        }
        return;
    }
    println!(
        "guidance autofix plan: {} operation(s), {} unresolved (read-only, no writes)",
        plan.operations.len(),
        plan.unresolved.len()
    );
    for operation in &plan.operations {
        println!(
            "  [{}] {}: '{}' -> '{}' (source_sha256={})",
            operation.category,
            operation.source_path,
            operation.raw_destination,
            operation.new_destination,
            operation.source_sha256
        );
    }
    for unresolved in &plan.unresolved {
        println!(
            "  UNRESOLVED [{}] {}: '{}' — {}",
            unresolved.category,
            unresolved.source_path,
            unresolved.raw_destination,
            unresolved.reason
        );
    }
}

fn print_autofix_result(result: &autofix::AutofixApplyResult, json: bool) {
    if json {
        match serde_json::to_string_pretty(&autofix::apply_result_to_json(result)) {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("failed to serialize apply result: {e}"),
        }
        return;
    }
    println!(
        "guidance autofix apply: {} applied, {} refused, rolled_back={}, post_audit_blocking_findings={}",
        result.applied.len(),
        result.refused.len(),
        result.rolled_back,
        result.post_audit_blocking_findings
    );
    for applied in &result.applied {
        println!(
            "  applied {}: '{}' -> '{}'",
            applied.source_path, applied.raw_destination, applied.new_destination
        );
    }
    for refused in &result.refused {
        println!(
            "  refused {}: '{}' — {}",
            refused.source_path, refused.raw_destination, refused.reason
        );
    }
}

fn build_plan_from_args(args: &GuidanceArgs) -> Result<plan::GuidancePlan, String> {
    let destination_paths = DestinationPaths::platform_default();
    let inputs = PlanInputs {
        source_root: &args.source,
        profile_path: &args.profile,
        select: &args.select,
        target_root: &args.target,
        scope_override: args.destination_scope,
        explicit_override: args.destination_path.as_deref(),
        destination_paths: &destination_paths,
    };
    build_plan(&inputs)
}

fn print_plan(plan: &plan::GuidancePlan, json: bool) {
    if json {
        match serde_json::to_string_pretty(plan) {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("failed to serialize plan: {e}"),
        }
    } else {
        print!("{}", render_text(plan));
    }
}
