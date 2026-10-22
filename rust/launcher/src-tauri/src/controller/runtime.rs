//! The server's game runtime (Reforged's `controller.ts` 1081–1226): accept a server's runtime set once,
//! install it next to the game, or use a host's local build instead. RA2's stub; RA13 fills it on RA7's
//! `net::runtime_files`. It replaces DR's 1.4.4 download, repair, pause and resume.

use super::Controller;
use crate::bridge::types::ActionResult;

const ROW: &str = "RA13";

impl Controller {
    /// The player accepted the runtime the server offers: `trustedRuntimes[serverId]`.
    pub async fn trust_runtime(&self) -> ActionResult {
        self.stub("trust_runtime", ROW).into()
    }

    /// Downloads the trusted runtime into `Win64` as a `runtime` task.
    pub async fn install_runtime(&self) -> ActionResult {
        self.stub("install_runtime", ROW).into()
    }

    /// A host's own build, used instead of the server's until cleared.
    pub async fn choose_local_runtime(&self) -> ActionResult {
        self.stub("choose_local_runtime", ROW).into()
    }

    pub async fn clear_local_runtime(&self) -> ActionResult {
        self.stub("clear_local_runtime", ROW).into()
    }
}
