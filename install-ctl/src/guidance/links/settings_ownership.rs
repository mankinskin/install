use super::{
    super::vscode,
    document::{Document, KINDS},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ownership {
    pub version: u8,
    pub settings_was_absent: bool,
    pub created_settings: BTreeSet<String>,
    pub sources: BTreeMap<String, Source>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub path: String,
    pub links: BTreeMap<String, OwnedLink>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnedLink {
    pub destination: String,
    pub target: PathBuf,
    pub inserted: bool,
}

pub(super) fn read(bytes: Option<&[u8]>, settings_was_absent: bool) -> Result<Ownership, String> {
    if let Some(bytes) = bytes {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("invalid ownership UTF-8: {error}"))?;
        let root =
            jsonc_parser::cst::CstRootNode::parse(text, &jsonc_parser::ParseOptions::default())
                .map_err(|error| format!("invalid guidance ownership: {error}"))?;
        check_unique(root.children())?;
    }
    let record: Ownership = match bytes {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid guidance ownership record: {error}"))?,
        None => Ownership {
            version: 1,
            settings_was_absent,
            created_settings: BTreeSet::new(),
            sources: BTreeMap::new(),
        },
    };
    if record.version != 1
        || record
            .created_settings
            .iter()
            .any(|kind| !KINDS.contains(&kind.as_str()))
    {
        return Err("invalid guidance ownership schema or kind".to_owned());
    }

    fn check_unique(nodes: Vec<jsonc_parser::cst::CstNode>) -> Result<(), String> {
        for node in nodes {
            if let Some(object) = node.as_object() {
                let mut names = BTreeSet::new();
                for property in object.properties() {
                    let name = property
                        .decoded_name()
                        .ok_or("invalid ownership property")?;
                    if !names.insert(name) {
                        return Err("duplicate ownership property".to_owned());
                    }
                }
            }
            check_unique(node.children())?;
        }
        Ok(())
    }
    for (name, source) in &record.sources {
        // Reuse the lexical source contract without touching source directories.
        vscode::parse_sources(&[format!("{name}={}", source.path)])?;
        if source.links.is_empty() {
            return Err(format!("ownership source '{name}' has no links"));
        }
        for (kind, link) in &source.links {
            if !KINDS.contains(&kind.as_str())
                || link.destination != format!(".github/{kind}/{name}")
                || link.target
                    != PathBuf::from("..")
                        .join("..")
                        .join(vscode::native_path(&source.path))
                        .join(kind)
            {
                return Err(format!("invalid owned mapping '{name}:{kind}'"));
            }
        }
    }
    Ok(record)
}

impl Ownership {
    pub fn validate_current(&self, doc: &Document) -> Result<(), String> {
        for source in self.sources.values() {
            for (kind, link) in &source.links {
                if link.inserted {
                    doc.require_true(kind, &link.destination)?;
                }
            }
        }
        Ok(())
    }
}
