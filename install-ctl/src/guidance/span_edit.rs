//! Exact-span Markdown link destination replacement.

use std::{collections::BTreeSet, ops::Range};

use markdown_links::extract;

/// Replaces link destinations in `text` by exact source span. `edits` are
/// `(old_destination, new_destination)` pairs; `old_destination` is matched
/// against the parsed destination of each link, and only a link whose
/// destination is located verbatim in the source (a `Some` span) is edited.
/// Code fences, code spans and any other text are never touched.
///
/// Returns the new text and the old values that had no located span.
pub fn replace_destinations(text: &str, edits: &[(String, String)]) -> (String, Vec<String>) {
    let links = extract(text);
    let mut spans: Vec<(Range<usize>, &str)> = Vec::new();
    let mut unlocated = Vec::new();
    for (old, new) in edits {
        let mut located = false;
        for link in links.iter().filter(|link| &link.destination == old) {
            if let Some(span) = &link.span {
                located = true;
                spans.push((span.clone(), new.as_str()));
            }
        }
        if !located && !unlocated.contains(old) {
            unlocated.push(old.clone());
        }
    }

    let mut seen = BTreeSet::new();
    spans.retain(|(span, _)| seen.insert((span.start, span.end)));
    spans.sort_by(|a, b| b.0.start.cmp(&a.0.start));

    let mut out = text.to_string();
    for (span, new) in spans {
        out.replace_range(span, new);
    }
    (out, unlocated)
}

#[cfg(test)]
mod tests {
    use super::replace_destinations;

    fn edit(old: &str, new: &str) -> Vec<(String, String)> {
        vec![(old.to_string(), new.to_string())]
    }

    #[test]
    fn replaces_only_located_spans_back_to_front() {
        let text = "[a](x.md) and [b](x.md#f)\n```\n[c](x.md)\n```\n`[d](x.md)`\n";
        let (out, unlocated) = replace_destinations(text, &edit("x.md", "y.md"));
        assert_eq!(
            out,
            "[a](y.md) and [b](x.md#f)\n```\n[c](x.md)\n```\n`[d](x.md)`\n"
        );
        assert!(unlocated.is_empty());
    }

    #[test]
    fn reports_values_without_a_located_span() {
        let (out, unlocated) = replace_destinations("[a](b.md)", &edit("missing.md", "z.md"));
        assert_eq!(out, "[a](b.md)");
        assert_eq!(unlocated, vec!["missing.md".to_string()]);
    }
}
