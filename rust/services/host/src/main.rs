//! `reforged-host`: the host kit's command line (MIGRATION_PLAN.md RD3, RH rows).

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = host::cli::Cli::parse();
    match host::run(&cli) {
        Ok(code) => code,
        Err(error) => {
            // As the Node kit's fail().
            eprint!("\nError: {error}\n");
            ExitCode::FAILURE
        }
    }
}
