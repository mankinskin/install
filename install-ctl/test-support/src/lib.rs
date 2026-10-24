mod cgi;
mod http;

pub use http::HttpFixture;

use std::path::{Path, PathBuf};

pub fn helper_path() -> PathBuf {
    PathBuf::from(env!("INSTALL_CTL_RAW_HELPER"))
}

pub fn command_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
