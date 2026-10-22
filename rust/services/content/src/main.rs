//! The `content` binary: `content serve [--env-file <file>]` runs the server (`npm start`), `content
//! verify` checks the game folder against the manifest (`npm run verify`). Without `--env-file`, a
//! `.env` in the working folder is read if there is one, as `node --env-file-if-exists=.env` did.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use service_kit::env::Env;

#[derive(Parser)]
#[command(name = "content", version, about = "Dauntless Revived content server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serves the game files, news and art pack.
    Serve {
        #[arg(long)]
        env_file: Option<PathBuf>,
    },
    /// Hashes the game folder against the manifest.
    Verify {
        /// Sizes only.
        #[arg(long)]
        quick: bool,
        #[arg(long)]
        game_dir: Option<PathBuf>,
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
}

fn env_file(given: Option<PathBuf>) -> Option<PathBuf> {
    given.or_else(|| Some(PathBuf::from(".env")).filter(|p| p.is_file()))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve { env_file: None }) {
        Command::Serve { env_file: file } => serve(env_file(file)),
        Command::Verify {
            quick,
            game_dir,
            manifest,
        } => {
            if let Some(file) = env_file(None) {
                let _ = dotenv_into_process(&file);
            }
            ExitCode::from(content::verify::run(game_dir, manifest, quick) as u8)
        }
    }
}

/// `verify` reads CONTENT_GAME_DIR and CONTENT_MANIFEST from the process environment, as Node did
/// after `--env-file-if-exists=.env`.
fn dotenv_into_process(file: &std::path::Path) -> Result<(), String> {
    let env = Env::load(Some(file)).map_err(|e| e.to_string())?;
    for name in ["CONTENT_GAME_DIR", "CONTENT_MANIFEST"] {
        if std::env::var_os(name).is_none()
            && let Some(value) = env.raw(name)
        {
            // Set before any thread starts: verify is single-threaded
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var(name, value);
            }
        }
    }
    Ok(())
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
