//! `reforged-host director serve --env-file <file>` runs the Director (`npm start`); `serve` is the
//! default.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use service_kit::env::Env;

#[derive(Debug, Args)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Starts the game servers and answers the backend's matchmaking on loopback.
    Serve {
        #[arg(long)]
        env_file: Option<PathBuf>,
    },
}

fn fatal(message: &str) -> ExitCode {
    crate::log::error(&format!("DeployServer startup failed: {message}"));
    ExitCode::FAILURE
}

pub fn run(cli: &Cli) -> ExitCode {
    let Command::Serve { env_file } = cli.command.clone().unwrap_or(Command::Serve { env_file: None });
    let config = Env::load(env_file.as_deref())
        .map_err(|e| e.to_string())
        .and_then(|env| crate::config::Config::from_env(&env));
    let config = match config {
        Ok(config) => config,
        Err(error) => return fatal(&format!("Configuration: {error}")),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return fatal(&error.to_string()),
    };
    match runtime.block_on(crate::server::run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fatal(&error),
    }
}
