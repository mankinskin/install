use std::collections::BTreeMap;

use crate::guidance::graph::{
    ClosureResult, ContentSource, DiagnosticCode, EdgeKind, EdgeStatus, compute_closure,
};

struct MemorySource(BTreeMap<String, String>);

impl MemorySource {
    fn new(files: &[(&str, &str)]) -> Self {
        Self(
            files
                .iter()
                .map(|(id, text)| (id.to_string(), text.to_string()))
                .collect(),
        )
    }
}

impl ContentSource for MemorySource {
    fn read_text(&self, id: &str) -> Option<String> {
        self.0.get(id).cloned()
    }

    fn exists(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    fn escapes_root(&self, _id: &str) -> bool {
        false
    }
}

fn closure(files: &[(&str, &str)]) -> ClosureResult {
    compute_closure(&["root.md".to_string()], &MemorySource::new(files))
}

fn blocking_codes(result: &ClosureResult) -> Vec<&DiagnosticCode> {
    result.diagnostics.iter().map(|d| &d.code).collect()
}

#[test]
fn closure_ignores_links_in_code_fences_and_spans() {
    let result = closure(&[
        (
            "root.md",
            "Real [a](a.md).\n\n```markdown\n| **Part** | [x](01-example.md) |\n```\n\nInline `[y](y.md)`.\n",
        ),
        ("a.md", "leaf"),
    ]);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result
            .artifacts
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["a.md", "root.md"]
    );
}

#[test]
fn closure_resolves_percent_encoded_relative_link() {
    let result = closure(&[("root.md", "See [spaced](a%20b.md)."), ("a b.md", "leaf")]);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(result.artifacts.contains("a b.md"));
    assert!(result.edges.iter().any(|e| e.target == "a b.md"
        && e.kind == EdgeKind::GuidanceReference
        && e.status == EdgeStatus::Resolved));
}

#[test]
fn closure_rejects_backslash_rooted_link_as_absolute_source_path() {
    let result = closure(&[("root.md", "See [x](\\x.md).")]);
    assert_eq!(
        blocking_codes(&result),
        vec![&DiagnosticCode::AbsoluteSourcePath]
    );
    assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Unsafe
        && e.target == "\\x.md"
        && e.status == EdgeStatus::Blocked));
}

#[test]
fn closure_skips_unsupported_scheme_without_blocking_diagnostic() {
    let result = closure(&[("root.md", "See [x](ftp://x).")]);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert!(result.edges.is_empty());
    assert_eq!(result.artifacts.len(), 1);
}
