use crate::guidance::remote::transport::ReceivedObject;
use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

pub(super) fn corruption<T>(detail: &str) -> Option<T> {
    eprintln!("guidance cache corruption: {detail}");
    None
}

pub(super) fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "guidance cache: cannot read {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn read_object(directory: &Path, oid: &str) -> Result<Option<ReceivedObject>, String> {
    let path = directory.join("objects").join(format!("{oid}.obj"));
    let Some(bytes) = read(&path)? else {
        return Ok(None);
    };
    let Some(end) = bytes.iter().position(|byte| *byte == b'\n') else {
        return Ok(corruption(&format!(
            "invalid header for {oid}; refetching object"
        )));
    };
    let kind = match std::str::from_utf8(&bytes[..end]) {
        Ok(kind) => kind.to_owned(),
        Err(_) => {
            return Ok(corruption(&format!(
                "invalid type for {oid}; refetching object"
            )));
        }
    };
    let object = ReceivedObject {
        oid: oid.to_owned(),
        kind,
        data: bytes[end + 1..].to_vec(),
    };
    if let Err(error) = object.verify() {
        return Ok(corruption(&format!("{oid}: {error}; refetching object")));
    }
    Ok(Some(object))
}

pub(super) fn write_object(directory: &Path, object: &ReceivedObject) -> Result<(), String> {
    object.verify()?;
    if let Some(existing) = read_object(directory, &object.oid)? {
        if existing.kind == object.kind {
            return Ok(());
        }
        return Err("guidance cache: immutable object changed type".into());
    }
    let mut bytes = Vec::with_capacity(object.kind.len() + 1 + object.data.len());
    bytes.extend_from_slice(object.kind.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(&object.data);
    publish(
        &directory
            .join("objects")
            .join(format!("{}.obj", object.oid)),
        &bytes,
    )
}

pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("guidance cache: publication requires a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("guidance cache: cannot allocate atomic publication: {error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("guidance cache: incomplete temporary write: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("guidance cache: cannot synchronize completed file: {error}"))?;
    temporary.persist(path).map_err(|error| {
        format!(
            "guidance cache: cannot publish completed file: {}",
            error.error
        )
    })?;
    Ok(())
}
