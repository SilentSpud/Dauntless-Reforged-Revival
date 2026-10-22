//! Publishing the game runtime (`legacy/ReforgedHost/src/publish.ts`): the files and `runtime.json`
//! the content server serves, and the copies beside the game servers' executable.
//!
//! Filled by RH4. `start` (RH2) uses [`ensure_server_flags`].

use std::path::Path;

use crate::error::{HostError, not_implemented};

/// Flag files the runtime reads beside the game servers' executable.
pub const SERVER_RUNTIME_FLAGS: [&str; 1] = ["CALL_HUNTPASS_QUERY.flag"];

/// Writes the missing flag files into the `Win64` folder; returns the ones it wrote.
pub fn ensure_server_flags(_win64_dir: &Path) -> Result<Vec<String>, HostError> {
    not_implemented("writing the game servers' flag files", "RH4")
}
