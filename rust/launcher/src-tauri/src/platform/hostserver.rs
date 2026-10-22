//! The server a host starts and stops from the Host page (Reforged's `src/main/hostserver.ts`):
//! `reforged-host` from the server release with `REFORGED_CONTROL=stdin`, the `REFORGED_SERVER_READY`
//! line and the port 61000 probe (RD23, RH2). RA2's stub; RA12 fills it.

use crate::bridge::types::HostServerView;
use crate::stub::{NotImplemented, not_implemented};

/// The server this launcher runs, if any.
#[derive(Default)]
pub struct HostServer {}

impl HostServer {
    pub fn view(&self) -> HostServerView {
        HostServerView {
            state: crate::bridge::types::HostServerState::Stopped,
            message: None,
            lines: Vec::new(),
            invite: None,
        }
    }

    pub async fn start(&self) -> Result<(), NotImplemented> {
        Err(not_implemented("RA12"))
    }

    pub async fn stop(&self) -> Result<(), NotImplemented> {
        Err(not_implemented("RA12"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_stub_names_its_row() {
        let server = HostServer::default();
        assert_eq!(server.start().await, Err(not_implemented("RA12")));
        assert_eq!(server.stop().await, Err(not_implemented("RA12")));
        assert_eq!(server.view().state, crate::bridge::types::HostServerState::Stopped);
    }
}
