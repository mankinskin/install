//! Dependency edge classification, Markdown link extraction, and transitive
//! closure computation over a guidance corpus.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

#[path = "graph/links.rs"]
mod links;
pub use links::{LinkClass, classify_link, extract_links, resolve_relative};

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    GuidanceReference,
    ProfileReference,
    GeneratedInput,
    SubmoduleOwnership,
    Unsupported,
    Unsafe,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeStatus {
    Resolved,
    Missing,
    Blocked,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DependencyEdge {
    pub kind: EdgeKind,
    pub source: String,
    pub target: String,
    pub status: EdgeStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    MissingDependency,
    Cycle,
    DuplicateDestination,
    AmbiguousOwnership,
    PathTraversal,
    AbsoluteSourcePath,
    SymlinkEscape,
    UnsupportedRecipeStep,
    UnsafeTarget,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub subject: String,
    pub message: String,
}

/// A node the closure can visit: either an existing on-disk source file or a
/// recipe-produced artifact whose bytes are read from an input file.
pub trait ContentSource {
    /// Read UTF-8 text content for `id` if the node both exists and is
    /// readable as text (non-`.md`/`.toml` nodes are never queried).
    fn read_text(&self, id: &str) -> Option<String>;
    /// Whether `id` exists at all (used for missing-dependency detection).
    fn exists(&self, id: &str) -> bool;
    /// Whether `id` resolves (e.g. via a symlink) to a path outside the
    /// source root. Checked before `exists` so an escaping symlink is
    /// reported distinctly rather than silently followed.
    fn escapes_root(&self, id: &str) -> bool;
}

pub struct ClosureResult {
    pub artifacts: BTreeSet<String>,
    pub edges: Vec<DependencyEdge>,
    pub diagnostics: Vec<Diagnostic>,
    pub skipped_external: BTreeSet<String>,
}

/// Missing content is distinct from a failed read or acquisition.
pub trait FallibleContentSource {
    type Error;
    fn read_text(&mut self, id: &str) -> Result<Option<String>, Self::Error>;
    fn exists(&mut self, id: &str) -> Result<bool, Self::Error>;
    fn escapes_root(&mut self, id: &str) -> Result<bool, Self::Error>;
}

struct LocalSource<'a>(&'a dyn ContentSource);

impl FallibleContentSource for LocalSource<'_> {
    type Error = std::convert::Infallible;

    fn read_text(&mut self, id: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.0.read_text(id))
    }

    fn exists(&mut self, id: &str) -> Result<bool, Self::Error> {
        Ok(self.0.exists(id))
    }

    fn escapes_root(&mut self, id: &str) -> Result<bool, Self::Error> {
        Ok(self.0.escapes_root(id))
    }
}

/// Compute the deterministic transitive closure over `roots` by following
/// `.md` and `.toml` links discovered in each visited node's text.
pub fn compute_closure(roots: &[String], source: &dyn ContentSource) -> ClosureResult {
    match compute_closure_fallible(roots, &mut LocalSource(source)) {
        Ok(closure) => closure,
        Err(impossible) => match impossible {},
    }
}

pub fn compute_closure_fallible<S: FallibleContentSource + ?Sized>(
    roots: &[String],
    source: &mut S,
) -> Result<ClosureResult, S::Error> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        InProgress,
        Done,
    }

    let mut state: BTreeMap<String, State> = BTreeMap::new();
    let mut edges: Vec<DependencyEdge> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut skipped_external: BTreeSet<String> = BTreeSet::new();
    let mut artifacts: BTreeSet<String> = BTreeSet::new();

    fn visit<S: FallibleContentSource + ?Sized>(
        id: &str,
        source: &mut S,
        state: &mut BTreeMap<String, State>,
        edges: &mut Vec<DependencyEdge>,
        diagnostics: &mut Vec<Diagnostic>,
        skipped_external: &mut BTreeSet<String>,
        artifacts: &mut BTreeSet<String>,
    ) -> Result<(), S::Error> {
        state.insert(id.to_string(), State::InProgress);
        artifacts.insert(id.to_string());

        let base_dir = id.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        let text = if id.ends_with(".md") {
            source.read_text(id)?
        } else {
            None
        };

        let mut links: Vec<String> = text.as_deref().map(extract_links).unwrap_or_default();
        links.sort();
        links.dedup();

        for raw in links {
            match classify_link(&raw) {
                LinkClass::External => {
                    skipped_external.insert(raw);
                }
                LinkClass::NonGuidance(resolved) => {
                    skipped_external.insert(resolved);
                }
                LinkClass::AbsoluteSource(target) => {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::AbsoluteSourcePath,
                        subject: id.to_string(),
                        message: format!("'{id}' links to absolute path '{target}'"),
                    });
                    edges.push(DependencyEdge {
                        kind: EdgeKind::Unsafe,
                        source: id.to_string(),
                        target,
                        status: EdgeStatus::Blocked,
                    });
                }
                LinkClass::Guidance | LinkClass::Profile => {
                    let resolved = resolve_relative(base_dir, &raw);
                    let Some(target) = resolved else {
                        diagnostics.push(Diagnostic {
                            code: DiagnosticCode::PathTraversal,
                            subject: id.to_string(),
                            message: format!(
                                "'{id}' links to '{raw}', which escapes the source root"
                            ),
                        });
                        continue;
                    };
                    let kind = if target.ends_with(".toml") {
                        EdgeKind::ProfileReference
                    } else {
                        EdgeKind::GuidanceReference
                    };

                    match state.get(&target).copied() {
                        Some(State::InProgress) => {
                            diagnostics.push(Diagnostic {
                                code: DiagnosticCode::Cycle,
                                subject: target.clone(),
                                message: format!(
                                    "dependency cycle: '{id}' -> '{target}' closes a cycle"
                                ),
                            });
                            edges.push(DependencyEdge {
                                kind,
                                source: id.to_string(),
                                target,
                                status: EdgeStatus::Blocked,
                            });
                        }
                        Some(State::Done) => {
                            edges.push(DependencyEdge {
                                kind,
                                source: id.to_string(),
                                target,
                                status: EdgeStatus::Resolved,
                            });
                        }
                        None => {
                            if source.escapes_root(&target)? {
                                diagnostics.push(Diagnostic {
                                    code: DiagnosticCode::SymlinkEscape,
                                    subject: target.clone(),
                                    message: format!(
                                        "'{id}' references '{target}', which resolves outside the source root"
                                    ),
                                });
                                edges.push(DependencyEdge {
                                    kind,
                                    source: id.to_string(),
                                    target,
                                    status: EdgeStatus::Blocked,
                                });
                                continue;
                            }
                            if !source.exists(&target)? {
                                diagnostics.push(Diagnostic {
                                    code: DiagnosticCode::MissingDependency,
                                    subject: target.clone(),
                                    message: format!(
                                        "'{id}' references '{target}', which does not exist"
                                    ),
                                });
                                edges.push(DependencyEdge {
                                    kind,
                                    source: id.to_string(),
                                    target,
                                    status: EdgeStatus::Missing,
                                });
                                continue;
                            }
                            edges.push(DependencyEdge {
                                kind,
                                source: id.to_string(),
                                target: target.clone(),
                                status: EdgeStatus::Resolved,
                            });
                            visit(
                                &target,
                                source,
                                state,
                                edges,
                                diagnostics,
                                skipped_external,
                                artifacts,
                            )?;
                        }
                    }
                }
            }
        }

        state.insert(id.to_string(), State::Done);
        Ok(())
    }

    let mut sorted_roots: Vec<String> = roots.to_vec();
    sorted_roots.sort();
    sorted_roots.dedup();
    for root in &sorted_roots {
        if state.contains_key(root) {
            continue;
        }
        if source.escapes_root(root)? {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::SymlinkEscape,
                subject: root.clone(),
                message: format!("selected root '{root}' resolves outside the source root"),
            });
            continue;
        }
        if !source.exists(root)? {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::MissingDependency,
                subject: root.clone(),
                message: format!("selected root '{root}' does not exist"),
            });
            continue;
        }
        visit(
            root,
            source,
            &mut state,
            &mut edges,
            &mut diagnostics,
            &mut skipped_external,
            &mut artifacts,
        )?;
    }

    edges.sort();
    edges.dedup();
    diagnostics.sort();
    diagnostics.dedup();

    Ok(ClosureResult {
        artifacts,
        edges,
        diagnostics,
        skipped_external,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_links_finds_inline_targets() {
        let text = "See [base](../instructions/base.md) and [ext](https://example.com).";
        let links = extract_links(text);
        assert_eq!(
            links,
            vec!["../instructions/base.md", "https://example.com"]
        );
    }

    #[test]
    fn extract_links_strips_titles() {
        let text = r#"[a](foo.md "title text")"#;
        assert_eq!(extract_links(text), vec!["foo.md"]);
    }

    #[test]
    fn classify_link_detects_external_and_unsafe() {
        assert!(matches!(
            classify_link("https://example.com"),
            LinkClass::External
        ));
        assert!(matches!(classify_link("#anchor"), LinkClass::External));
        assert!(matches!(
            classify_link("/etc/passwd"),
            LinkClass::AbsoluteSource(_)
        ));
        assert!(matches!(
            classify_link("../../etc/notes.md"),
            LinkClass::Guidance
        ));
    }

    #[test]
    fn resolve_relative_rejects_escape_past_root() {
        assert_eq!(resolve_relative("a", "../b.md"), Some("b.md".to_string()));
        assert_eq!(resolve_relative("", "../b.md"), None);
    }
}
