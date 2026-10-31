//! Relative Markdown link rewriting: given a link's source-relative target
//! and the final destination-relative paths of both the referencing and
//! referenced artifacts, compute the correct relative link for the new
//! location (or `None` if the relative shape is unchanged).

fn split_components(rel: &str) -> Vec<&str> {
    rel.split('/').filter(|s| !s.is_empty()).collect()
}

/// `from_final` and `target_final` are both destination-root-relative paths
/// (forward slashes) for the referencing file and the referenced file.
/// Returns the corrected relative link, or `None` when the relative path is
/// unchanged by the move (no rewrite needed).
pub fn rewrite_link(old_link: &str, from_final: &str, target_final: &str) -> Option<String> {
    let from_parts = split_components(from_final);
    let to_parts = split_components(target_final);
    let from_dir = &from_parts[..from_parts.len().saturating_sub(1)];

    let common = from_dir
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from_dir.len() - common;

    let mut new_parts: Vec<String> = std::iter::repeat_n("..".to_string(), ups).collect();
    new_parts.extend(to_parts[common..].iter().map(|s| s.to_string()));
    if new_parts.is_empty() {
        return None;
    }
    let new_path = new_parts.join("/");

    let old_path_part = old_link.split('#').next().unwrap_or(old_link);
    let fragment = &old_link[old_path_part.len()..];
    // Compare decoded paths so an encoded but otherwise identical link is
    // left alone; emit the new path percent-encoded with the fragment as-is.
    if markdown_links::decode_percent(old_path_part) == new_path {
        None
    } else {
        Some(format!(
            "{}{fragment}",
            markdown_links::encode_destination(&new_path)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_relative_shape_needs_no_rewrite() {
        // Both files move together, keeping the same relative nesting.
        assert_eq!(
            rewrite_link(
                "../instructions/base.md",
                "a/prompt.md",
                "instructions/base.md"
            ),
            None
        );
    }

    #[test]
    fn relocation_produces_a_new_relative_link() {
        // Source: a/prompt.md -> ../instructions/base.md
        // Final: .agents/a/prompt.md and .agents/instructions/base.md
        // (same relative shape once both share `.agents/` root)
        let rewritten = rewrite_link("../base.md", "a/b/prompt.md", "other/base.md");
        assert_eq!(rewritten, Some("../../other/base.md".to_string()));
    }

    #[test]
    fn encodes_new_path_and_keeps_fragment() {
        let rewritten = rewrite_link("../base.md#sec", "a/b/prompt.md", "other/my base.md");
        assert_eq!(
            rewritten,
            Some("../../other/my%20base.md#sec".to_string())
        );
    }

    #[test]
    fn encoded_identical_link_needs_no_rewrite() {
        assert_eq!(
            rewrite_link("x%20y.md#f", "a/p.md", "a/x y.md"),
            None
        );
    }

    #[test]
    fn preserves_fragment() {
        let rewritten = rewrite_link("../base.md#section", "a/b/prompt.md", "other/base.md");
        assert_eq!(rewritten, Some("../../other/base.md#section".to_string()));
    }
}
