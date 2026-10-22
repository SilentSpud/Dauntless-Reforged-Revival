//! The project's server preset in `preset/` (`legacy/ReforgedHost/src/preset.ts`).
//!
//! Filled by RH4. `setup` (RH3) applies it with [`apply_preset`].

use crate::error::{HostError, not_implemented};
use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetApplied {
    pub name: String,
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
    /// The host's own game data, left in place.
    pub kept: Vec<String>,
    pub backups: Vec<String>,
}

/// Applies the preset where the server has no game data of its own (all of it with `force`).
/// `None` when there is no preset.
pub fn apply_preset(_paths: &Paths, _force: bool) -> Result<Option<PresetApplied>, HostError> {
    not_implemented("applying the server preset", "RH4")
}
