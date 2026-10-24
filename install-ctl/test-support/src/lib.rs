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

pub fn python_denial_path(root: &Path) -> PathBuf {
    let trap = root.join("python-denial-bin");
    std::fs::create_dir_all(&trap).expect("create owned denial directory");
    for name in ["python", "python3"] {
        std::fs::copy(helper_path(), trap.join(name)).expect("copy Python denial trap");
    }
    let path = format!(
        "{}:{}",
        trap.display(),
        std::env::var("PATH").expect("PATH is available")
    );
    std::fs::write(root.join("python-denial.path"), path).expect("write denial PATH evidence");
    trap
}
