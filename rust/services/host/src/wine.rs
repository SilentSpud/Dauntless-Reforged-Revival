//! Running the Windows game servers through Wine or Proton (`legacy/ReforgedHost/src/wine.ts` and
//! `gameserver.ts`): what the Director starts for each game server on Linux, through
//! `.workspace/bin/reforged-gameserver` and the hidden `reforged-host gameserver` command.
//!
//! Filled by RH5.

use crate::error::{HostError, not_implemented};

/// The first of the programs found on PATH, as a full path.
pub fn find_on_path(_names: &[&str]) -> Result<Option<String>, HostError> {
    not_implemented("searching PATH", "RH5")
}

/// Runs one game server with the Director's arguments; returns its exit code.
pub fn run_game_server(_args: &[String]) -> Result<i32, HostError> {
    not_implemented("reforged-host gameserver", "RH5")
}
