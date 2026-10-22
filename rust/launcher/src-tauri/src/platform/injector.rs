//! Loading a DLL into a running game (Reforged's `src/main/injector.ts`, RD9): Dumper-7 for the SDK and
//! CatalogExporter for the game data, both by process id. RA2's stub; RA12 fills it (Windows), RA12a
//! decides Linux.

use std::path::PathBuf;

use crate::stub::{NotImplemented, not_implemented};

/// Why an injection failed. RA12 replaces the stub with its own reasons.
pub type InjectError = NotImplemented;

/// Loads `dll` into the process `pid`.
pub async fn inject(_pid: u32, _dll: PathBuf) -> Result<(), InjectError> {
    Err(not_implemented("RA12"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_stub_names_its_row() {
        assert_eq!(inject(1, PathBuf::new()).await, Err(not_implemented("RA12")));
    }
}
