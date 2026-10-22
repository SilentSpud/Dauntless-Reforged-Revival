//! Where the launcher keeps its files (row L26). Tauri's own app data folder would be
//! `…/io.github.freezamotoqueiro.dauntlessreforgedrevival`; it is not used. The folders are the Electron launcher's,
//! so settings, keys and `logs/` of an upgraded 1.5 profile keep working.

use std::path::PathBuf;

/// Electron's `userData`: the app's product name under the roaming app data folder (Windows) or
/// `$XDG_CONFIG_HOME` / `~/.config` (Linux).
pub const PRODUCT_NAME: &str = "Dauntless Reforged Revival Launcher";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub user_data: PathBuf,
    pub documents: PathBuf,
    pub home: PathBuf,
}

impl Paths {
    /// From the operating system's folders.
    pub fn from_system() -> Option<Self> {
        let home = dirs::home_dir()?;
        // Debug builds only: a throwaway profile, so a developer's real settings and keys stay untouched
        if cfg!(debug_assertions)
            && let Some(dir) = std::env::var_os("REFORGED_DEV_PROFILE").map(PathBuf::from)
        {
            return Some(Self {
                user_data: dir.join("User Data"),
                documents: dir.join("Documents"),
                home,
            });
        }
        let user_data = dirs::config_dir()?.join(PRODUCT_NAME);
        let documents = dirs::document_dir().unwrap_or_else(|| home.join("Documents"));
        Some(Self {
            user_data,
            documents,
            home,
        })
    }

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
        }
    }
}
