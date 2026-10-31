//! Link rewrite computation for one planned Markdown artifact.

use std::collections::BTreeMap;

use markdown_links::{DestinationShape, classify_destination, extract, resolve_local};

use super::Rewrite;
use crate::guidance::rewrite::rewrite_link;

/// Computes the link rewrites for the artifact `id` whose Markdown `text`
/// moves to `final_path`. `final_paths` maps every planned artifact id to its
/// final destination path. Destinations are deduplicated; `old_link` keeps the
/// raw destination as written.
pub(super) fn compute_rewrites(
    id: &str,
    text: &str,
    final_path: &str,
    final_paths: &BTreeMap<String, String>,
) -> Vec<Rewrite> {
    let base_dir = id.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut destinations: Vec<String> = extract(text)
        .into_iter()
        .map(|link| link.destination)
        .collect();
    destinations.sort();
    destinations.dedup();

    let mut rewrites = Vec::new();
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
        if let Some(new_link) = rewrite_link(&raw, final_path, target_final) {
            rewrites.push(Rewrite {
                old_link: raw,
                new_link,
            });
        }
    }
    rewrites
}
