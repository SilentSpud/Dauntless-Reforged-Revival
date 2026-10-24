//! `reforged-host gateway serve [--env-file <file>]` runs the gateway (`npm start`),
//! `reforged-host gateway make-cert ...` makes a certificate for a new install (`npm run make-cert`). Without
//! `--env-file`, a `.env` in the working folder is read if there is one, as
//! `node --env-file-if-exists=.env` did.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use service_kit::env::Env;
use service_kit::linelog;

#[derive(Debug, Args)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Serves the public TLS port (plain HTTP on a Tailscale address with GATEWAY_TLS=0).
    Serve {
        #[arg(long)]
        env_file: Option<PathBuf>,
    },
    /// Makes a self-signed certificate for a new install, or prints a certificate's fingerprint
    /// (`make-cert --help` for the arguments).
    #[command(disable_help_flag = true)]
    MakeCert {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

pub fn run(cli: &Cli) -> ExitCode {
    match cli.command.clone().unwrap_or(Command::Serve { env_file: None }) {
        Command::Serve { env_file } => serve(env_file.or_else(|| Some(PathBuf::from(".env")).filter(|p| p.is_file()))),
        Command::MakeCert { args } => ExitCode::from(crate::cert::make_cert_main(&args) as u8),
    }
}

fn fatal(message: &str) -> ExitCode {
    linelog::error(message, serde_json::json!({}));
    ExitCode::FAILURE
}

fn serve(env_file: Option<PathBuf>) -> ExitCode {
    let env = match Env::load(env_file.as_deref()) {
        Ok(env) => env,
        Err(error) => return fatal(&format!("Configuration: {error}")),
    };
    let config = match crate::config::Config::from_env(&env) {
        Ok(config) => config,
        Err(error) => return fatal(&format!("Configuration: {error}")),
    };
    let capture = match crate::server::load_capture(&env) {
        Ok(capture) => capture,
        Err(error) => return fatal(&error),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return fatal(&error.to_string()),
    };
    match runtime.block_on(crate::server::run(config, capture)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fatal(&error),
    }
}
