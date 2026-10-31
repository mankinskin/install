use pulldown_cmark::{Event, Parser, Tag};

/// Extract Markdown link and image targets in source order, stripping titles.
///
/// Uses CommonMark parsing so that example links inside code spans and fenced
/// code blocks are not treated as dependencies.
pub fn extract_links(text: &str) -> Vec<String> {
    Parser::new(text)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. })
                if !dest_url.is_empty() =>
            {
                Some(dest_url.into_string())
            }
            _ => None,
        })
        .collect()
}

pub enum LinkClass {
    External,
    Guidance,
    Profile,
    NonGuidance(String),
    AbsoluteSource(String),
}

pub fn classify_link(raw: &str) -> LinkClass {
    if raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("mailto:")
        || raw.starts_with('#')
    {
        return LinkClass::External;
    }
    let path_part = raw.split('#').next().unwrap_or(raw);
    if path_part.is_empty() {
        return LinkClass::External;
    }
    if path_part.starts_with('/')
        || path_part.starts_with('\\')
        || (path_part.len() > 1 && path_part.as_bytes()[1] == b':')
    {
        return LinkClass::AbsoluteSource(path_part.to_string());
    }
    if path_part.ends_with(".md") {
        LinkClass::Guidance
    } else if path_part.ends_with(".toml") {
        LinkClass::Profile
    } else {
        LinkClass::NonGuidance(path_part.to_string())
    }
}

/// Resolve a link against a source-root-relative directory without escaping.
pub fn resolve_relative(base_dir: &str, raw: &str) -> Option<String> {
    let path_part = raw.split('#').next().unwrap_or(raw);
    let mut stack: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path_part.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}
