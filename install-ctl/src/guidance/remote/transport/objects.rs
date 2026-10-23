use super::ReceivedObject;
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn verifier(directory: &std::path::Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(directory);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_INDEX_FILE",
        "GIT_SHALLOW_FILE",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(name);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", directory.join("absent.config"))
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

pub(super) fn decode(pack: &[u8], format: &str) -> Result<Vec<ReceivedObject>, String> {
    let directory =
        tempfile::tempdir().map_err(|_| "selective transport: cannot allocate pack quarantine")?;
    let init = verifier(directory.path())
        .args(["init", "--bare", "--quiet", "--template="])
        .arg(format!("--object-format={format}"))
        .arg(directory.path())
        .output()
        .map_err(|_| "selective transport: cannot start Git object verifier")?;
    if !init.status.success() {
        return Err("selective transport: cannot initialize pack quarantine".into());
    }
    let mut index = verifier(directory.path())
        .arg("--git-dir")
        .arg(directory.path())
        .args(["index-pack", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "selective transport: cannot start pack verification")?;
    index
        .stdin
        .take()
        .ok_or("selective transport: missing verifier input")?
        .write_all(pack)
        .map_err(|_| "selective transport: pack verifier rejected input")?;
    let indexed = index
        .wait_with_output()
        .map_err(|_| "selective transport: pack verifier did not terminate")?;
    if !indexed.status.success() {
        #[cfg(test)]
        if let Some(root) = std::env::var_os("GUIDANCE_TRANSPORT_EVIDENCE_ROOT") {
            std::fs::create_dir_all(&root)
                .map_err(|_| "cannot create fixture evidence directory")?;
            std::fs::write(std::path::Path::new(&root).join("rejected.pack"), pack)
                .map_err(|_| "cannot preserve fixture pack")?;
            std::fs::write(
                std::path::Path::new(&root).join("verifier-error.txt"),
                &indexed.stderr,
            )
            .map_err(|_| "cannot preserve fixture verification error")?;
        }
        return Err("selective transport: corrupt or incomplete received pack".into());
    }
    let output = verifier(directory.path())
        .arg("--git-dir")
        .arg(directory.path())
        .args(["cat-file", "--batch-all-objects", "--batch"])
        .output()
        .map_err(|_| "selective transport: cannot inspect verified objects")?;
    if !output.status.success() {
        return Err("selective transport: cannot read verified objects".into());
    }
    let mut remaining = output.stdout.as_slice();
    let mut objects = Vec::new();
    while !remaining.is_empty() {
        let end = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or("selective transport: incomplete object header")?;
        let header = std::str::from_utf8(&remaining[..end])
            .map_err(|_| "selective transport: malformed object header")?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 {
            return Err("selective transport: invalid verified object header".into());
        }
        let length: usize = fields[2]
            .parse()
            .map_err(|_| "selective transport: invalid object size")?;
        remaining = &remaining[end + 1..];
        if remaining.get(length) != Some(&b'\n') {
            return Err("selective transport: truncated verified object".into());
        }
        let object = ReceivedObject {
            oid: fields[0].to_owned(),
            kind: fields[1].to_owned(),
            data: remaining[..length].to_vec(),
        };
        verify(&object)?;
        objects.push(object);
        remaining = &remaining[length + 1..];
    }
    Ok(objects)
}

pub(super) fn verify(object: &ReceivedObject) -> Result<(), String> {
    let oid = gix_hash::ObjectId::from_hex(object.oid.as_bytes())
        .map_err(|_| "selective transport: invalid received object ID")?;
    let kind = gix_object::Kind::from_bytes(object.kind.as_bytes())
        .map_err(|_| "selective transport: invalid received object type")?;
    let actual = gix_object::compute_hash(oid.kind(), kind, &object.data)
        .map_err(|_| "selective transport: object hash computation failed")?;
    if actual != oid {
        return Err("selective transport: received object hash mismatch".into());
    }
    Ok(())
}

pub(super) fn validate_metadata(objects: &[ReceivedObject], root: &str) -> Result<(), String> {
    use std::collections::{BTreeMap, BTreeSet};
    let index: BTreeMap<_, _> = objects
        .iter()
        .map(|object| (object.oid.as_str(), object))
        .collect();
    if index.len() != objects.len()
        || index.get(root).map(|object| object.kind.as_str()) != Some("commit")
    {
        return Err("selective transport: duplicate objects or missing requested commit".into());
    }
    let mut pending = vec![root.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(oid) = pending.pop() {
        if !visited.insert(oid.clone()) {
            continue;
        }
        let object = index
            .get(oid.as_str())
            .ok_or("selective transport: incomplete metadata tree")?;
        let hash = gix_hash::ObjectId::from_hex(oid.as_bytes())
            .map_err(|_| "selective transport: invalid metadata object ID")?
            .kind();
        match object.kind.as_str() {
            "commit" => {
                let commit = gix_object::CommitRef::from_bytes(&object.data, hash)
                    .map_err(|_| "selective transport: malformed received commit")?;
                pending.push(commit.tree().to_string());
                pending.extend(
                    commit
                        .parents()
                        .map(|parent| parent.to_string())
                        .filter(|parent| index.contains_key(parent.as_str())),
                );
            }
            "tree" => {
                let tree = gix_object::TreeRef::from_bytes(&object.data, hash)
                    .map_err(|_| "selective transport: malformed received tree")?;
                pending.extend(
                    tree.entries
                        .iter()
                        .filter(|entry| entry.mode.is_tree())
                        .map(|entry| entry.oid.to_string()),
                );
            }
            _ => return Err("selective transport: unexpected object in metadata-only pack".into()),
        }
    }
    if visited.len() != objects.len() {
        return Err(
            "selective transport: unsolicited object outside requested metadata closure".into(),
        );
    }
    Ok(())
}
