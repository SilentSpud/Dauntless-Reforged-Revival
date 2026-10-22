//! Where the launcher keeps its files (row L26). Tauri's own app data folder would be
//! `…/io.github.mixutin.dauntlessrevived`; it is not used. The folders are the Electron launcher's,
//! so settings, keys, `logs/` and `offline/` of an upgraded profile keep working.

use std::path::{Path, PathBuf};

/// Electron's `userData`: the app's product name under the roaming app data folder (Windows) or
/// `$XDG_CONFIG_HOME` / `~/.config` (Linux).
pub const PRODUCT_NAME: &str = "Dauntless Revived Launcher";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub user_data: PathBuf,
    pub documents: PathBuf,
    pub home: PathBuf,
    pub default_install_dir: PathBuf,
}

impl Paths {
    /// From the operating system's folders.
    pub fn from_system() -> Option<Self> {
        let home = dirs::home_dir()?;
        // Debug builds only: a throwaway profile, so a developer's real settings and keys stay untouched
        if cfg!(debug_assertions)
            && let Some(dir) = std::env::var_os("DAUNTLESS_REVIVED_DEV_PROFILE").map(PathBuf::from)
        {
            let local = dir.join("Local");
            return Some(Self::with(dir.join("User Data"), dir.join("Documents"), home, &local));
        }
        let user_data = dirs::config_dir()?.join(PRODUCT_NAME);
        let documents = dirs::document_dir().unwrap_or_else(|| home.join("Documents"));
        let local = std::env::var_os("LOCALAPPDATA").map_or_else(|| home.join("AppData").join("Local"), PathBuf::from);
        Some(Self::with(user_data, documents, home, &local))
    }

    fn with(user_data: PathBuf, documents: PathBuf, home: PathBuf, local_app_data: &Path) -> Self {
        let default_install_dir = if cfg!(target_os = "linux") {
            home.join("Games").join("DauntlessRevived")
        } else {
            local_app_data.join("DauntlessRevived").join("Game")
        };
        Self {
            user_data,
            documents,
            home,
            default_install_dir,
        }
    }

    #[cfg_attr(
        not(any(test, feature = "offline")),
        expect(dead_code, reason = "the tests and offline play use it")
    )]
    pub fn keys_dir(&self) -> PathBuf {
        self.user_data.join("keys")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.user_data.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_electron_folders() {
        let paths = Paths::from_system().unwrap();
        assert!(paths.user_data.ends_with(PRODUCT_NAME));
        assert_eq!(paths.keys_dir(), paths.user_data.join("keys"));
        if cfg!(windows) {
            let appdata = PathBuf::from(std::env::var_os("APPDATA").unwrap());
            assert_eq!(paths.user_data, appdata.join(PRODUCT_NAME));
            assert!(
                paths
                    .default_install_dir
                    .ends_with(Path::new("DauntlessRevived").join("Game"))
            );
        } else {
            assert!(paths.default_install_dir.ends_with("Games/DauntlessRevived"));
        }
    }
}
