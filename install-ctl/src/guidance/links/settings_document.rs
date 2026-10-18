use jsonc_parser::{
    ParseOptions,
    cst::{CstInputValue, CstLeafNode, CstNode, CstObject, CstRootNode},
};
use std::collections::BTreeSet;

pub(super) const KINDS: [&str; 3] = ["agents", "instructions", "prompts"];

pub(super) fn setting(kind: &str) -> &'static str {
    match kind {
        "agents" => "chat.agentFilesLocations",
        "instructions" => "chat.instructionsFilesLocations",
        "prompts" => "chat.promptFilesLocations",
        _ => unreachable!("validated guidance kind"),
    }
}

pub(super) struct Document {
    root: CstRootNode,
    object: CstObject,
}

impl Document {
    pub fn parse(bytes: Option<&[u8]>) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes.unwrap_or(b"{}\n"))
            .map_err(|error| format!("settings.json is not UTF-8: {error}"))?;
        let options = ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: false,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
            allow_bare_decimal_point_numbers: false,
            allow_non_finite_numbers: false,
            allow_extended_string_escapes: false,
        };
        let root = CstRootNode::parse(text, &options)
            .map_err(|error| format!("invalid settings JSONC: {error}"))?;
        let object = root
            .object_value()
            .ok_or("settings document must be an object")?;
        for kind in KINDS {
            let key = setting(kind);
            reject_duplicates(&object, Some(key))?;
            if let Some(prop) = object.get(key) {
                let locations = prop
                    .object_value()
                    .ok_or_else(|| format!("setting '{key}' must be an object"))?;
                reject_duplicates(&locations, None)?;
            }
        }
        Ok(Self { root, object })
    }

    pub fn require_true(&self, kind: &str, destination: &str) -> Result<(), String> {
        let prop = self
            .object
            .object_value(setting(kind))
            .and_then(|object| object.get(destination));
        if !prop
            .and_then(|p| p.value())
            .is_some_and(|value| value.to_string() == "true")
        {
            return Err(format!(
                "owned setting '{destination}' was changed or removed"
            ));
        }
        Ok(())
    }

    pub fn insert(
        &self,
        kind: &str,
        destination: &str,
        created: &mut BTreeSet<String>,
    ) -> Result<bool, String> {
        let key = setting(kind);
        let locations = match self.object.object_value(key) {
            Some(object) => object,
            None => {
                self.object.append(key, CstInputValue::Object(Vec::new()));
                created.insert(kind.to_owned());
                self.object.object_value(key).expect("inserted object")
            }
        };
        match locations.get(destination) {
            Some(prop)
                if prop
                    .value()
                    .is_some_and(|value| value.to_string() == "true") =>
            {
                Ok(false)
            }
            Some(_) => Err(format!("setting '{destination}' must be true")),
            None => {
                locations.append(destination, CstInputValue::Bool(true));
                Ok(true)
            }
        }
    }

    pub fn remove(&self, kind: &str, destination: &str) -> Result<(), String> {
        self.require_true(kind, destination)?;
        self.object
            .object_value(setting(kind))
            .expect("validated object")
            .get(destination)
            .expect("validated property")
            .remove();
        Ok(())
    }

    pub fn remove_empty_created_settings(&self, created: &BTreeSet<String>) {
        for kind in created {
            let key = setting(kind);
            if self.object.object_value(key).is_some_and(|object| {
                object.properties().is_empty() && !has_comment(object.children())
            }) {
                self.object.get(key).expect("existing property").remove();
            }
        }
    }

    pub fn is_empty_without_comments(&self) -> bool {
        self.object.properties().is_empty() && !has_comment(self.root.children())
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.root.to_string().into_bytes()
    }
}

fn has_comment(nodes: Vec<CstNode>) -> bool {
    nodes.iter().any(|node| {
        matches!(node, CstNode::Leaf(CstLeafNode::Comment(_))) || has_comment(node.children())
    })
}

fn reject_duplicates(object: &CstObject, only: Option<&str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for prop in object.properties() {
        let name = prop
            .decoded_name()
            .ok_or("invalid settings property name")?;
        if only.is_none_or(|only| only == name) && !seen.insert(name.clone()) {
            return Err(format!("duplicate relevant setting '{name}'"));
        }
    }
    Ok(())
}
