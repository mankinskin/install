mod cli;
mod commands;
mod config;
mod freshness;
mod guidance;
pub mod installable;
mod logging;
mod paths;
mod process;
mod registry;
mod selection;
mod shell;

use std::{path::Path, time::Duration};

use clap::{Parser, Subcommand};
use cli::ViewerCmd;
use config::Config;
use registry::{Artifact, ArtifactKind, load_registry, sync_catalog};
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
    /// Render the registry projection to COMMANDS.md, or verify it is current.
    Catalog {
        #[arg(long)]
        check: bool,
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
}

/// Env var carrying the shadow copy's own path: its presence marks the
/// current process as a relaunched shadow copy (guards against relaunching
/// forever) and tells it where to schedule its own cleanup.
const SHADOW_ENV_VAR: &str = "INSTALL_CTL_SHADOW";

fn main() {
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
        }) => {
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
            let force = !no_force;
            if cli.dry_run {
                print_plan(&selected, force);
            } else {
                run_install(&selected, force);
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
            let target_root = match root {
                Some(r) => r,
                None => match std::env::var_os("CARGO_HOME")
                    .map(std::path::PathBuf::from)
                    .or_else(|| dirs::home_dir().map(|h| h.join(".cargo")))
                {
                    Some(dir) => dir,
                    None => fail("cannot locate target root directory"),
                },
            };
            run_uninstall(&selected, &target_root);
        }
        Some(Command::SelfUninstall { root }) => {
            let target_root = match root {
                Some(r) => r,
                None => match std::env::var_os("CARGO_HOME")
                    .map(std::path::PathBuf::from)
                    .or_else(|| dirs::home_dir().map(|h| h.join(".cargo")))
                {
                    Some(dir) => dir,
                    None => fail("cannot locate target root directory"),
                },
            };
            run_self_uninstall(&target_root);
        }
        Some(Command::Catalog { check }) => {
            if let Err(error) = sync_catalog(check) {
                fail(&error);
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

fn print_plan(selected: &[Artifact], force: bool) {
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
                install_command_string(&repo_root, &target_dir, path, &bins, &features, force)
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

#[allow(clippy::too_many_arguments)]
fn install_args(
    repo_root: &Path,
    target_dir: &Path,
    path: &str,
    bins: &[&str],
    features: &[&str],
    force: bool,
) -> Vec<String> {
    let full_path = repo_root.join(path).to_string_lossy().to_string();
    let target_dir_str = target_dir.to_string_lossy().to_string();
    let mut args = vec![
        "install".to_string(),
        "--path".to_string(),
        full_path,
        "--target-dir".to_string(),
        target_dir_str,
        "--offline".to_string(),
    ];
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
) -> String {
    std::iter::once("cargo".to_string())
        .chain(install_args(
            repo_root, target_dir, path, bins, features, force,
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
fn relaunch_from_shadow_copy_if_replacing_self(selected: &[Artifact]) {
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

    let installing_self = selected.iter().any(|a| {
        a.kind == ArtifactKind::RustBinary
            && a.bin
                .as_deref()
                .unwrap_or(a.id.as_str())
                .eq_ignore_ascii_case(&own_bin)
    });
    if !installing_self {
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

fn run_install(selected: &[Artifact], force: bool) {
    relaunch_from_shadow_copy_if_replacing_self(selected);

    let repo_root = match registry::resolve_repo_root() {
        Ok(root) => root,
        Err(e) => fail(&e),
    };

    // Isolate build artifacts from the dev target dir so a running debug
    // binary's file lock doesn't block the build itself.
    let target_dir = repo_root.join("target/install-tools");
    unsafe {
        std::env::set_var("CARGO_TARGET_DIR", &target_dir);
    }

    let (rust_artifacts, ext_artifacts) = split_by_kind(selected);

    if !rust_artifacts.is_empty() {
        install_rust_binaries(&rust_artifacts, &repo_root, &target_dir, force);
    }

    for artifact in ext_artifacts {
        install_vscode_extension(artifact, &repo_root);
    }
}

fn install_rust_binaries(
    artifacts: &[&Artifact],
    repo_root: &Path,
    target_dir: &Path,
    force: bool,
) {
    let plan = match RustBuildPlan::new(artifacts, repo_root) {
        Ok(p) => p,
        Err(e) => fail(&e),
    };

    println!("==> {}", plan.bins.join(", "));

    // Warm-up build first: it writes into target_dir, never into
    // $CARGO_HOME/bin, so it never needs an installed binary's process
    // killed. It also produces the exact output `cargo install` would copy,
    // which the freshness check below compares against what's installed
    // before deciding whether killing/reinstalling is needed at all.
    let build_args = plan.build_args();
    let arg_refs: Vec<&str> = build_args.iter().map(String::as_str).collect();
    let label = plan.bins.join(", ");
    if let Err(e) = shell::run_cmd_args("cargo", &arg_refs, repo_root, &label) {
        fail(&e);
    }

    // cargo install only accepts one --path per call, so distinct crates
    // still need one call each; siblings sharing a crate (ticket +
    // ticket-mcp) are grouped into a single call via --bin/--features union.
    for group in group_by_path(artifacts) {
        let (ids, path, bins, features) = group_summary(&group);

        // Skip bins whose freshly built output is byte-identical to what's
        // already installed: cargo install would be a no-op for them, so
        // there is nothing to gain from killing their running process.
        let stale_bins: Vec<&str> = bins
            .iter()
            .copied()
            .filter(|bin| !freshness::is_up_to_date(target_dir, bin))
            .collect();
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

        let args = install_args(repo_root, target_dir, path, &stale_bins, &features, force);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let label = ids.join(", ");
        println!("==> {label}");
        if let Err(e) = shell::run_cmd_args("cargo", &arg_refs, repo_root, &label) {
            fail(&e);
        }
    }
}

fn install_vscode_extension(artifact: &Artifact, repo_root: &Path) {
    println!("==> {}", artifact.id);
    let script = artifact.npm_script.as_deref().unwrap_or("install:vsix");
    let ext_dir = repo_root.join(&artifact.path);

    if !ext_dir.join("node_modules").is_dir()
        && let Err(e) = shell::run_cmd_args("npm", &["ci"], &ext_dir, &artifact.id)
    {
        fail(&e);
    }

    if let Err(e) = shell::run_cmd_args("npm", &["run", script], &ext_dir, &artifact.id) {
        fail(&e);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1);
}
