use crate::{
    fingerprint, freshness,
    plan::{InstallOptions, InstallPlan},
    registry, resolve_install_root,
};

pub fn executable_root() -> Result<Option<std::path::PathBuf>, String> {
    let exe = if std::env::var_os(crate::SHADOW_ENV_VAR).is_some() {
        std::env::var_os("INSTALL_CTL_ORIGINAL_EXE")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "shadow installer is missing its original executable path".to_string())?
    } else {
        std::env::current_exe()
            .map_err(|error| format!("cannot locate running installer: {error}"))?
    };
    Ok(exe
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "bin"))
        .and_then(|bin| bin.parent())
        .map(std::path::Path::to_path_buf))
}

pub fn selection_plan(
    mut selection: Vec<String>,
    root: Option<std::path::PathBuf>,
    options: InstallOptions,
) -> Result<InstallPlan, String> {
    if selection.is_empty() {
        selection.push("all".into());
    }
    let registry = registry::load_registry_v2()?;
    let plan = InstallPlan::from_inventory(
        &registry.entries,
        &selection,
        resolve_install_root(root),
        options,
    )?;
    for exclusion in &plan.excluded {
        eprintln!("skipped: {}: {}", exclusion.id, exclusion.reason);
    }
    Ok(plan)
}

pub fn retain_installed(plan: &mut InstallPlan) -> Result<(), String> {
    let mut installed = Vec::new();
    for artifact in &plan.selected {
        let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
        if freshness::is_installed(&plan.root, bin)? {
            installed.push(artifact.clone());
        }
    }
    plan.selected = installed;
    Ok(())
}

pub fn run(plan: &InstallPlan) -> Result<(), String> {
    let mut installed = Vec::new();
    for artifact in &plan.selected {
        let bin = artifact.bin.as_deref().unwrap_or(&artifact.id);
        if freshness::is_installed(&plan.root, bin)? {
            installed.push(artifact);
        }
    }
    let repo_root = registry::resolve_repo_root()?;
    let snapshot = if installed.is_empty() {
        None
    } else {
        Some(fingerprint::Snapshot::capture(&repo_root, &installed)?)
    };
    // Validate the entire selection before printing a success table.
    let rows = plan
        .selected
        .iter()
        .map(|artifact| {
            let status = match snapshot
                .as_ref()
                .filter(|snapshot| snapshot.sources.contains_key(&artifact.id))
            {
                None => "not-installed",
                Some(snapshot)
                    if fingerprint::is_current(
                        &plan.root,
                        artifact,
                        snapshot.source(artifact),
                    )? =>
                {
                    "up-to-date"
                }
                Some(_) => "outdated",
            };
            Ok((&artifact.id, status))
        })
        .collect::<Result<Vec<_>, String>>()?;
    println!("artifact\tstatus");
    for (id, status) in rows {
        println!("{id}\t{status}");
    }
    Ok(())
}
