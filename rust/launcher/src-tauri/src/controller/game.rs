//! The game folder (Reforged's `controller.ts` 949–1077): find, choose and check an existing Dauntless
//! 1.14.7. RA2's stub; RA13 fills it on RA9's `platform::game_folder`. It replaces DR's install folder
//! and 1.4.4 download (`choose_install_dir`, `use_existing_game_folder`, `use_existing_game_path`).

use super::Controller;
use crate::bridge::types::ActionResult;

const ROW: &str = "RA13";

impl Controller {
    /// "Choose folder…": a folder dialog, then the same checks as a pasted path.
    pub async fn choose_game_folder(&self) -> ActionResult {
        self.stub("choose_game_folder", ROW).into()
    }

    /// A pasted path: the Dauntless folder, its parent, `Archon` or `Win64`; verified at once when
    /// unverified.
    pub async fn use_game_path(&self, _game_path: &str) -> ActionResult {
        self.stub("use_game_path", ROW).into()
    }

    /// One of `game.detected`, by index (already checked to be below 16).
    pub async fn use_detected_game(&self, _index: usize) -> ActionResult {
        self.stub("use_detected_game", ROW).into()
    }

    /// Hashes the executable as a `verify` task and sets `game_verified`.
    pub async fn verify_game(&self) -> ActionResult {
        self.stub("verify_game", ROW).into()
    }
}
