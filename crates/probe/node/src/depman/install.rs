use crate::{depman::NodeDependencyManager, NodeDependencyManagerType};
use log::debug;
use probe_core::{async_trait, untar, Describable, Installable, ProbeError, Resolvable};
use std::path::{Path, PathBuf};

#[async_trait]
impl Installable<'_> for NodeDependencyManager {
    fn get_install_dir(&self) -> Result<PathBuf, ProbeError> {
        Ok(self.install_dir.join(self.get_resolved_version()))
    }

    async fn install(&self, install_dir: &Path, download_path: &Path) -> Result<bool, ProbeError> {
        if install_dir.exists() {
            debug!(target: self.get_log_target(), "Dependency manager already installed, continuing");

            return Ok(false);
        }

        // This may not be accurate for all releases!
        let prefix = if matches!(&self.type_of, NodeDependencyManagerType::Yarn) {
            format!("yarn-v{}", self.get_resolved_version())
        } else {
            "package".into()
        };

        debug!(
            target: self.get_log_target(),
            "Attempting to install {} to {}",
            download_path.to_string_lossy(),
            install_dir.to_string_lossy(),
        );

        untar(download_path, install_dir, Some(&prefix))?;

        debug!(target: self.get_log_target(), "Successfully installed dependency manager");

        Ok(true)
    }
}