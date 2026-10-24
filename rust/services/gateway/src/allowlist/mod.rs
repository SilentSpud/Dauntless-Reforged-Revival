//! The allowlist helper (`src/allowlist`; MIGRATION_PLAN.md RS4): keeps the game ports open to
//! exactly the addresses of players who logged in, as the gateway reports them, through Windows
//! Firewall or one nftables table on a Linux VPS. It runs with administrator rights (or
//! `CAP_NET_ADMIN`), so it listens on loopback only, wants the shared secret on every request, and
//! only ever changes its own firewall rule or table.

pub mod cli;
pub mod config;
pub mod firewall;
pub mod helper;
pub mod nft;
pub mod ports;
pub mod state;
pub mod sync;
#[cfg(windows)]
pub mod windows;
