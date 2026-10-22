//! The launcher's rules that decide where keys go and what the page shows (rows L11 to L18). These
//! are authoritative; the TypeScript copies in `src/shared` stay only for the page's feedback, and
//! both run the same test tables in `test/cases`.

pub mod game_language;
pub mod i18n;
pub mod invite;
pub mod manifest;
#[allow(
    dead_code,
    reason = "the platform layer (P4) and the downloader (P3) use the rest of the pins"
)]
pub mod pins;
pub mod text;

pub mod status;
pub mod username;
