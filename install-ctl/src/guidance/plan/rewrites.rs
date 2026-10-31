//! Link rewrite computation for one planned Markdown artifact.

use std::collections::BTreeMap;

use markdown_links::{DestinationShape, classify_destination, extract, resolve_local};

use super::Rewrite;
use crate::guidance::{
    graph::{Diagnostic, DiagnosticCode},
    rewrite::rewrite_link,
};

/// Computes the link rewrites for the artifact `id` whose Markdown `text`
/// moves to `final_path`. `final_paths` maps every planned artifact id to its
/// final destination path. Destinations are deduplicated; `old_link` keeps the
/// raw destination as written. A destination with an occurrence that is not
/// located verbatim in the source yields a non-blocking `UnlocatableRewrite`
/// diagnostic; the rewrite is kept when another occurrence is located and
/// dropped when none is.
pub(super) fn compute_rewrites(
    id: &str,
    text: &str,
    final_path: &str,
    final_paths: &BTreeMap<String, String>,
) -> (Vec<Rewrite>, Vec<Diagnostic>) {
    let base_dir = id.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let links = extract(text);
    let mut destinations: Vec<String> = links.iter().map(|l| l.destination.clone()).collect();
    destinations.sort();
    destinations.dedup();

    let mut rewrites = Vec::new();
    let mut diagnostics = Vec::new();
    for raw in destinations {
        let DestinationShape::Local(path) = classify_destination(&raw) else {
            continue;
        };
        let Some(target_id) = resolve_local(base_dir, &path) else {
            continue;
        };
        let Some(target_final) = final_paths.get(&target_id) else {
            continue;
        };
        let Some(new_link) = rewrite_link(&raw, final_path, target_final) else {
            continue;
        };
        let occurrences = links.iter().filter(|l| l.destination == raw);
        let (located, unlocated) =
            occurrences.fold((0usize, 0usize), |(located, unlocated), link| {
                if link.span.is_some() {
                    (located + 1, unlocated)
                } else {
                    (located, unlocated + 1)
                }
            });
        if unlocated > 0 {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnlocatableRewrite,
                subject: id.to_string(),
                message: format!(
                    "'{id}': link destination '{raw}' is not found verbatim in the source; {unlocated} occurrence(s) left unchanged"
                ),
            });
        }
        if located > 0 {
            rewrites.push(Rewrite {
                old_link: raw,
                new_link,
            });
        }
    }
    (rewrites, diagnostics)
}
