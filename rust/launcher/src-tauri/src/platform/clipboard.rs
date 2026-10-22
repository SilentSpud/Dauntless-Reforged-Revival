//! `copyText`: the page has no clipboard permission, so an invite link is copied here (RD22). The
//! command checks the 4096-character limit (`bridge::spec::COPY_TEXT_MAX`). RA2's stub; RA11 fills it.

use crate::stub::{NotImplemented, not_implemented};

/// Puts `text` on the system clipboard.
pub fn write_text(_text: &str) -> Result<(), NotImplemented> {
    Err(not_implemented("RA11"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_names_its_row() {
        assert_eq!(write_text("x"), Err(not_implemented("RA11")));
    }
}
