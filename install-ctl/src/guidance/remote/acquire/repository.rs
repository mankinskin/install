use super::super::{cache::CachedRepository, transport::ReceivedObject};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(super) struct Entry {
    pub oid: String,
    pub mode: u16,
}

impl Entry {
    pub fn is_directory(&self) -> bool {
        self.mode == 0o40000
    }
    pub fn is_gitlink(&self) -> bool {
        self.mode == 0o160000
    }
    pub fn is_symlink(&self) -> bool {
        self.mode == 0o120000
    }
}

pub(super) struct Repository {
    pub source: String,
    pub cache: CachedRepository,
    pub entries: BTreeMap<Vec<u8>, Entry>,
    pub modules: Option<BTreeMap<String, String>>,
}

impl Repository {
    pub fn new(source: &str, mut cache: CachedRepository) -> Result<Self, String> {
        let metadata = cache.metadata()?;
        let objects: BTreeMap<_, _> = metadata
            .iter()
            .map(|object| (object.oid.as_str(), object))
            .collect();
        let hash = gix_hash::ObjectId::from_hex(cache.head_oid().as_bytes())
            .map_err(|_| "guidance acquisition: invalid commit ID")?
            .kind();
        let object = objects
            .get(cache.head_oid())
            .ok_or("guidance acquisition: missing commit")?;
        let commit = gix_object::CommitRef::from_bytes(&object.data, hash)
            .map_err(|_| "guidance acquisition: invalid commit metadata")?;
        let tree = commit.tree().to_string();
        let mut entries = BTreeMap::new();
        entries.insert(
            Vec::new(),
            Entry {
                oid: tree.clone(),
                mode: 0o40000,
            },
        );
        flatten(&tree, &[], &objects, &mut entries, hash, 0)?;
        Ok(Self {
            source: source.to_owned(),
            cache,
            entries,
            modules: None,
        })
    }
}

fn flatten(
    oid: &str,
    prefix: &[u8],
    objects: &BTreeMap<&str, &ReceivedObject>,
    entries: &mut BTreeMap<Vec<u8>, Entry>,
    hash: gix_hash::Kind,
    depth: usize,
) -> Result<(), String> {
    if depth > 256 {
        return Err("guidance acquisition: Git tree nesting exceeds 256 levels".into());
    }
    let object = objects
        .get(oid)
        .ok_or("guidance acquisition: incomplete tree metadata")?;
    let tree = gix_object::TreeRef::from_bytes(&object.data, hash)
        .map_err(|_| "guidance acquisition: malformed tree metadata")?;
    for item in tree.entries {
        let mut path = prefix.to_vec();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(item.filename);
        entries.insert(
            path.clone(),
            Entry {
                oid: item.oid.to_string(),
                mode: item.mode.value(),
            },
        );
        if item.mode.is_tree() {
            flatten(
                &item.oid.to_string(),
                &path,
                objects,
                entries,
                hash,
                depth + 1,
            )?;
        }
    }
    Ok(())
}
