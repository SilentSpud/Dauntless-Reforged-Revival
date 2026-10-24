//! The Director (Mystic Paradox's DeployServer, `paradox/director`) in Rust, MIGRATION_PLAN.md RB10:
//! it starts the game servers, Ramsgate and the Training Grounds kept up by their supervisors and a
//! process per hunt, and tells the backend where each player goes. Its routes, replies and the game
//! servers' command lines are the Node Director's.

pub mod app;
pub mod cli;
pub mod cmdline;
pub mod config;
pub mod gameservers;
pub mod hub;
pub mod hunt;
pub mod log;
pub mod matchmaker;
pub mod memory;
pub mod occupancy;
pub mod port_pool;
pub mod process;
pub mod random;
pub mod runtime_update;
pub mod server;
#[cfg(windows)]
mod win_spawn;
