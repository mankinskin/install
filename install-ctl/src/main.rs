mod apply_report;
mod cli;
mod commands;
mod config;
mod fingerprint;
mod freshness;
mod guidance;
pub mod installable;
mod logging;
mod paths;
mod plan;
mod prebuilt;
mod process;
mod registry;
mod review_plan;
mod selection;
mod shell;
mod status;
mod tui;
mod update;

use std::{path::Path, time::Duration};

use apply_report::{ActionStatus, ApplyAction, ApplyReport};
use clap::{Parser, Subcommand};
use cli::{ViewerCmd, WorkspaceCmd};
use config::Config;
use plan::{InstallOptions, InstallPlan};
use registry::{Artifact, ArtifactKind, load_registry, load_registry_v2, sync_catalog};
use selection::resolve_selection;

#[derive(Parser)]
#[command(
    name = "install-ctl",
    about = "Install workspace tool binaries and extensions, and manage the viewer lifecycle"
)]
struct Cli {
    /// Print supported artifacts grouped by category and exit.
    #[arg(long)]
    list: bool,

    /// Print the planned actions without performing them.
    #[arg(long)]
    dry_run: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Install one or more artifacts by id, category, or "all".
    Install {
        #[arg(required = true)]
        selection: Vec<String>,
        /// Skip passing --force to `cargo install` for rust-binary artifacts.
        #[arg(long)]
        no_force: bool,
        /// Run `cargo install` without network access; the warm-up build remains online.
        #[arg(long)]
        offline: bool,
        /// Also prevent the warm-up build from updating Cargo.lock.
        #[arg(long)]
        locked: bool,
        /// Target installation root directory.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
    },
    /// Install selected registry tools from a verified local prebuilt bundle.
    PrebuiltInstall {
        #[arg(required = true)]
        selection: Vec<String>,
        /// Directory containing the delivered binary blobs.
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Versioned JSON manifest whose SHA-256 values are the integrity authority.
        #[arg(long)]
        manifest: std::path::PathBuf,
        /// Consumer installation home; binaries go below .workflow-tools/bin.
        #[arg(long)]
        home: std::path::PathBuf,
    },
    /// Uninstall one or more artifacts by id, category, or "all".
    Uninstall {
        #[arg(required = true)]
        selection: Vec<String>,
        /// Target installation root directory.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
    },
    /// Uninstall install-ctl itself from the installation root.
    SelfUninstall {
        /// Installation root directory.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
    },
    /// Compare installed binaries and saved source fingerprints without compiling.
    Status {
        /// Artifact ids, categories, or "all" (default: all installable artifacts).
        selection: Vec<String>,
        #[arg(long)]
        root: Option<std::path::PathBuf>,
    },
    /// Update installed, outdated binaries from the local checkout; never install missing ones.
    Update {
        /// Artifact ids, categories, or "all" (default: all installable artifacts).
        selection: Vec<String>,
        #[arg(long)]
        root: Option<std::path::PathBuf>,
        /// Run `cargo install` offline; the comparison build remains online.
        #[arg(long)]
        offline: bool,
        /// Require the comparison build's lockfile to remain unchanged.
        #[arg(long)]
        locked: bool,
    },
    /// Fetch and install the latest install-ctl revision from its repository.
    SelfUpdate,
    /// Render the registry projection to COMMANDS.md, or verify it is current.
    Catalog {
        #[arg(long)]
        check: bool,
    },
    /// Select and confirm installable artifacts interactively.
    Tui {
        /// Run `cargo install` without network access; the warm-up build remains online.
        #[arg(long)]
        offline: bool,
        /// Also prevent the warm-up build from updating Cargo.lock.
        #[arg(long)]
        locked: bool,
        /// Target installation root directory.
        #[arg(long)]
        root: Option<std::path::PathBuf>,
        /// Guidance source root; required when a guidance profile is supplied.
        #[arg(long)]
        guidance_source: Option<std::path::PathBuf>,
        /// Optional TOML guidance profile path (repeatable).
        #[arg(long = "guidance-profile")]
        guidance_profiles: Vec<std::path::PathBuf>,
        /// Guidance corpus id to include (repeatable; required with a profile).
        #[arg(long = "guidance-select")]
        guidance_select: Vec<String>,
        /// Guidance target root (required with a profile).
        #[arg(long)]
        guidance_target: Option<std::path::PathBuf>,
        /// Local delivered-binary directory used after TUI confirmation.
        #[arg(long)]
        prebuilt_bundle: Option<std::path::PathBuf>,
        /// Versioned local bundle manifest used after TUI confirmation.
        #[arg(long)]
        prebuilt_manifest: Option<std::path::PathBuf>,
        /// Consumer home; prebuilt bins install below .workflow-tools/bin.
        #[arg(long)]
        prebuilt_home: Option<std::path::PathBuf>,
    },
    /// Start a viewer server (alias for `viewer start`; kept top-level so
    /// existing `install-ctl start <viewer>` invocations, e.g. from
    /// .vscode/tasks.json, keep working verbatim).
    Start {
        server: String,
        #[arg(long, alias = "fg", short = 'f')]
        foreground: bool,
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Build + install the frontend linked to a viewer server (alias for
    /// `viewer prepare`; kept top-level for the same reason as `start`).
    Prepare { server: String },
    /// Viewer lifecycle surface (list/status/build/install/start/stop/restart/task/prepare/static-dir),
    /// nested under `viewer` so it does not collide with the top-level
    /// artifact-registry `--list`/`install`.
    Viewer {
        #[command(subcommand)]
        command: ViewerCmd,
    },
    /// Guidance profile planning/installation/autofix/one-command fetch
    /// (`guidance plan`, `install`, `autofix`, `get`).
    Guidance {
        #[command(subcommand)]
        command: guidance::GuidanceCmd,
    },
    /// Workspace-scoped helper commands.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCmd,
    },
}

/// Env var carrying the shadow copy's own path: its presence marks the
/// current process as a relaunched shadow copy (guards against relaunching
/// forever) and tells it where to schedule its own cleanup.
const SHADOW_ENV_VAR: &str = "INSTALL_CTL_SHADOW";

fn main() {
    if guidance::remote::transport::run_worker_if_requested() {
        return;
    }
    cleanup_shadow_copy_if_running_from_one();

    let cli = Cli::parse();

    if cli.list {
        match load_registry() {
            Ok(reg) => print_list(&reg.artifacts),
            Err(e) => fail(&e),
        }
        return;
    }

    match cli.command {
        Some(Command::Install {
            selection,
            no_force,
            offline,
            locked,
            root,
        }) => {
            let reg = match load_registry_v2() {
                Ok(reg) => reg,
                Err(e) => fail(&e),
            };
            let plan = match InstallPlan::from_inventory(
                &reg.entries,
                &selection,
                resolve_install_root(root),
                InstallOptions {
                    force: !no_force,
                    offline,
                    locked,
                },
            ) {
                Ok(plan) => plan,
                Err(error) => fail(&error),
            };
            if cli.dry_run {
                print_plan(&plan);
            } else {
                let report = run_install(&plan);
                print_apply_report(&report);
                if report.has_failures() {
                    std::process::exit(1);
                }
            }
        }
        Some(Command::PrebuiltInstall {
            selection,
            bundle,
            manifest,
            home,
        }) => {
            let registry = match load_registry_v2() {
                Ok(registry) => registry,
                Err(error) => fail(&error),
            };
            let plan = InstallPlan::from_inventory(
                &registry.entries,
                &selection,
                home.join(".workflow-tools"),
                InstallOptions::default(),
            )
            .unwrap_or_else(|error| fail(&error));
            let report = prebuilt::apply(&plan, &bundle, &manifest, &registry.entries, cli.dry_run);
            print_apply_report(&report);
            if report.has_failures() {
                std::process::exit(1);
            }
        }
        Some(Command::Uninstall { selection, root }) => {
            let reg = match load_registry() {
                Ok(reg) => reg,
                Err(e) => fail(&e),
            };
            let selected = match resolve_selection(&reg.artifacts, &selection) {
                Ok(s) => s,
                Err(e) => fail(&e),
            };
            if selected.is_empty() {
                fail("selection matched no artifacts");
            }
            let target_root = resolve_install_root(root);
            run_uninstall(&selected, &target_root);
        }
        Some(Command::SelfUninstall { root }) => {
            let target_root = resolve_install_root(root);
            run_self_uninstall(&target_root);
        }
        Some(Command::Status { selection, root }) => {
            let plan = status::selection_plan(selection, root, InstallOptions::default())
                .unwrap_or_else(|error| fail(&error));
            status::run(&plan).unwrap_or_else(|error| fail(&error));
        }
        Some(Command::Update {
            selection,
            root,
            offline,
            locked,
        }) => {
            let mut plan = status::selection_plan(
                selection,
                root,
                InstallOptions {
                    force: true,
                    offline,
                    locked,
                },
            )
            .unwrap_or_else(|error| fail(&error));
            status::retain_installed(&mut plan).unwrap_or_else(|error| fail(&error));
            if plan.selected.is_empty() {
                println!("No installed artifacts selected; nothing to update.");
            } else if cli.dry_run {
                println!("Matching source/binary fingerprints skip all builds; otherwise:");
                print_plan(&plan);
            } else {
                let report = run_install(&plan);
                print_apply_report(&report);
                if report.has_failures() {
                    std::process::exit(1);
                }
            }
        }
        Some(Command::SelfUpdate) => {
            relaunch_from_shadow_copy_if_replacing_self(true);
            if cli.dry_run {
                update::print_plan();
            } else if let Err(error) = update::run() {
                fail(&error);
            }
        }
        Some(Command::Catalog { check }) => {
            if let Err(error) = sync_catalog(check) {
                fail(&error);
            }
        }
        Some(Command::Tui {
            offline,
            locked,
            root,
            guidance_source,
            guidance_profiles,
            guidance_select,
            guidance_target,
            prebuilt_bundle,
            prebuilt_manifest,
            prebuilt_home,
        }) => {
            // A confirmed TUI selection cannot be replayed after this point.
            // Shadow before rendering regardless of the initial root: review
            // can edit that root to the running executable's destination.
            let target_root = resolve_install_root(root);
            relaunch_from_shadow_copy_if_replacing_self(true);
            let reg = match load_registry_v2() {
                Ok(reg) => reg,
                Err(e) => fail(&e),
            };
            let prebuilt = match (prebuilt_bundle, prebuilt_manifest, prebuilt_home) {
                (None, None, None) => None,
                (Some(bundle), Some(manifest), Some(home)) => Some((bundle, manifest, home)),
                _ => fail(
                    "--prebuilt-bundle, --prebuilt-manifest, and --prebuilt-home must be supplied together",
                ),
            };
            let target_root = prebuilt
                .as_ref()
                .map(|(_, _, home)| home.join(".workflow-tools"))
                .unwrap_or(target_root);
            let source_label = prebuilt.as_ref().map(|(bundle, manifest, _)| {
                format!(
                    "local prebuilt bundle={} manifest={} (SHA-256; no Cargo)",
                    bundle.display(),
                    manifest.display()
                )
            });
            let guidance_inputs = if guidance_profiles.is_empty() {
                if guidance_source.is_some()
                    || guidance_target.is_some()
                    || !guidance_select.is_empty()
                {
                    fail(
                        "guidance source, target, and select require at least one --guidance-profile",
                    );
                }
                vec![]
            } else {
                let source_root = guidance_source.unwrap_or_else(|| {
                    fail("--guidance-source is required with --guidance-profile")
                });
                let target_root = guidance_target.unwrap_or_else(|| {
                    fail("--guidance-target is required with --guidance-profile")
                });
                if guidance_select.is_empty() {
                    fail("--guidance-select is required with --guidance-profile");
                }
                guidance_profiles
                    .into_iter()
                    .map(|profile_path| review_plan::GuidanceInput {
                        source_root: source_root.clone(),
                        profile_path,
                        select: guidance_select.clone(),
                        target_root: target_root.clone(),
                        scope_override: None,
                        explicit_override: None,
                    })
                    .collect()
            };
            let destination_paths = guidance::destination::DestinationPaths::platform_default();
            match tui::run_with_source(
                &reg.entries,
                &guidance_inputs,
                &destination_paths,
                target_root,
                InstallOptions {
                    force: true,
                    offline,
                    locked,
                },
                source_label.as_deref(),
            ) {
                Ok(Some(review)) => {
                    // TUI self-replacement was handled before interaction.
                    // Do not replay its argv after confirmation: the root may
                    // have been edited since the pre-interaction check.
                    let report = if let Some((bundle, manifest, _)) = prebuilt {
                        review_plan::apply_review_plan(&review, |plan| {
                            prebuilt::apply(plan, &bundle, &manifest, &reg.entries, cli.dry_run)
                        })
                    } else {
                        review_plan::apply_review_plan(&review, run_install_after_tui_review)
                    };
                    print_apply_report(&report);
                    if report.has_failures() {
                        std::process::exit(1);
                    }
                }
                Ok(None) => println!("cancelled; no writes performed"),
                Err(error) => fail(&error),
            }
        }
        Some(Command::Start {
            server,
            foreground,
            extra,
        }) => run_viewer(|cfg, root| commands::cmd_start(cfg, root, &server, foreground, extra)),
        Some(Command::Prepare { server }) => {
            run_viewer(|cfg, root| commands::cmd_prepare(cfg, root, &server))
        }
        Some(Command::Viewer { command }) => {
            run_viewer(|cfg, root| dispatch_viewer(cfg, root, command))
        }
        Some(Command::Guidance { command }) => {
            if let Err(e) = guidance::run(command) {
                fail(&e);
            }
        }
        Some(Command::Workspace { command }) => match command {
            WorkspaceCmd::McpConfig {
                workspace,
                selection,
            } => {
                if let Err(e) =
                    commands::workspace::run_mcp_config(&workspace, &selection, cli.dry_run)
                {
                    fail(&e);
                }
            }
        },
        None => {
            eprintln!("error: no command given; try --list or `install <selection>`");
            std::process::exit(1);
        }
    }
}

/// Load `viewer-ctl.toml` from the same repo root the artifact registry
/// resolves, then run `action` against it, failing the process on error.
fn run_viewer<F>(action: F)
where
    F: FnOnce(&Config, &Path) -> Result<(), String>,
{
    let root = match registry::resolve_repo_root() {
        Ok(root) => root,
        Err(e) => fail(&e),
    };
    let cfg = match Config::load(&root) {
        Ok(c) => c,
        Err(e) => fail(&e),
    };
    if let Err(e) = action(&cfg, &root) {
        fail(&e);
    }
}

fn dispatch_viewer(cfg: &Config, root: &Path, command: ViewerCmd) -> Result<(), String> {
    match command {
        ViewerCmd::List => commands::cmd_list(cfg),
        ViewerCmd::Status { name } => commands::cmd_status(cfg, name.as_deref()),
        ViewerCmd::Build { name, kind } => commands::cmd_build(cfg, root, &name, kind),
        ViewerCmd::Install { name, kind } => commands::cmd_install(cfg, root, &name, kind),
        ViewerCmd::Uninstall { name, kind } => commands::cmd_uninstall(cfg, root, &name, kind),
        ViewerCmd::Start {
            server,
            foreground,
            extra,
        } => commands::cmd_start(cfg, root, &server, foreground, extra),
        ViewerCmd::Stop { server } => commands::cmd_stop(cfg, &server),
        ViewerCmd::Restart {
            server,
            foreground,
            extra,
        } => restart_server(cfg, root, &server, foreground, extra),
        ViewerCmd::Task { name } => commands::cmd_task(cfg, root, &name),
        ViewerCmd::Prepare { server } => commands::cmd_prepare(cfg, root, &server),
        ViewerCmd::StaticDir { server } => commands::cmd_static_dir(cfg, &server),
    }
}

fn restart_server(
    cfg: &Config,
    root: &Path,
    server: &str,
    foreground: bool,
    extra: Vec<String>,
) -> Result<(), String> {
    commands::cmd_stop(cfg, server)?;
    std::thread::sleep(Duration::from_millis(500));
    commands::cmd_start(cfg, root, server, foreground, extra)
}

fn print_list(artifacts: &[Artifact]) {
    let mut categories: Vec<&str> = artifacts.iter().map(|a| a.category.as_str()).collect();
    categories.sort();
    categories.dedup();

    for category in categories {
        println!("{category}:");
        for artifact in artifacts.iter().filter(|a| a.category == category) {
            println!("  {}", artifact.id);
        }
    }
}

fn run_uninstall(selected: &[Artifact], target_root: &Path) {
    use installable::Installable;
    let mut total_removed = 0;
    let mut total_missing = 0;
    for artifact in selected {
        match artifact.uninstall(target_root) {
            Ok(report) => {
                total_removed += report.removed.len();
                total_missing += report.missing.len();
                for path in report.removed {
                    println!("removed: {path}");
                }
            }
            Err(e) => fail(&e),
        }
    }
    println!("uninstalled {total_removed} file(s), {total_missing} missing");
}

fn run_self_uninstall(target_root: &Path) {
    let bin_dir = target_root.join("bin");
    let candidates = [bin_dir.join("install-ctl"), bin_dir.join("install-ctl.exe")];

    let mut removed = false;
    for candidate in &candidates {
        if candidate.is_file() {
            if let Err(e) = std::fs::remove_file(candidate) {
                fail(&format!("failed to self-uninstall install-ctl binary: {e}"));
            }
            println!("removed: {}", candidate.display());
            removed = true;
        }
    }

    if removed {
        println!(
            "successfully uninstalled install-ctl from {}",
            target_root.display()
        );
    } else {
        println!(
            "install-ctl binary was not found under {}",
            bin_dir.display()
        );
    }
}

fn print_plan(plan: &InstallPlan) {
    let selected = &plan.selected;
    let force = plan.options.force;
    let cargo_options = plan.options;
    let install_root = &plan.root;
    let repo_root = match registry::resolve_repo_root() {
        Ok(root) => root,
        Err(e) => fail(&e),
    };
    let target_dir = repo_root.join("target/install-tools");
    let (rust_artifacts, ext_artifacts) = split_by_kind(selected);

    if !rust_artifacts.is_empty() {
        let plan = match RustBuildPlan::new(&rust_artifacts, &repo_root) {
            Ok(p) => p,
            Err(e) => fail(&e),
        };
        println!("==> {}", plan.bins.join(", "));
        println!("    {}", plan.build_command_string());

        for group in group_by_path(&rust_artifacts) {
            let (ids, path, bins, features) = group_summary(&group);
            println!("==> {}", ids.join(", "));
            println!(
                "    {}",
                install_command_string(
                    &repo_root,
                    &target_dir,
                    path,
                    &bins,
                    &features,
                    force,
                    cargo_options,
                    install_root,
                )
            );
        }
    }

    for artifact in ext_artifacts {
        let script = artifact.npm_script.as_deref().unwrap_or("install:vsix");
        println!("==> {}", artifact.id);
        println!(
            "    (cd \"{}\" && npm ci && npm run {})",
            artifact.path, script
        );
    }
}

/// Split a selection into rust-binary and vscode-extension artifacts,
/// preserving relative order within each group.
fn split_by_kind(selected: &[Artifact]) -> (Vec<&Artifact>, Vec<&Artifact>) {
    let mut rust_artifacts = Vec::new();
    let mut ext_artifacts = Vec::new();
    for artifact in selected {
        match artifact.kind {
            ArtifactKind::RustBinary => rust_artifacts.push(artifact),
            ArtifactKind::VscodeExtension => ext_artifacts.push(artifact),
        }
    }
    (rust_artifacts, ext_artifacts)
}

/// Everything needed to build every selected rust-binary artifact in a
/// single `cargo build` call against the root workspace manifest.
///
/// Building through `cargo install --path <crate>` once per artifact (the
/// old approach) ignores the enclosing workspace: it re-resolves and
/// re-fetches each crate's dependency graph from scratch on every call,
/// bypassing the workspace `Cargo.lock` and `[patch]` table, and unifies
/// features only within that one crate — so siblings built with different
/// `--features` (e.g. `ticket` vs `ticket-mcp`) evict each other's cache
/// entries and rebuild shared dependencies repeatedly. Building every
/// requested binary together, against the workspace manifest, resolves
/// features once and reuses the workspace's own lockfile/patches.
struct RustBuildPlan {
    manifest_path: String,
    packages: Vec<String>,
    bins: Vec<String>,
    feature_flags: Vec<String>,
}

impl RustBuildPlan {
    fn new(artifacts: &[&Artifact], repo_root: &Path) -> Result<Self, String> {
        let manifest_path = repo_root.join("Cargo.toml").to_string_lossy().to_string();
        let mut packages: Vec<String> = Vec::new();
        let mut bins: Vec<String> = Vec::new();
        let mut feature_flags: Vec<String> = Vec::new();

        for artifact in artifacts {
            let pkg = package_name_for(repo_root, &artifact.path)?;
            if !packages.contains(&pkg) {
                packages.push(pkg.clone());
            }
            let bin = artifact.bin.clone().unwrap_or_else(|| artifact.id.clone());
            if !bins.contains(&bin) {
                bins.push(bin);
            }
            for feature in &artifact.features {
                let flag = format!("{pkg}/{feature}");
                if !feature_flags.contains(&flag) {
                    feature_flags.push(flag);
                }
            }
        }

        Ok(Self {
            manifest_path,
            packages,
            bins,
            feature_flags,
        })
    }

    fn build_args(&self) -> Vec<String> {
        let mut args = vec![
            "build".to_string(),
            "--manifest-path".to_string(),
            self.manifest_path.clone(),
            "--release".to_string(),
        ];
        for pkg in &self.packages {
            args.push("-p".to_string());
            args.push(pkg.clone());
        }
        for bin in &self.bins {
            args.push("--bin".to_string());
            args.push(bin.clone());
        }
        if !self.feature_flags.is_empty() {
            args.push("--features".to_string());
            args.push(self.feature_flags.join(","));
        }
        args
    }

    fn build_command_string(&self) -> String {
        std::iter::once("cargo".to_string())
            .chain(self.build_args())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Read `[package].name` out of `<repo_root>/<artifact_path>/Cargo.toml`.
fn package_name_for(repo_root: &Path, artifact_path: &str) -> Result<String, String> {
    let manifest_path = repo_root.join(artifact_path).join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read {}: {e}", manifest_path.display()))?;
    let value: toml::Value = text
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", manifest_path.display()))?;
    value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("no [package].name in {}", manifest_path.display()))
}

/// Group rust-binary artifacts that share a source path (e.g. `ticket` +
/// `ticket-mcp`, or `spec-cli` + `spec-mcp`) so a single `cargo install`
/// call can install every bin from that crate (cargo install only accepts
/// one `--path` per invocation, so distinct crates still need one call
/// each).
fn group_by_path<'a>(artifacts: &[&'a Artifact]) -> Vec<Vec<&'a Artifact>> {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: std::collections::HashMap<&str, Vec<&'a Artifact>> =
        std::collections::HashMap::new();
    for artifact in artifacts {
        groups.entry(artifact.path.as_str()).or_insert_with(|| {
            order.push(artifact.path.as_str());
            Vec::new()
        });
        groups
            .get_mut(artifact.path.as_str())
            .unwrap()
            .push(artifact);
    }
    order
        .into_iter()
        .map(|path| groups.remove(path).unwrap())
        .collect()
}

/// Collapse a group of artifacts sharing one source path into the ids,
/// shared path, unique `--bin` names, and unioned `--features` needed to
/// install all of them in one `cargo install` call.
fn group_summary<'a>(
    group: &[&'a Artifact],
) -> (Vec<&'a str>, &'a str, Vec<&'a str>, Vec<&'a str>) {
    let path = group[0].path.as_str();
    let mut ids = Vec::new();
    let mut bins: Vec<&str> = Vec::new();
    let mut features: Vec<&str> = Vec::new();
    for artifact in group {
        ids.push(artifact.id.as_str());
        let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
        if !bins.contains(&bin) {
            bins.push(bin);
        }
        for feature in &artifact.features {
            let feature = feature.as_str();
            if !features.contains(&feature) {
                features.push(feature);
            }
        }
    }
    (ids, path, bins, features)
}

#[cfg(test)]
struct CargoInstallOptions {
    offline: bool,
    locked: bool,
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn install_args(
    repo_root: &Path,
    target_dir: &Path,
    path: &str,
    bins: &[&str],
    features: &[&str],
    force: bool,
    cargo_options: CargoInstallOptions,
) -> Vec<String> {
    let root = resolve_install_root(None);
    let options = InstallOptions {
        force,
        offline: cargo_options.offline,
        locked: cargo_options.locked,
    };
    install_args_with_root(
        repo_root, target_dir, path, bins, features, force, options, &root,
    )
}

#[allow(clippy::too_many_arguments)]
fn install_args_with_root(
    repo_root: &Path,
    target_dir: &Path,
    path: &str,
    bins: &[&str],
    features: &[&str],
    force: bool,
    cargo_options: InstallOptions,
    install_root: &Path,
) -> Vec<String> {
    let full_path = repo_root.join(path).to_string_lossy().to_string();
    let target_dir_str = target_dir.to_string_lossy().to_string();
    let mut args = vec![
        "install".to_string(),
        "--path".to_string(),
        full_path,
        "--target-dir".to_string(),
        target_dir_str,
        "--root".to_string(),
        install_root.to_string_lossy().to_string(),
    ];
    if cargo_options.offline {
        args.push("--offline".to_string());
    }
    // Install the resolution just built, not a second, independently updated graph.
    args.push("--locked".to_string());
    for bin in bins {
        args.push("--bin".to_string());
        args.push((*bin).to_string());
    }
    if !features.is_empty() {
        args.push("--features".to_string());
        args.push(features.join(","));
    }
    if force {
        args.push("--force".to_string());
    }
    args
}

fn install_command_string(
    repo_root: &Path,
    target_dir: &Path,
    path: &str,
    bins: &[&str],
    features: &[&str],
    force: bool,
    cargo_options: InstallOptions,
    install_root: &Path,
) -> String {
    std::iter::once("cargo".to_string())
        .chain(install_args_with_root(
            repo_root,
            target_dir,
            path,
            bins,
            features,
            force,
            cargo_options,
            install_root,
        ))
        .collect::<Vec<_>>()
        .join(" ")
}

/// On Windows, a running `install-ctl.exe` holds an exclusive lock on its
/// own image file, so `cargo install` cannot overwrite `install-ctl.exe` in
/// place while this process is alive (`os error 5`, access denied) — killing
/// other instances (see `process::pids_by_image_name`'s own-PID exclusion)
/// isn't enough, since *this* process's lock is the one blocking the move.
///
/// When the current selection would replace the binary this process was
/// launched from, copy the running exe to a temp "shadow" file, relaunch the
/// exact same command from that shadow copy (which the loader locks instead),
/// and exit immediately: a live process's exe lock only releases on exit, not
/// on wait, so the original can't stick around to collect the child's exit
/// code without re-introducing the same lock.
///
/// No-op on non-Windows targets, where replacing a running executable's
/// backing file is a normal, supported operation, and when this process is
/// itself already a relaunched shadow copy (`SHADOW_ENV_VAR` set).
fn relaunch_from_shadow_copy_if_replacing_self(replacing_self: bool) {
    if !cfg!(windows) || std::env::var_os(SHADOW_ENV_VAR).is_some() {
        return;
    }

    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let Some(own_bin) = current_exe
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return;
    };

    if !replacing_self {
        return;
    }

    let shadow_path =
        std::env::temp_dir().join(format!("{own_bin}-shadow-{}.exe", std::process::id()));
    if let Err(e) = std::fs::copy(&current_exe, &shadow_path) {
        eprintln!(
            "warning: could not create shadow copy for self-update ({e}); install may fail \
             with 'access denied' while this process is running"
        );
        return;
    }

    println!("==> relaunching from a shadow copy so the running {own_bin}.exe can be replaced");
    match std::process::Command::new(&shadow_path)
        .args(std::env::args_os().skip(1))
        .env(SHADOW_ENV_VAR, &shadow_path)
        .env("INSTALL_CTL_ORIGINAL_EXE", &current_exe)
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("warning: failed to relaunch from shadow copy: {e}");
            let _ = std::fs::remove_file(&shadow_path);
        }
    }
}

/// A shadow copy can't delete the file backing its own running image either
/// (same lock as above), so schedule its removal via a short-lived detached
/// helper that waits for this process to exit first. Best-effort: a failure
/// here just leaves a harmless leftover file in the temp directory.
#[cfg(windows)]
fn cleanup_shadow_copy_if_running_from_one() {
    use std::os::windows::process::CommandExt;

    let Some(shadow_path) = std::env::var_os(SHADOW_ENV_VAR) else {
        return;
    };
    let shadow_path = windows_display_path(std::path::PathBuf::from(shadow_path));

    // Resolve the helpers by absolute path: under Git Bash / MSYS a GNU
    // `timeout.exe` (which rejects `/t`) shadows the System32 one on PATH.
    let system32 = std::path::PathBuf::from(
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()),
    )
    .join("System32");
    let ping_exe = windows_display_path(system32.join("PING.EXE"));

    // `raw_arg` bypasses Rust's MSVCRT-style quoting, which escapes inner
    // quotes as `\"` — a sequence `cmd` does not understand and would pass on
    // to `del` as part of the path. Leading `call` keeps the payload from
    // starting with a quote, which `cmd /C` would strip asymmetrically.
    let _ = std::process::Command::new(system32.join("cmd.exe"))
        .raw_arg("/C")
        .raw_arg(format!(
            r#"call "{ping_exe}" -n 3 127.0.0.1 >nul & del /f /q "{shadow_path}""#
        ))
        .spawn();
}

#[cfg(not(windows))]
fn cleanup_shadow_copy_if_running_from_one() {}

/// Render a path the way `cmd` expects it: canonicalized (so a POSIX-style
/// `TEMP` such as `/tmp` from Git Bash becomes a real drive-rooted path, which
/// `del` would otherwise reject as switch syntax) and without the `\\?\`
/// verbatim prefix.
#[cfg(windows)]
fn windows_display_path(path: std::path::PathBuf) -> String {
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let path = path.to_string_lossy().into_owned();
    path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
}

fn resolve_install_root(root: Option<std::path::PathBuf>) -> std::path::PathBuf {
    root.or_else(|| status::executable_root().unwrap_or_else(|error| fail(&error)))
        .or_else(|| std::env::var_os("CARGO_HOME").map(std::path::PathBuf::from))
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
        .unwrap_or_else(|| fail("cannot locate target root directory"))
}

fn run_install(plan: &InstallPlan) -> ApplyReport {
    run_install_with_shadow(plan, true)
}

fn run_install_after_tui_review(plan: &InstallPlan) -> ApplyReport {
    run_install_with_shadow(plan, false)
}

fn run_install_with_shadow(plan: &InstallPlan, may_relaunch: bool) -> ApplyReport {
    let mut report = ApplyReport::default();
    let selected = &plan.selected;
    let force = plan.options.force;
    let cargo_options = plan.options;
    let target_root = &plan.root;
    let replacing_self = selected.iter().any(|artifact| {
        artifact.kind == ArtifactKind::RustBinary
            && artifact
                .bin
                .as_deref()
                .unwrap_or(artifact.id.as_str())
                .eq_ignore_ascii_case("install-ctl")
    });
    relaunch_from_shadow_copy_if_replacing_self(may_relaunch && replacing_self);

    let repo_root = match registry::resolve_repo_root() {
        Ok(root) => root,
        Err(error) => {
            for artifact in selected {
                report.push(binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Failed,
                    &error,
                ));
            }
            return report;
        }
    };

    // Isolate build artifacts from the dev target dir so a running debug
    // binary's file lock doesn't block the build itself.
    let target_dir = repo_root.join("target/install-tools");
    unsafe {
        std::env::set_var("CARGO_TARGET_DIR", &target_dir);
    }

    for exclusion in &plan.excluded {
        report.push(ApplyAction {
            id: exclusion.id.clone(),
            target: std::path::PathBuf::new(),
            status: ActionStatus::Skipped,
            detail: exclusion.reason.clone(),
        });
    }
    let (rust_artifacts, ext_artifacts) = split_by_kind(selected);

    if !rust_artifacts.is_empty() {
        report.actions.extend(
            install_rust_binaries(
                &rust_artifacts,
                &repo_root,
                &target_dir,
                force,
                cargo_options,
                target_root,
            )
            .actions,
        );
    }

    for artifact in ext_artifacts {
        let (status, detail) = if report.has_failures() {
            (
                ActionStatus::Skipped,
                "not executed after an earlier failure".to_string(),
            )
        } else {
            match install_vscode_extension(artifact, &repo_root) {
                Ok(()) => (
                    ActionStatus::Written,
                    "extension install command completed".to_string(),
                ),
                Err(error) => (ActionStatus::Failed, error),
            }
        };
        report.push(ApplyAction {
            id: artifact.id.clone(),
            target: repo_root.join(&artifact.path),
            status,
            detail,
        });
    }
    report
}

fn binary_action(
    artifact: &Artifact,
    root: &Path,
    status: ActionStatus,
    detail: &str,
) -> ApplyAction {
    let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
    ApplyAction {
        id: artifact.id.clone(),
        target: root
            .join("bin")
            .join(format!("{bin}{}", std::env::consts::EXE_SUFFIX)),
        status,
        detail: detail.to_string(),
    }
}

fn print_apply_report(report: &ApplyReport) {
    report.render();
}

fn install_rust_binaries(
    artifacts: &[&Artifact],
    repo_root: &Path,
    target_dir: &Path,
    force: bool,
    cargo_options: InstallOptions,
    target_root: &Path,
) -> ApplyReport {
    let mut report = ApplyReport::default();
    let initial = match fingerprint::Snapshot::capture(repo_root, artifacts) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            for artifact in artifacts {
                report.push(binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Failed,
                    &error,
                ));
            }
            return report;
        }
    };
    let mut pending = Vec::new();
    for artifact in artifacts {
        match fingerprint::is_current(target_root, artifact, initial.source(artifact)) {
            Ok(true) => report.push(binary_action(
                artifact,
                target_root,
                ActionStatus::Unchanged,
                "source fingerprint and installed binary match; no build needed",
            )),
            Ok(false) => pending.push(*artifact),
            Err(error) => report.push(binary_action(
                artifact,
                target_root,
                ActionStatus::Failed,
                &error,
            )),
        }
    }
    if report.has_failures() || pending.is_empty() {
        return report;
    }
    let artifacts = pending.as_slice();
    let plan = match RustBuildPlan::new(artifacts, repo_root) {
        Ok(p) => p,
        Err(error) => {
            for artifact in artifacts {
                report.push(binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Failed,
                    &error,
                ));
            }
            return report;
        }
    };

    println!("==> {}", plan.bins.join(", "));

    // Warm-up build first: it writes into target_dir, never into
    // $CARGO_HOME/bin, so it never needs an installed binary's process
    // killed. It also produces the exact output `cargo install` would copy,
    // which the freshness check below compares against what's installed
    // before deciding whether killing/reinstalling is needed at all.
    if let Err(error) = build_rust_binaries(&plan, repo_root, target_dir, cargo_options.locked) {
        for artifact in artifacts {
            report.push(binary_action(
                artifact,
                target_root,
                ActionStatus::Failed,
                &error,
            ));
        }

        return report;
    }
    let snapshot = match fingerprint::Snapshot::capture(repo_root, artifacts) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            for artifact in artifacts {
                report.push(binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Failed,
                    &error,
                ));
            }
            return report;
        }
    };
    if artifacts
        .iter()
        .any(|artifact| initial.source(artifact) != snapshot.source(artifact))
    {
        // An unlocked warm-up may legitimately refresh Cargo.lock. Rebuild once
        // against the final snapshot before claiming those inputs were installed.
        if let Err(error) = build_rust_binaries(&plan, repo_root, target_dir, true)
            .and_then(|()| fingerprint::Snapshot::capture(repo_root, artifacts))
            .and_then(|after| {
                if after.sources == snapshot.sources {
                    Ok(())
                } else {
                    Err("source inputs changed during build; retry installation".into())
                }
            })
        {
            for artifact in artifacts {
                report.push(binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Failed,
                    &error,
                ));
            }
            return report;
        }
    }

    // cargo install only accepts one --path per call, so distinct crates
    // still need one call each; siblings sharing a crate (ticket +
    // ticket-mcp) are grouped into a single call via --bin/--features union.
    for group in group_by_path(artifacts) {
        if report.has_failures() {
            for artifact in group {
                report.push(binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Skipped,
                    "not executed after an earlier failure",
                ));
            }
            continue;
        }
        let (ids, path, bins, features) = group_summary(&group);

        // Skip bins whose freshly built output is byte-identical to what's
        // already installed: cargo install would be a no-op for them, so
        // there is nothing to gain from killing their running process.
        let stale_bins: Vec<&str> = bins
            .iter()
            .copied()
            .filter(|bin| !freshness::is_up_to_date(target_dir, target_root, bin))
            .collect();
        for artifact in &group {
            let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
            if !stale_bins.contains(&bin) {
                let result = fingerprint::record(target_root, artifact, snapshot.source(artifact));
                report.push(binary_action(
                    artifact,
                    target_root,
                    if result.is_ok() {
                        ActionStatus::Unchanged
                    } else {
                        ActionStatus::Failed
                    },
                    &result
                        .err()
                        .unwrap_or_else(|| "byte-identical; source fingerprint recorded".into()),
                ));
            }
        }
        if stale_bins.is_empty() {
            println!("==> {} (up to date, skipping)", ids.join(", "));
            continue;
        }

        // Stop only the binaries that are actually about to change: a
        // locked exe on Windows blocks cargo install's replace step.
        for bin in &stale_bins {
            let running = process::pids_by_image_name(bin);
            for pid in running {
                process::print_process_info(pid, bin);
                if !process::kill_process(pid, bin) {
                    eprintln!(
                        "warning: [{bin}] failed to stop PID {} before install",
                        pid.as_u32()
                    );
                }
            }
        }

        let args = install_args_with_root(
            repo_root,
            target_dir,
            path,
            &stale_bins,
            &features,
            force,
            cargo_options,
            target_root,
        );
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let label = ids.join(", ");
        println!("==> {label}");
        let result = shell::run_cmd_args("cargo", &arg_refs, repo_root, &label).and_then(|()| {
            let after = fingerprint::Snapshot::capture(repo_root, artifacts)?;
            if after.sources != snapshot.sources {
                return Err(
                    "source inputs changed during installation; fingerprints not recorded".into(),
                );
            }
            Ok(())
        });
        for artifact in &group {
            let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
            if !stale_bins.contains(&bin) {
                continue;
            }
            let action = match &result {
                Err(error) => binary_action(artifact, target_root, ActionStatus::Failed, error),
                Ok(()) if freshness::is_up_to_date(target_dir, target_root, bin) => {
                    match fingerprint::record(target_root, artifact, snapshot.source(artifact)) {
                        Ok(()) => binary_action(
                            artifact,
                            target_root,
                            ActionStatus::Written,
                            "installed binary verified; source fingerprint recorded",
                        ),
                        Err(error) => {
                            binary_action(artifact, target_root, ActionStatus::Failed, &error)
                        }
                    }
                }
                Ok(()) => binary_action(
                    artifact,
                    target_root,
                    ActionStatus::Failed,
                    "installed binary is missing or differs from the freshly built binary",
                ),
            };
            report.push(action);
        }
    }
    report
}

fn build_rust_binaries(
    plan: &RustBuildPlan,
    repo_root: &Path,
    target_dir: &Path,
    locked: bool,
) -> Result<(), String> {
    let mut args = plan.build_args();
    args.push("--target-dir".into());
    args.push(target_dir.to_string_lossy().into_owned());
    if locked {
        args.push("--locked".into());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    shell::run_cmd_args("cargo", &arg_refs, repo_root, &plan.bins.join(", "))
}

fn install_vscode_extension(artifact: &Artifact, repo_root: &Path) -> Result<(), String> {
    println!("==> {}", artifact.id);
    let script = artifact.npm_script.as_deref().unwrap_or("install:vsix");
    let ext_dir = repo_root.join(&artifact.path);

    if !ext_dir.join("node_modules").is_dir()
        && let Err(e) = shell::run_cmd_args("npm", &["ci"], &ext_dir, &artifact.id)
    {
        return Err(e);
    }

    shell::run_cmd_args("npm", &["run", script], &ext_dir, &artifact.id)
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1);
}
