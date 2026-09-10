use crate::registry::Artifact;

/// Mirrors install-tools.sh's selection semantics: `all`, a single artifact
/// id, or a whole category name; comma-separated tokens are also accepted.
pub fn resolve_selection(
    artifacts: &[Artifact],
    tokens: &[String],
) -> Result<Vec<Artifact>, String> {
    let mut selected: Vec<Artifact> = Vec::new();

    let push = |artifact: &Artifact, selected: &mut Vec<Artifact>| {
        if !selected.iter().any(|s| s.id == artifact.id) {
            selected.push(artifact.clone());
        }
    };

    for raw in tokens {
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }

            if token == "all" {
                for artifact in artifacts {
                    push(artifact, &mut selected);
                }
                continue;
            }

            if let Some(artifact) = artifacts.iter().find(|a| a.id == token) {
                push(artifact, &mut selected);
                continue;
            }

            let category_matches: Vec<&Artifact> =
                artifacts.iter().filter(|a| a.category == token).collect();
            if !category_matches.is_empty() {
                for artifact in category_matches {
                    push(artifact, &mut selected);
                }
                continue;
            }

            return Err(format!("unknown artifact or category: {token}"));
        }
    }

    Ok(selected)
}
