//! Clap types for the viewer lifecycle subcommand surface (ported from
//! viewer-ctl's `cli.rs`). Nested under `install-ctl viewer <...>` — see
//! `main.rs` for the top-level surface and the list/install collision note.

use clap::{Subcommand, ValueEnum};

#[derive(Subcommand)]
pub enum ViewerCmd {
    /// List every component defined in viewer-ctl.toml.
    List,
    /// Show running state of one or all servers.
    Status { name: Option<String> },
    /// Build artifacts for a component (no install).
    Build {
        name: String,
        /// Restrict to a single component kind when the name is ambiguous.
        #[arg(long)]
        kind: Option<KindArg>,
    },
    /// Install built artifacts.
    /// - server     → cargo install --path <source_dir> --force
    /// - frontend   → copy build_output to <install_root>/<name>/
    /// - extension  → kind-specific (e.g. vscode sync)
    Install {
        name: String,
        #[arg(long)]
        kind: Option<KindArg>,
    },
    /// Uninstall installed artifacts for a component.
    Uninstall {
        name: String,
        #[arg(long)]
        kind: Option<KindArg>,
    },
    /// Start a server. Sets STATIC_DIR if a frontend is linked AND installed.
    Start {
        server: String,
        /// Keep the server in the foreground: inherit the current stdout/stderr
        /// and block until the process exits. By default the server is
        /// detached and viewer-ctl exits once the port is ready.
        #[arg(long, alias = "fg", short = 'f')]
        foreground: bool,
        /// Extra args forwarded to the server binary.
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Stop the server (kills the process listening on its port).
    Stop { server: String },
    /// Stop then start the server.
    Restart {
        server: String,
        /// Keep the server in the foreground (see `start --foreground`).
        #[arg(long, alias = "fg", short = 'f')]
        foreground: bool,
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Run a named task (sequence of shell commands).
    Task { name: String },
    /// Build + install the frontend linked to the given server.
    ///
    /// Designed for use as a vscode `preLaunchTask` so a debug launch of the
    /// server binary serves a freshly built frontend bundle. A no-op if the
    /// server has no linked frontend. Prints the resolved install dir to
    /// stdout so it can be captured for `STATIC_DIR`.
    Prepare { server: String },
    /// Print the resolved STATIC_DIR for a server's linked frontend.
    ///
    /// Exits with an error if no frontend is linked. Does not build or
    /// install — useful for scripting around the install layout.
    StaticDir { server: String },
}

#[derive(Clone, Copy, ValueEnum, Debug)]
pub enum KindArg {
    Server,
    Frontend,
    Extension,
}
