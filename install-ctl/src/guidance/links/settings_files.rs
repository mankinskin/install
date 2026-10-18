use std::{
    fs,
    io::{self, Write},
    path::Path,
};

pub(super) fn safe_directory(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if super::super::ordinary_directory(&metadata) => Ok(true),
        Ok(_) => Err(format!(
            "settings ancestor '{}' is not an ordinary directory",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect '{}': {error}", path.display())),
    }
}

pub(super) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if safe_file(&metadata) => fs::read(path)
            .map(Some)
            .map_err(|error| format!("cannot read '{}': {error}", path.display())),
        Ok(_) => Err(format!(
            "configuration '{}' is not an ordinary file",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot inspect '{}': {error}", path.display())),
    }
}

fn safe_file(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.is_file() && metadata.file_attributes() & 0x400 == 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_file() && !metadata.file_type().is_symlink()
    }
}

pub(super) fn replace(path: &Path, bytes: &Option<Vec<u8>>) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid configuration path")?;
    if !safe_directory(parent)? {
        if bytes.is_none() {
            return Ok(());
        }
        fs::create_dir(parent)
            .map_err(|error| format!("cannot create '{}': {error}", parent.display()))?;
    }
    read_optional(path)?;
    match bytes {
        Some(bytes) => {
            let mut staged = tempfile::NamedTempFile::new_in(parent)
                .map_err(|error| format!("cannot stage '{}': {error}", path.display()))?;
            staged
                .write_all(bytes)
                .and_then(|()| staged.as_file().sync_all())
                .map_err(|error| format!("cannot stage '{}': {error}", path.display()))?;
            staged
                .persist(path)
                .map_err(|error| format!("cannot replace '{}': {}", path.display(), error.error))?;
        }
        None => match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot remove '{}': {error}", path.display())),
        },
    }
    Ok(())
}

pub(super) fn remove_empty_directory(path: &Path) -> Result<(), String> {
    if !safe_directory(path)? {
        return Ok(());
    }
    fs::remove_dir(path)
        .map_err(|error| format!("cannot remove call-created '{}': {error}", path.display()))
}
