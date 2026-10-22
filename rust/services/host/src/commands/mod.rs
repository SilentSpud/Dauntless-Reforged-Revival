//! One module per command. Each names the MIGRATION_PLAN.md row that fills it; until then it returns
//! [`crate::error::HostError::NotImplemented`].
//!
//! | Module | Command | Row |
//! | --- | --- | --- |
//! | [`start`] | `start` | RH2 |
//! | [`setup`] | `setup` | RH3 |
//! | [`invite`] | `invite` | RH3 |
//! | [`doctor`] | `doctor` | RH3 |
//! | [`multipliers`] | `multipliers` | RH3 |
//! | [`backup`] | `backup` (new) | RH3 |
//! | [`gamedata`] | `gamedata` | RH4 |
//! | [`preset`] | `preset` | RH4 |
//! | [`publish_runtime`] | `publish-runtime` | RH4 |
//! | [`update`] | `update` | RH5 |
//! | `crate::wine::run_game_server` | `gameserver` (hidden, Linux) | RH5 |

pub mod backup;
pub mod doctor;
pub mod gamedata;
pub mod invite;
pub mod multipliers;
pub mod preset;
pub mod publish_runtime;
pub mod setup;
pub mod start;
pub mod update;

use std::process::ExitCode;

use crate::Outcome;
use crate::cli::Command;
use crate::paths::Paths;

/// Runs a command against the checkout at `paths`.
pub fn dispatch(command: &Command, paths: &Paths) -> Outcome {
    match command {
        Command::Setup(args) => setup::run(paths, args),
        Command::Start => start::run(paths),
        Command::Invite(args) => invite::run(paths, args),
        Command::Doctor(args) => doctor::run(paths, args),
        Command::Gamedata(args) => gamedata::run(paths, args),
        Command::PublishRuntime(args) => publish_runtime::run(paths, args),
        Command::Preset(args) => preset::run(paths, args),
        Command::Multipliers(args) => multipliers::run(paths, args),
        Command::Update(args) => update::run(paths, args),
        Command::Backup => backup::run(paths),
        Command::Gameserver { args } => {
            let code = crate::wine::run_game_server(args)?;
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
    }
}
