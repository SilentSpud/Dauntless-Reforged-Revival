//! The Host page's Requirements tab (Reforged's `src/main/requirements.ts`): what this PC has of what a
//! host needs, each detected afresh with a 15 s cache. RA2's stub; RA12 fills it (Postgres instead of
//! Node and MongoDB once RB14 ships, RD23).

use crate::bridge::types::Requirement;
use crate::stub::{NotImplemented, not_implemented};

pub async fn check_requirements() -> Result<Vec<Requirement>, NotImplemented> {
    Err(not_implemented("RA12"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_stub_names_its_row() {
        assert_eq!(check_requirements().await, Err(not_implemented("RA12")));
    }
}
