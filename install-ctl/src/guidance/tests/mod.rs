//! Integration tests exercising `build_plan`/`install_plan` end-to-end
//! against real temporary directories (never the host user/system dirs).

use std::{collections::HashSet, fs, path::Path, path::PathBuf};

use serde::Deserialize;
use tempfile::TempDir;

use crate::guidance::destination::DestinationPaths;
use crate::guidance::install::install_plan;
use crate::guidance::plan::{PlanInputs, build_plan};
use crate::guidance::profile::DestinationScopeKind;

mod cli;
mod closure;
mod diagnostics;
mod fixtures;
mod get;
mod install;
mod installer_script;
mod link_closure;
mod scopes;
mod uninstall;

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn test_destination_paths(tmp: &TempDir) -> DestinationPaths {
    DestinationPaths {
        user_root: tmp.path().join("user-dest"),
        system_root: tmp.path().join("system-dest"),
    }
}

fn direct_profile(dest_scope: &str, dest_path: Option<&str>) -> String {
    let dest_path_line = dest_path
        .map(|p| format!("path = \"{p}\"\n"))
        .unwrap_or_default();
    format!(
        "[profile]\nid = \"example\"\n\n[[corpus]]\nid = \"root\"\npaths = [\"prompt.md\"]\n\n[destination]\nscope = \"{dest_scope}\"\n{dest_path_line}"
    )
}
