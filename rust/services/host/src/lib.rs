//! The host kit, `reforged-host` (MIGRATION_PLAN.md RD3, section 4.4): the Node kit in
//! `legacy/ReforgedHost` as one Rust binary, with the same commands and options and the same files
//! (the `.env` files; `host.json` became `host.toml`, read from a Node kit's host.json until the
//! first write), so an existing host moves over with `update`.
//!
//! RH1 is the skeleton: the command line ([`cli`]), the layout ([`paths`]), the env files
//! ([`envfile`]) and the settings with the service env files built from it ([`hostconfig`]) are
//! real. Each command is a module in [`commands`]; the other modules follow the Node kit's files and
//! name the row that fills them. Shared helpers one row writes and another calls are declared here
//! with their signatures, so the wave 2 rows (RH2-RH5) each fill only their own files.

pub mod cli;
pub mod commands;
pub mod envfile;
pub mod error;
pub mod hostconfig;
pub mod hosttoml;
pub mod paths;
pub mod servers;

// RH2
pub mod run;
pub mod services;
// RH3
pub mod doctor;
pub mod invite;
pub mod mongo;
// RH4
pub mod captures;
pub mod gamedata;
pub mod gamepack;
pub mod preset;
pub mod progression;
pub mod publish;
// RH5
pub mod linux;
pub mod selfupdate;
pub mod wine;

use std::process::ExitCode;

pub use error::HostError;

/// What a command returns: its exit code, or the error `main` prints.
pub type Outcome = Result<ExitCode, HostError>;

/// Runs the parsed command line: finds the checkout, then the command.
pub fn run(cli: &cli::Cli) -> Outcome {
    // A server needs no checkout: its env file says everything (DR X7).
    if let cli::Command::Server(server) = &cli.command {
        return Ok(server.run());
    }
    let root = paths::locate_repo_root()?;
    commands::dispatch(&cli.command, &paths::Paths::new(root))
}
