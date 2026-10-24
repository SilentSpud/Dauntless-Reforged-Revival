//! `reforged-host content serve [--env-file <file>]` runs the server (`npm start`); `serve` is
//! the default. Without `--env-file`, a `.env` in the working folder is read if there is one, as
//! `node --env-file-if-exists=.env` did.

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
    /// Serves the game runtime, news and art pack.
    Serve {
        #[arg(long)]
        env_file: Option<PathBuf>,
    },
}

fn env_file(given: Option<PathBuf>) -> Option<PathBuf> {
    given.or_else(|| Some(PathBuf::from(".env")).filter(|p| p.is_file()))
}

pub fn run(cli: &Cli) -> ExitCode {
    let Command::Serve { env_file: file } = cli.command.clone().unwrap_or(Command::Serve { env_file: None });
    serve(env_file(file))
}

fn serve(env_file: Option<PathBuf>) -> ExitCode {
    let env = match Env::load(env_file.as_deref()) {
        Ok(env) => env,
        Err(error) => {
            crate::log::error(&format!("Configuration: {error}"), serde_json::json!({}));
            return ExitCode::FAILURE;
        }
    };
    let config = match crate::config::Config::from_env(&env) {
        Ok(config) => config,
        Err(error) => {
            crate::log::error(&format!("Configuration: {error}"), serde_json::json!({}));
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            crate::log::error(&error.to_string(), serde_json::json!({}));
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(crate::server::run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            crate::log::error(&error, serde_json::json!({}));
            ExitCode::FAILURE
        }
    }
}
