//! `Installable` trait contract modeling installation, uninstallation, and
//! status queries across installable artifacts and domain components.

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallReport {
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallReport {
    pub removed: Vec<String>,
    pub missing: Vec<String>,
}

/// Core trait required for all installable artifacts and domain components.
/// Enforces that every installable entity defines its uninstallation path
/// alongside its installation path.
pub trait Installable {
    /// Unique identifier for the artifact or component.
    fn id(&self) -> &str;

    /// Query whether the artifact is currently installed under `target_root`.
    fn is_installed(&self, target_root: &Path) -> Result<bool, String>;

    /// Install the artifact into `target_root`.
    fn install(&self, target_root: &Path) -> Result<InstallReport, String>;

    /// Uninstall the artifact from `target_root`.
    fn uninstall(&self, target_root: &Path) -> Result<UninstallReport, String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct MockArtifact {
        id: String,
        file_name: String,
    }

    impl Installable for MockArtifact {
        fn id(&self) -> &str {
            &self.id
        }

        fn is_installed(&self, target_root: &Path) -> Result<bool, String> {
            Ok(target_root.join(&self.file_name).is_file())
        }

        fn install(&self, target_root: &Path) -> Result<InstallReport, String> {
            let path = target_root.join(&self.file_name);
            if path.is_file() {
                Ok(InstallReport {
                    written: vec![],
                    unchanged: vec![path.display().to_string()],
                })
            } else {
                fs::write(&path, "mock-binary-content").map_err(|e| e.to_string())?;
                Ok(InstallReport {
                    written: vec![path.display().to_string()],
                    unchanged: vec![],
                })
            }
        }

        fn uninstall(&self, target_root: &Path) -> Result<UninstallReport, String> {
            let path = target_root.join(&self.file_name);
            if path.is_file() {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
                Ok(UninstallReport {
                    removed: vec![path.display().to_string()],
                    missing: vec![],
                })
            } else {
                Ok(UninstallReport {
                    removed: vec![],
                    missing: vec![path.display().to_string()],
                })
            }
        }
    }

    #[test]
    fn mock_artifact_installable_lifecycle() {
        let dir = TempDir::new().unwrap();
        let artifact = MockArtifact {
            id: "test-tool".to_string(),
            file_name: "test-tool.bin".to_string(),
        };

        assert!(!artifact.is_installed(dir.path()).unwrap());

        let install_rep = artifact.install(dir.path()).unwrap();
        assert_eq!(install_rep.written.len(), 1);
        assert!(artifact.is_installed(dir.path()).unwrap());

        let reinstall_rep = artifact.install(dir.path()).unwrap();
        assert_eq!(reinstall_rep.unchanged.len(), 1);

        let uninstall_rep = artifact.uninstall(dir.path()).unwrap();
        assert_eq!(uninstall_rep.removed.len(), 1);
        assert!(!artifact.is_installed(dir.path()).unwrap());

        let second_uninstall = artifact.uninstall(dir.path()).unwrap();
        assert_eq!(second_uninstall.missing.len(), 1);
    }
}
