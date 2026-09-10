//! Multi-step task runner (ported from viewer-ctl).

use std::path::Path;

use crate::{config::Config, info, paths::disp, shell::run_cmd_owned, warn};

pub fn cmd_task(cfg: &Config, root: &Path, name: &str) -> Result<(), String> {
    let task = cfg
        .task(name)
        .ok_or_else(|| format!("no [[task]] named `{name}` in viewer-ctl.toml"))?;
    let tag = task.name.as_str();
    if !task.description.is_empty() {
        info!(tag, "{}", task.description);
    }
    for step in &task.steps {
        let dir = root.join(&step.dir);
        info!(tag, "step: {} (cwd={})", step.cmd.join(" "), disp(&dir));
        let res = run_cmd_owned(&step.cmd, &dir, tag);
        if let Err(e) = res {
            if step.allow_failure {
                warn!(tag, "step failed (allow_failure=true): {e}");
            } else {
                return Err(e);
            }
        }
    }
    info!(tag, "done.");
    Ok(())
}
