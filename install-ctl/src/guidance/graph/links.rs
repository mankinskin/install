/// Extract inline Markdown targets in source order, stripping link titles.
pub fn extract_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'['
            && let Some(close_bracket_rel) = text[i..].find(']')
        {
            let after_label = i + close_bracket_rel + 1;
            if text[after_label..].starts_with('(') {
                let paren_start = after_label + 1;
                if let Some(close_paren_rel) = text[paren_start..].find(')') {
                    let raw_target = &text[paren_start..paren_start + close_paren_rel];
                    let target = raw_target.split_whitespace().next().unwrap_or("");
                    if !target.is_empty() {
                        links.push(target.to_string());
                    }
                    i = paren_start + close_paren_rel + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    links
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
