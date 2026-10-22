//! The server's game runtime on this PC (Reforged's `src/main/runtime-files.ts`): `MysticParadox.dll` and
//! `winmm.dll` next to the game, the set id a player accepts once per server, the check before every
//! launch, the install (download with `net::http::request_to_file`, replace in use, unblock) and a host's
//! local build. RA2's stub; RA7 fills it. `runtime_set_id` must stay byte for byte Reforged's, so accepted
//! runtimes stay accepted.

use std::path::Path;

use crate::bridge::types::{RuntimeFileInfo, RuntimeState};
use crate::stub::{NotImplemented, not_implemented};

const ROW: &str = "RA7";

/// SHA-256 hex of the lines `name:size:sha256`, sorted by name, joined by `\n`.
pub fn runtime_set_id(_files: &[RuntimeFileInfo]) -> Result<String, NotImplemented> {
    Err(not_implemented(ROW))
}

/// The runtime installed in `win64_dir` against the server's list, and whether the game can start.
pub fn check_runtime(
    _win64_dir: &Path,
    _offered: Option<&[RuntimeFileInfo]>,
    _trusted: bool,
    _local_dir: Option<&Path>,
) -> Result<(RuntimeState, bool), NotImplemented> {
    Err(not_implemented(ROW))
}

/// Both runtime files present and non-empty.
pub fn is_complete_runtime_dir(_dir: &Path) -> Result<bool, NotImplemented> {
    Err(not_implemented(ROW))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stubs_name_their_row() {
        assert_eq!(runtime_set_id(&[]), Err(not_implemented(ROW)));
        assert_eq!(
            check_runtime(Path::new("."), None, false, None),
            Err(not_implemented(ROW))
        );
        assert_eq!(is_complete_runtime_dir(Path::new(".")), Err(not_implemented(ROW)));
    }
}
