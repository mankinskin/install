use super::{GuidanceLinkArgs, links};

pub(super) fn run(args: GuidanceLinkArgs, operation: links::LinkOperation) -> Result<(), String> {
    if args.harness == links::Harness::CopilotCli && (!args.sources.is_empty() || args.migrate) {
        return Err("--source and --migrate are only supported with --harness vscode".to_owned());
    }
    if operation == links::LinkOperation::Unlink && args.migrate {
        return Err("--migrate is only valid with guidance link".to_owned());
    }
    if args.harness == links::Harness::Vscode {
        let plan =
            match links::settings::build_plan(&args.repo, &args.sources, operation, args.migrate) {
                Ok(plan) => plan,
                Err(error) => {
                    if args.json {
                        println!(
                            "{}",
                            serde_json::json!({"plan": null, "applied": false, "error": error})
                        );
                    }
                    return Err(error);
                }
            };
        let result = if !plan.links.diagnostics.is_empty() {
            Err(format!(
                "blocking link plan: {}",
                plan.links.diagnostics.join("; ")
            ))
        } else if args.apply {
            links::settings::apply_plan(&plan)
        } else {
            Ok(())
        };
        if args.json {
            let report = serde_json::json!({
                "plan": &plan,
                "applied": args.apply && result.is_ok(),
                "error": result.as_ref().err(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .map_err(|error| format!("cannot serialize link report: {error}"))?
            );
        } else {
            for entry in &plan.links.entries {
                println!(
                    "{:?}: {} -> {} (source {})",
                    entry.action,
                    entry.destination,
                    entry.target.display(),
                    entry.source
                );
            }
            for source in &plan.links.skipped_sources {
                println!("Skipped missing source: {source}");
            }
            for diagnostic in &plan.links.diagnostics {
                println!("Blocked: {diagnostic}");
            }
            for change in &plan.configuration_changes {
                println!("Configuration: {change}");
            }
            for (destination, target) in &plan.links.migrations {
                println!(
                    "Migrate legacy alias: {destination} -> {} into named links",
                    target.display()
                );
            }
            if args.apply && result.is_ok() {
                println!("Applied VS Code links, discovery settings, and ownership");
            }
        }
        return result;
    }
    let plan = links::build_link_plan(&args.repo, args.harness, operation)?;
    let result = if !plan.diagnostics.is_empty() {
        Err(format!(
            "blocking link plan: {}",
            plan.diagnostics.join("; ")
        ))
    } else if args.apply {
        links::apply_link_plan(&plan)
    } else {
        Ok(())
    };
    if args.json {
        let report = serde_json::json!({
            "plan": &plan,
            "applied": args.apply && result.is_ok(),
            "error": result.as_ref().err(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("cannot serialize link report: {error}"))?
        );
    } else {
        for entry in &plan.entries {
            println!(
                "{:?}: {} -> {} (source {})",
                entry.action,
                entry.destination,
                entry.target.display(),
                entry.source
            );
        }
        for source in &plan.skipped_sources {
            println!("Skipped missing source: {source}");
        }
        for diagnostic in &plan.diagnostics {
            println!("Blocked: {diagnostic}");
        }
        if args.apply && result.is_ok() {
            println!("Applied symbolic-link plan; no guidance files copied");
        }
    }
    result
}
