//! Wire types shared by the launcher and the servers.
//!
//! Each type here has one definition that both sides compile against, so a changed field breaks the
//! build on both sides instead of failing at runtime (MIGRATION_PLAN.md D9, X4). Their JSON form is
//! the one the TypeScript code reads and writes today.

pub mod account;
pub mod invite;
pub mod manifest;
pub mod status;
