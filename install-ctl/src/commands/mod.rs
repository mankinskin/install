//! Command implementations for the viewer lifecycle surface (ported from
//! viewer-ctl).
//!
//! - `list` / `status` — read-only inspection
//! - `build` / `install` — dispatched per component kind via [`for_matching`]
//! - `start` / `stop` / `restart` — see [`server`]
//! - `task` — see [`task`]

pub mod extension;
pub mod frontend;
pub mod server;
pub mod task;

use std::path::Path;

use crate::{
    cli::KindArg,
    config::{Component, Config},
    info,
    paths::disp,
    process::{kill_process, pids_by_image_name, pids_on_port},
    warn,
};

use self::{
    extension::{build_extension, install_extension, uninstall_extension},
    frontend::{build_frontend, install_frontend, uninstall_frontend},
    server::{build_server, install_server, uninstall_server},
};

pub use self::{
    server::{cmd_start, cmd_stop},
    task::cmd_task,
};

// ── list / status ────────────────────────────────────────────────────────────

pub fn cmd_list(cfg: &Config) -> Result<(), String> {
    println!("servers:");
    for s in &cfg.servers {
        let fe = cfg
            .frontend_for_server(&s.name)
            .map(|f| format!(" ↔ frontend `{}`", f.name))
            .unwrap_or_default();
        println!(
            "  {:<16}  package={}  port={}{}",
            s.name, s.package, s.port, fe
        );
    }
    println!("frontends:");
    for f in &cfg.frontends {
        let install = cfg.frontend_install_dir(&f.name);
        println!(
            "  {:<16}  serves={:<14}  install={}",
            f.name,
            f.serves.as_deref().unwrap_or("-"),
            disp(&install)
        );
    }
    println!("extensions:");
    for e in &cfg.extensions {
        println!("  {:<16}  kind={}  source={}", e.name, e.kind, e.source_dir);
    }
    println!("tasks:");
    for t in &cfg.tasks {
        let desc = if t.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", t.description)
        };
        println!("  {}{desc}", t.name);
    }
    Ok(())
}

pub fn cmd_status(cfg: &Config, name: Option<&str>) -> Result<(), String> {
    let servers: Vec<_> = match name {
        Some(n) => match cfg.server(n) {
            Some(s) => vec![s],
            None => return Err(format!("no [[server]] named `{n}` in viewer-ctl.toml")),
        },
        None => cfg.servers.iter().collect(),
    };
    for s in servers {
        let pids = pids_on_port(s.port);
        if pids.is_empty() {
            println!("  {:<16}  port={}  (down)", s.name, s.port);
        } else {
            let pid_list: Vec<String> = pids.iter().map(|p| p.as_u32().to_string()).collect();
            println!(
                "  {:<16}  port={}  pids=[{}]",
                s.name,
                s.port,
                pid_list.join(",")
            );
        }
    }
    Ok(())
}

// ── build / install dispatch ─────────────────────────────────────────────────

/// Apply a per-kind action to every component matching `name` in `cfg`.
///
/// When `kind` is `None` and multiple matches exist, all are processed in
/// the order: server, frontend, extension.
fn for_matching<F>(
    cfg: &Config,
    name: &str,
    kind: Option<KindArg>,
    mut action: F,
) -> Result<(), String>
where
    F: FnMut(Component<'_>) -> Result<(), String>,
{
    let mut matched = false;

    if matches!(kind, None | Some(KindArg::Server))
        && let Some(s) = cfg.server(name)
    {
        action(Component::Server(s))?;
        matched = true;
    }
    if matches!(kind, None | Some(KindArg::Frontend))
        && let Some(f) = cfg.frontend(name)
    {
        action(Component::Frontend(f))?;
        matched = true;
    }
    if matches!(kind, None | Some(KindArg::Extension))
        && let Some(e) = cfg.extension(name)
    {
        action(Component::Extension(e))?;
        matched = true;
    }

    if !matched {
        return Err(format!(
            "no component named `{name}` (kind filter: {:?}). Run `install-ctl viewer list`.",
            kind
        ));
    }
    Ok(())
}

pub fn cmd_build(
    cfg: &Config,
    root: &Path,
    name: &str,
    kind: Option<KindArg>,
) -> Result<(), String> {
    for_matching(cfg, name, kind, |c| match c {
        Component::Server(s) => build_server(root, s),
        Component::Frontend(f) => build_frontend(root, f),
        Component::Extension(e) => build_extension(root, e),
    })
}

pub fn cmd_install(
    cfg: &Config,
    root: &Path,
    name: &str,
    kind: Option<KindArg>,
) -> Result<(), String> {
    for_matching(cfg, name, kind, |c| match c {
        Component::Server(s) => install_server(root, s),
        Component::Frontend(f) => install_frontend(cfg, root, f),
        Component::Extension(e) => install_extension(cfg, root, e),
    })
}

pub fn cmd_uninstall(
    cfg: &Config,
    root: &Path,
    name: &str,
    kind: Option<KindArg>,
) -> Result<(), String> {
    for_matching(cfg, name, kind, |c| match c {
        Component::Server(s) => uninstall_server(root, s),
        Component::Frontend(f) => uninstall_frontend(cfg, f),
        Component::Extension(e) => uninstall_extension(cfg, root, e),
    })
}

// ── prepare / static-dir ─────────────────────────────────────────────────────

/// Build + install the frontend linked to `server_name` and print its
/// resolved install dir.
///
/// Intended as a vscode `preLaunchTask` for a debug launch of the server
/// binary. The server is expected to read `STATIC_DIR` on boot; the launch
/// config sets that to the printed path (or to the deterministic
/// `<install_root>/<frontend>` location).
///
/// Side-effect: any process listening on the server's port is killed
/// first. This frees the file lock on `target/debug/<server>.exe` on
/// Windows so the subsequent cargo debug build can replace the binary.
///
/// If the server exists but no frontend declares `serves = "<server>"`,
/// the port is still freed and the command succeeds without building.
pub fn cmd_prepare(cfg: &Config, root: &Path, server_name: &str) -> Result<(), String> {
    let server = cfg
        .server(server_name)
        .ok_or_else(|| format!("no [[server]] named `{server_name}` in viewer-ctl.toml"))?;

    // Free the port so the cargo debug build can replace the running binary
    // (Windows holds a file lock on the .exe of any running process).
    // cmd_stop is idempotent: it succeeds silently if nothing is listening.
    server::cmd_stop(cfg, &server.name)?;

    // Belt-and-braces: also kill any orphaned process whose image name matches
    // the server's package (e.g. a previous lldb debug session that exited
    // ungracefully and left the binary running on a stale or no port).
    // Without this the next `cargo build` fails with
    // "failed to remove file ... Access denied" on Windows.
    let orphans = pids_by_image_name(&server.package);
    if !orphans.is_empty() {
        warn!(
            "prepare",
            "killing {} orphaned `{}` process(es): {:?}",
            orphans.len(),
            server.package,
            orphans.iter().map(|p| p.as_u32()).collect::<Vec<_>>()
        );
        for pid in &orphans {
            kill_process(*pid, "prepare");
        }
    }

    let Some(fe) = cfg.frontend_for_server(&server.name) else {
        info!(
            "prepare",
            "server `{}` has no linked frontend; nothing to build", server.name
        );
        return Ok(());
    };
    build_frontend(root, fe)?;
    install_frontend(cfg, root, fe)?;
    let install = cfg.frontend_install_dir(&fe.name);
    println!("{}", disp(&install));
    Ok(())
}

/// Print the resolved STATIC_DIR for a server's linked frontend.
///
/// Errors if the server is unknown or has no linked frontend. Does not
/// build or install.
pub fn cmd_static_dir(cfg: &Config, server_name: &str) -> Result<(), String> {
    let server = cfg
        .server(server_name)
        .ok_or_else(|| format!("no [[server]] named `{server_name}` in viewer-ctl.toml"))?;
    let fe = cfg.frontend_for_server(&server.name).ok_or_else(|| {
        format!(
            "server `{}` has no linked frontend in viewer-ctl.toml",
            server.name
        )
    })?;
    let install = cfg.frontend_install_dir(&fe.name);
    println!("{}", disp(&install));
    Ok(())
}
