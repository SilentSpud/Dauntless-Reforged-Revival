//! The host kit's workspace as the Host page sees it (Reforged's `src/main/hostkit.ts`): the project
//! folder, the SDK, the Dumper-7 dumps, the builds, the game data and the server's `host.json`. RA2's stub;
//! RA12 fills it for the planned layout only (RD23).

use std::path::Path;

use crate::bridge::types::HostKit;
use crate::stub::{NotImplemented, not_implemented};

/// What the Host page shows for the workspace at `workspace` (None: not found yet).
pub async fn detect_host_kit(_workspace: Option<&Path>) -> Result<HostKit, NotImplemented> {
    Err(not_implemented("RA12"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_stub_names_its_row() {
        assert_eq!(detect_host_kit(None).await, Err(not_implemented("RA12")));
    }
}
