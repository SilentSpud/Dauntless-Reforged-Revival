//! Mystic Paradox at the pinned commit with the Reforged patches (`legacy/ReforgedHost/src/upstream.ts`).
//!
//! Filled by RH3 for setup's first step. Once the fork is in `paradox/` (RP2, RP4) and the services
//! come from the server release, RH5 replaces it and removes the `upstream` command.

use std::path::{Path, PathBuf};

use crate::error::{HostError, not_implemented};

/// `upstream/mystic-paradox.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamPin {
    pub name: String,
    pub repository: String,
    pub commit: String,
    pub patches: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamResult {
    AlreadyReady,
    Fetched,
    Updated,
}

#[derive(Debug, Clone)]
pub struct EnsureUpstream<'a> {
    pub dir: &'a Path,
    pub marker_file: &'a Path,
    pub pin: &'a UpstreamPin,
    pub patches_root: &'a Path,
    pub log_file: Option<PathBuf>,
    pub force: bool,
}

pub fn git_available() -> Result<bool, HostError> {
    not_implemented("checking for git", "RH3")
}

pub fn read_pin(_file: &Path) -> Result<UpstreamPin, HostError> {
    not_implemented("reading the upstream pin", "RH3")
}

pub fn ensure_upstream(_options: &EnsureUpstream<'_>) -> Result<UpstreamResult, HostError> {
    not_implemented("fetching and patching Mystic Paradox", "RH3")
}
