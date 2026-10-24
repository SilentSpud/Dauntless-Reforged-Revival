//! The `admin` binary: `admin serve --env-file <file>` runs the admin API; `serve` is the default.
//! `admin schema` prints the GraphQL schema (`schema.graphql`).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use service_kit::env::Env;

#[derive(Parser)]
#[command(name = "admin", version, about = "Dauntless Reforged Revival admin API")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serves the admin API on loopback.
    Serve {
        #[arg(long)]
        env_file: Option<PathBuf>,
    },
    /// Prints the GraphQL schema.
    Schema,
}

fn fail(error: &str) -> ExitCode {
    admin::log::error(&format!("Configuration: {error}"), serde_json::json!({}));
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let file = match Cli::parse().command.unwrap_or(Command::Serve { env_file: None }) {
        Command::Schema => {
            print!("{}", admin::schema::sdl());
            return ExitCode::SUCCESS;
        }
        Command::Serve { env_file } => env_file,
    };
    let env = match Env::load(file.as_deref()) {
        Ok(env) => env,
        Err(error) => return fail(&error.to_string()),
    };
    let config = match admin::config::Config::from_env(&env) {
        Ok(config) => config,
        Err(error) => return fail(&error),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return fail(&error.to_string()),
    };
    match runtime.block_on(admin::server::run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error),
    }
}
