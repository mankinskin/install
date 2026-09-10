//! Plan construction: combines profile loading, closure computation,
//! destination mapping, and rewrite generation into one serializable,
//! deterministic [`GuidancePlan`].

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::guidance::{
    destination::{
        DestinationPaths, is_initialized_submodule, map_repo_scope_path, resolve_destination_root,
        split_at_agents_root,
    },
    graph::{
        ClosureResult, ContentSource, Diagnostic, DiagnosticCode, compute_closure, extract_links,
        resolve_relative,
    },
    profile::{CorpusItem, DestinationScopeKind, Profile, RecipeStep, load_profile},
    rewrite::rewrite_link,
};

/// Origin of an artifact's bytes: an on-disk source file, or a recipe copy
/// step whose bytes are read from another source-root file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOrigin {
    Direct,
    RecipeCopy { from: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedArtifact {
    pub id: String,
    pub origin: ArtifactOrigin,
    pub final_path: String,
    pub rewrites: Vec<Rewrite>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rewrite {
    pub old_link: String,
    pub new_link: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GuidancePlan {
    pub profile_id: String,
    pub selected_roots: Vec<String>,
    pub destination_scope: String,
    pub destination_root: String,
    pub artifacts: Vec<PlannedArtifact>,
    pub edges: Vec<crate::guidance::graph::DependencyEdge>,
    pub skipped_external: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl GuidancePlan {
    pub fn is_blocking(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

pub struct PlanInputs<'a> {
    pub source_root: &'a Path,
    pub profile_path: &'a Path,
    pub select: &'a [String],
    pub target_root: &'a Path,
    /// Overrides the profile's declared destination scope when set.
    pub scope_override: Option<DestinationScopeKind>,
    /// Overrides the profile's declared explicit destination path when set.
    pub explicit_override: Option<&'a Path>,
    pub destination_paths: &'a DestinationPaths,
}

struct FsContentSource<'a> {
    source_root: &'a Path,
    recipe_from: &'a BTreeMap<String, String>,
}

impl ContentSource for FsContentSource<'_> {
    fn read_text(&self, id: &str) -> Option<String> {
        let read_id = self.recipe_from.get(id).map(String::as_str).unwrap_or(id);
        fs::read_to_string(self.source_root.join(read_id)).ok()
    }

    fn exists(&self, id: &str) -> bool {
        if self.recipe_from.contains_key(id) {
            return true;
        }
        self.source_root.join(id).is_file()
    }

    fn escapes_root(&self, id: &str) -> bool {
        let read_id = self.recipe_from.get(id).map(String::as_str).unwrap_or(id);
        let candidate = self.source_root.join(read_id);
        let (Ok(canonical_root), Ok(canonical_candidate)) = (
            fs::canonicalize(self.source_root),
            fs::canonicalize(&candidate),
        ) else {
            return false;
        };
        !canonical_candidate.starts_with(&canonical_root)
    }
}

/// Build a plan. Read-only: never writes, executes a recipe, or touches the
/// network. Returns `Err` only for a malformed/unloadable profile or an
/// unknown selection id; everything else surfaces as a blocking diagnostic
/// on the returned plan so all issues are visible in one pass.
pub fn build_plan(inputs: &PlanInputs) -> Result<GuidancePlan, String> {
    if !inputs.source_root.is_dir() {
        return Err(format!(
            "source root '{}' is not a directory",
            inputs.source_root.display()
        ));
    }
    let profile = load_profile(inputs.profile_path)?;
    build_plan_with_profile(profile, inputs)
}

/// Build a plan from an already-resolved [`Profile`] value, skipping the
/// `load_profile` TOML read/parse step. `inputs.profile_path` is not read
/// on this path. Used by `guidance get` for its synthesized, profile-less
/// selection; `build_plan` above is the unchanged entry point every other
/// caller (including all pre-existing tests) continues to use.
pub fn build_plan_with_profile(
    profile: Profile,
    inputs: &PlanInputs,
) -> Result<GuidancePlan, String> {
    let by_id: BTreeMap<&str, &CorpusItem> = profile.corpus.iter().map(|c| (c.id(), c)).collect();

    let mut selected_roots: Vec<String> = Vec::new();
    let mut recipe_from: BTreeMap<String, String> = BTreeMap::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut edges: Vec<crate::guidance::graph::DependencyEdge> = Vec::new();

    let mut selection: Vec<&str> = inputs.select.iter().map(String::as_str).collect();
    selection.sort();
    selection.dedup();

    for sel in &selection {
        let Some(item) = by_id.get(sel) else {
            return Err(format!(
                "selection '{sel}' does not match any corpus item in profile '{}'",
                profile.id
            ));
        };
        match item {
            CorpusItem::Direct { paths, .. } => {
                for p in paths {
                    selected_roots.push(p.clone());
                }
            }
            CorpusItem::Recipe { id, steps } => {
                for step in steps {
                    match step {
                        RecipeStep::Copy { from, to } => {
                            if let Some(existing_from) = recipe_from.get(to)
                                && existing_from != from
                            {
                                diagnostics.push(Diagnostic {
                                    code: DiagnosticCode::AmbiguousOwnership,
                                    subject: to.clone(),
                                    message: format!(
                                        "'{to}' is produced from both '{existing_from}' and '{from}'"
                                    ),
                                });
                                continue;
                            }
                            if !to.ends_with(".md") && !to.ends_with(".toml") {
                                diagnostics.push(Diagnostic {
                                    code: DiagnosticCode::UnsafeTarget,
                                    subject: to.clone(),
                                    message: format!(
                                        "corpus item '{id}' produces non-guidance artifact '{to}'"
                                    ),
                                });
                                continue;
                            }
                            recipe_from.insert(to.clone(), from.clone());
                            selected_roots.push(to.clone());
                            edges.push(crate::guidance::graph::DependencyEdge {
                                kind: crate::guidance::graph::EdgeKind::GeneratedInput,
                                source: to.clone(),
                                target: from.clone(),
                                status: crate::guidance::graph::EdgeStatus::Resolved,
                            });
                        }
                        RecipeStep::Unsupported(kind) => {
                            diagnostics.push(Diagnostic {
                                code: DiagnosticCode::UnsupportedRecipeStep,
                                subject: id.clone(),
                                message: format!(
                                    "corpus item '{id}' has an unsupported recipe step kind '{kind}'"
                                ),
                            });
                            edges.push(crate::guidance::graph::DependencyEdge {
                                kind: crate::guidance::graph::EdgeKind::Unsupported,
                                source: id.clone(),
                                target: kind.clone(),
                                status: crate::guidance::graph::EdgeStatus::Blocked,
                            });
                        }
                    }
                }
            }
        }
    }

    for root in &selected_roots {
        if !root.ends_with(".md") && !root.ends_with(".toml") {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnsafeTarget,
                subject: root.clone(),
                message: format!("selection includes non-guidance target '{root}'"),
            });
        }
    }

    let content_source = FsContentSource {
        source_root: inputs.source_root,
        recipe_from: &recipe_from,
    };
    let ClosureResult {
        artifacts: artifact_ids,
        edges: closure_edges,
        diagnostics: closure_diagnostics,
        skipped_external,
    } = compute_closure(&selected_roots, &content_source);
    edges.extend(closure_edges);
    diagnostics.extend(closure_diagnostics);

    let scope = inputs.scope_override.unwrap_or(profile.destination.scope);
    let explicit_destination = inputs
        .explicit_override
        .or(profile.destination.explicit_path.as_deref());

    let destination_root = match resolve_destination_root(
        scope,
        explicit_destination,
        inputs.target_root,
        inputs.destination_paths,
    ) {
        Ok(root) => root,
        Err(diag) => {
            diagnostics.push(diag);
            PathBuf::new()
        }
    };

    // Map every artifact to its final destination-relative path.
    let mut final_paths: BTreeMap<String, String> = BTreeMap::new();
    for id in &artifact_ids {
        let final_path = match scope {
            DestinationScopeKind::Repo => map_repo_scope_path(id, inputs.target_root),
            _ => split_at_agents_root(id).1.to_string(),
        };
        final_paths.insert(id.clone(), final_path);
    }

    // Duplicate-destination diagnostic.
    let mut by_final: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, final_path) in &final_paths {
        by_final
            .entry(final_path.as_str())
            .or_default()
            .push(id.as_str());
    }
    for (final_path, ids) in &by_final {
        if ids.len() > 1 {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::DuplicateDestination,
                subject: final_path.to_string(),
                message: format!("artifacts {ids:?} all map to final destination '{final_path}'"),
            });
        }
    }

    // Compute rewrites per artifact.
    let mut artifacts: Vec<PlannedArtifact> = Vec::new();
    for id in &artifact_ids {
        let final_path = final_paths.get(id).cloned().unwrap_or_default();
        let origin = match recipe_from.get(id) {
            Some(from) => ArtifactOrigin::RecipeCopy { from: from.clone() },
            None => ArtifactOrigin::Direct,
        };

        let mut rewrites = Vec::new();
        if id.ends_with(".md") {
            let read_id = recipe_from.get(id).cloned().unwrap_or_else(|| id.clone());
            if let Some(text) = fs::read_to_string(inputs.source_root.join(&read_id)).ok() {
                let base_dir = id.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let mut links = extract_links(&text);
                links.sort();
                links.dedup();
                for raw in links {
                    let Some(target_id) = resolve_relative(base_dir, &raw) else {
                        continue;
                    };
                    let Some(target_final) = final_paths.get(&target_id) else {
                        continue;
                    };
                    if let Some(new_link) = rewrite_link(&raw, &final_path, target_final) {
                        rewrites.push(Rewrite {
                            old_link: raw,
                            new_link,
                        });
                    }
                }
            }
        }

        artifacts.push(PlannedArtifact {
            id: id.clone(),
            origin,
            final_path,
            rewrites,
        });
    }

    // Submodule-ownership edges are informational, attached per artifact.
    for a in &artifacts {
        let (owning_dir, _) = split_at_agents_root(&a.id);
        if let Some(owning_dir) = owning_dir
            && matches!(scope, DestinationScopeKind::Repo)
            && is_initialized_submodule(inputs.target_root, owning_dir)
        {
            edges.push(crate::guidance::graph::DependencyEdge {
                kind: crate::guidance::graph::EdgeKind::SubmoduleOwnership,
                source: a.id.clone(),
                target: owning_dir.to_string(),
                status: crate::guidance::graph::EdgeStatus::Resolved,
            });
        }
    }

    edges.sort();
    edges.dedup();
    diagnostics.sort();
    diagnostics.dedup();

    let scope_label = match scope {
        DestinationScopeKind::Repo => "repo",
        DestinationScopeKind::User => "user",
        DestinationScopeKind::System => "system",
        DestinationScopeKind::Explicit => "explicit",
    };

    Ok(GuidancePlan {
        profile_id: profile.id,
        selected_roots,
        destination_scope: scope_label.to_string(),
        destination_root: crate::paths::disp(&destination_root),
        artifacts,
        edges,
        skipped_external: skipped_external.into_iter().collect(),
        diagnostics,
    })
}

pub fn render_text(plan: &GuidancePlan) -> String {
    let mut out = String::new();
    out.push_str(&format!("profile: {}\n", plan.profile_id));
    out.push_str(&format!("destination scope: {}\n", plan.destination_scope));
    out.push_str(&format!("destination root: {}\n", plan.destination_root));
    out.push_str(&format!("artifacts ({}):\n", plan.artifacts.len()));
    for a in &plan.artifacts {
        out.push_str(&format!("  {} -> {}\n", a.id, a.final_path));
        for r in &a.rewrites {
            out.push_str(&format!("    rewrite: {} -> {}\n", r.old_link, r.new_link));
        }
    }
    if !plan.skipped_external.is_empty() {
        out.push_str("skipped external references:\n");
        for s in &plan.skipped_external {
            out.push_str(&format!("  {s}\n"));
        }
    }
    if !plan.diagnostics.is_empty() {
        out.push_str("diagnostics (blocking):\n");
        for d in &plan.diagnostics {
            out.push_str(&format!("  [{:?}] {}\n", d.code, d.message));
        }
    }
    out
}
