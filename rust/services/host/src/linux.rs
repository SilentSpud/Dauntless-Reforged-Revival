//! Linux hosts (`legacy/ReforgedHost/src/linux.ts`): privileges, the systemd units, the ufw script
//! and the Wine prefix.
//!
//! Filled by RH5, with the Linux release. `setup` (RH3) and `start` (RH2) use the helpers below.

use std::path::{Path, PathBuf};

use crate::error::{HostError, not_implemented};
use crate::hostconfig::{HostConfig, LinuxOptions};
use crate::paths::Paths;

pub const UNIT_SERVER: &str = "reforged.service";
pub const UNIT_ALLOWLIST: &str = "reforged-allowlist.service";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Wine,
    Proton,
}

/// What runs the game servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRuntime {
    pub kind: RuntimeKind,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinePrefix {
    pub ok: bool,
    pub created: bool,
    pub detail: Option<String>,
}

pub fn is_root() -> Result<bool, HostError> {
    not_implemented("checking for root", "RH5")
}

pub fn can_bind_port(_port: u16) -> Result<bool, HostError> {
    not_implemented("checking the port privilege", "RH5")
}

pub fn can_change_firewall() -> Result<bool, HostError> {
    not_implemented("checking the firewall privilege", "RH5")
}

/// `None` when neither Proton nor Wine was found.
pub fn game_runtime(_options: &LinuxOptions) -> Result<Option<GameRuntime>, HostError> {
    not_implemented("finding Wine or Proton", "RH5")
}

pub fn ensure_wine_prefix(_paths: &Paths, _options: &LinuxOptions, _log_file: &Path) -> Result<WinePrefix, HostError> {
    not_implemented("preparing the Wine prefix", "RH5")
}

/// Writes `.workspace/bin/reforged-gameserver`; returns its path.
pub fn write_gameserver_wrapper(_paths: &Paths) -> Result<PathBuf, HostError> {
    not_implemented("writing the game-server launcher", "RH5")
}

/// Writes the systemd units and their `install.sh`; returns the files.
pub fn write_systemd_units(_config: &HostConfig, _paths: &Paths) -> Result<Vec<PathBuf>, HostError> {
    not_implemented("writing the systemd units", "RH5")
}

/// The ufw script setup writes to `firewall.sh`.
pub fn linux_firewall_script(_config: &HostConfig) -> Result<String, HostError> {
    not_implemented("the ufw script", "RH5")
}
