//! Guidance profile model and loading.
//!
//! A profile is a TOML document naming a corpus (either direct files or a
//! recipe that produces files) and a destination policy. Profiles are
//! data-driven: there is no global profile database, only the file named on
//! the command line.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawProfile {
    profile: RawProfileMeta,
    #[serde(default, rename = "corpus")]
    corpus: Vec<RawCorpusItem>,
    destination: RawDestination,
}

#[derive(Debug, Deserialize)]
struct RawProfileMeta {
    id: String,
}

#[derive(Debug, Deserialize)]
struct RawCorpusItem {
    id: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    recipe: Option<RawRecipe>,
}

#[derive(Debug, Deserialize)]
struct RawRecipe {
    #[serde(default, rename = "step")]
    steps: Vec<RawRecipeStep>,
}

#[derive(Debug, Deserialize)]
struct RawRecipeStep {
    kind: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDestination {
    scope: String,
    #[serde(default)]
    path: Option<String>,
}

/// A normalized, validated guidance profile.
#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub corpus: Vec<CorpusItem>,
    pub destination: DestinationSpec,
}

#[derive(Debug, Clone)]
pub enum CorpusItem {
    /// A directly selected set of repo-relative source paths.
    Direct { id: String, paths: Vec<String> },
    /// A recipe whose steps produce artifacts from repo-relative inputs.
    Recipe { id: String, steps: Vec<RecipeStep> },
}

impl CorpusItem {
    pub fn id(&self) -> &str {
        match self {
            CorpusItem::Direct { id, .. } => id,
            CorpusItem::Recipe { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RecipeStep {
    /// Copy `from` (repo-relative, within the source root) to the
    /// artifact identified by `to` (repo-relative destination-side id).
    Copy { from: String, to: String },
    /// A recipe step kind this installer does not execute. Recorded as a
    /// blocking diagnostic rather than silently skipped or run.
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct DestinationSpec {
    pub scope: DestinationScopeKind,
    pub explicit_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum DestinationScopeKind {
    Repo,
    User,
    System,
    Explicit,
}

/// Load and normalize a profile file. Returns an actionable error naming the
/// offending field on any malformed input.
pub fn load_profile(profile_path: &Path) -> Result<Profile, String> {
    let text = fs::read_to_string(profile_path)
        .map_err(|e| format!("failed to read profile {}: {e}", profile_path.display()))?;
    let raw: RawProfile = toml::from_str(&text)
        .map_err(|e| format!("malformed profile {}: {e}", profile_path.display()))?;

    if raw.profile.id.trim().is_empty() {
        return Err(format!(
            "profile {} is missing required field profile.id",
            profile_path.display()
        ));
    }

    let mut seen_ids = BTreeSet::new();
    let mut corpus = Vec::with_capacity(raw.corpus.len());
    for item in raw.corpus {
        if item.id.trim().is_empty() {
            return Err(format!(
                "profile {} has a corpus entry with an empty id",
                profile_path.display()
            ));
        }
        if !seen_ids.insert(item.id.clone()) {
            return Err(format!(
                "profile {} declares duplicate corpus id '{}'",
                profile_path.display(),
                item.id
            ));
        }

        match item.recipe {
            None => {
                if item.paths.is_empty() {
                    return Err(format!(
                        "corpus item '{}' has neither paths nor a recipe",
                        item.id
                    ));
                }
                for p in &item.paths {
                    validate_repo_relative(p, &item.id)?;
                }
                corpus.push(CorpusItem::Direct {
                    id: item.id,
                    paths: item.paths,
                });
            }
            Some(recipe) => {
                if !item.paths.is_empty() {
                    return Err(format!(
                        "corpus item '{}' declares both paths and a recipe; only one is allowed",
                        item.id
                    ));
                }
                if recipe.steps.is_empty() {
                    return Err(format!(
                        "corpus item '{}' has a recipe with no steps",
                        item.id
                    ));
                }
                let mut steps = Vec::with_capacity(recipe.steps.len());
                for step in recipe.steps {
                    steps.push(normalize_recipe_step(step, &item.id)?);
                }
                corpus.push(CorpusItem::Recipe { id: item.id, steps });
            }
        }
    }

    let scope = match raw.destination.scope.as_str() {
        "repo" => DestinationScopeKind::Repo,
        "user" => DestinationScopeKind::User,
        "system" => DestinationScopeKind::System,
        "explicit" => DestinationScopeKind::Explicit,
        other => {
            return Err(format!(
                "profile {} has unknown destination.scope '{other}' (expected repo, user, system, or explicit)",
                profile_path.display()
            ));
        }
    };
    let explicit_path = raw.destination.path.map(PathBuf::from);
    if scope == DestinationScopeKind::Explicit && explicit_path.is_none() {
        return Err(format!(
            "profile {} has destination.scope = \"explicit\" but no destination.path",
            profile_path.display()
        ));
    }

    Ok(Profile {
        id: raw.profile.id,
        corpus,
        destination: DestinationSpec {
            scope,
            explicit_path,
        },
    })
}

/// Synthesize a minimal direct-corpus profile from bare repo-relative
/// selection paths, for `guidance get` when no `--profile` is given. Each
/// path becomes its own corpus item (id == path), validated through the
/// same [`validate_repo_relative`] check an authored profile's paths go
/// through — no bypass of that validation for the profile-less path.
/// Destination defaults to the `repo` scope, matching `plan`/`install`'s
/// own default when a profile declares no destination override.
pub fn synthesize_direct_profile(select: &[String]) -> Result<Profile, String> {
    let mut corpus = Vec::with_capacity(select.len());
    for path in select {
        validate_repo_relative(path, path)?;
        corpus.push(CorpusItem::Direct {
            id: path.clone(),
            paths: vec![path.clone()],
        });
    }
    Ok(Profile {
        id: "get".to_string(),
        corpus,
        destination: DestinationSpec {
            scope: DestinationScopeKind::Repo,
            explicit_path: None,
        },
    })
}

fn normalize_recipe_step(step: RawRecipeStep, item_id: &str) -> Result<RecipeStep, String> {
    match step.kind.as_str() {
        "copy" => {
            let from = step
                .from
                .ok_or_else(|| format!("corpus item '{item_id}' has a copy step missing 'from'"))?;
            let to = step
                .to
                .ok_or_else(|| format!("corpus item '{item_id}' has a copy step missing 'to'"))?;
            validate_repo_relative(&from, item_id)?;
            validate_repo_relative(&to, item_id)?;
            Ok(RecipeStep::Copy { from, to })
        }
        other => Ok(RecipeStep::Unsupported(other.to_string())),
    }
}

/// A source path must be relative, use forward slashes, and stay within its
/// root after lexical normalization (no leading `..`, no absolute path).
pub fn validate_repo_relative(raw: &str, subject: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Err(format!("'{subject}' has an empty path"));
    }
    let path = Path::new(raw);
    if path.is_absolute() || raw.starts_with('/') || raw.starts_with('\\') {
        return Err(format!(
            "'{subject}' has an absolute path '{raw}'; source paths must be repo-relative"
        ));
    }
    let mut depth: i64 = 0;
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            Normal(_) => depth += 1,
            CurDir => {}
            ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "'{subject}' has a path '{raw}' that escapes its root via '..'"
                    ));
                }
            }
            Prefix(_) | RootDir => {
                return Err(format!(
                    "'{subject}' has an absolute path '{raw}'; source paths must be repo-relative"
                ));
            }
        }
    }
    Ok(())
}
