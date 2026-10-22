//! The allowlist helper (`src/allowlist`, and the Linux kit's `allowlist.mjs`; MIGRATION_PLAN.md
//! S3): keeps the game ports open to exactly the addresses of players who logged in, as the gateway
//! reports them. It runs with administrator rights, so it listens on loopback only, wants the shared
//! secret on every request, and only ever changes its own firewall rule.

pub mod config;
pub mod firewall;
pub mod helper;
pub mod state;
pub mod sync;
#[cfg(windows)]
pub mod windows;
