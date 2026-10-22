//! The `content` binary: `content serve [--env-file <file>]` runs the server (`npm start`); `serve` is
//! the default. Without `--env-file`, a `.env` in the working folder is read if there is one, as
//! `node --env-file-if-exists=.env` did.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use service_kit::env::Env;

#[derive(Parser)]
#[command(name = "content", version, about = "Dauntless Reforged Revival content server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Command::Serve { env_file: file } = cli.command.unwrap_or(Command::Serve { env_file: None });
    serve(env_file(file))
}

fn serve(env_file: Option<PathBuf>) -> ExitCode {
    let env = match Env::load(env_file.as_deref()) {
        Ok(env) => env,
        Err(error) => {
            content::log::error(&format!("Configuration: {error}"), serde_json::json!({}));
            return ExitCode::FAILURE;
        }
    };
    let config = match content::config::Config::from_env(&env) {
        Ok(config) => config,
        Err(error) => {
            content::log::error(&format!("Configuration: {error}"), serde_json::json!({}));
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            content::log::error(&error.to_string(), serde_json::json!({}));
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(content::server::run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            content::log::error(&error, serde_json::json!({}));
            ExitCode::FAILURE
        }
    }
}
