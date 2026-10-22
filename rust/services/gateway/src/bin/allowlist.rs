//! The `allowlist` binary (`npm run start:allowlist`, the Linux kit's `allowlist.mjs`): keeps the
//! game ports open to players who logged in. `--close-ports` closes them once and exits, for stop
//! scripts (a supervisor that ends the helper with TerminateProcess never lets it close them
//! itself). Without `--env-file`, a `.env` in the working folder is read if there is one.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use gateway::allowlist::config::AllowlistConfig;
use gateway::allowlist::firewall::{Backend, plan};
use gateway::allowlist::helper::{AuditLog, Helper, applier};
use serde_json::json;
use service_kit::env::Env;
use service_kit::linelog;

#[derive(Parser)]
#[command(name = "allowlist", version, about = "Dauntless Revived allowlist helper")]
struct Cli {
    #[arg(long)]
    env_file: Option<PathBuf>,
    /// Close the game ports once and exit.
    #[arg(long)]
    close_ports: bool,
}

fn fatal(message: &str) -> ExitCode {
    linelog::error(message, json!({}));
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let env_file = cli
        .env_file
        .or_else(|| Some(PathBuf::from(".env")).filter(|p| p.is_file()));
    let config = Env::load(env_file.as_deref())
        .map_err(|e| e.to_string())
        .and_then(|env| AllowlistConfig::from_env(&env));
    let config = match config {
        Ok(config) => config,
        Err(error) => return fatal(&format!("Configuration: {error}")),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return fatal(&error.to_string()),
    };
    runtime.block_on(run(config, cli.close_ports))
}

async fn run(config: AllowlistConfig, close_ports: bool) -> ExitCode {
    let audit = Arc::new(AuditLog::new(Some(config.audit_log.clone())));
    let backend = if config.dry_run {
        linelog::warn(
            "DRY RUN: the firewall is never changed; every change is only written to the audit log",
            json!({}),
        );
        Backend::dry_run(config.nft.clone())
    } else {
        match Backend::for_platform(config.nft.clone()) {
            Ok(backend) => backend,
            Err(error) => return fatal(&error),
        }
    };
    #[cfg(windows)]
    if !config.dry_run && !gateway::allowlist::windows::is_elevated() {
        return fatal(
            "ALLOWLIST_DRY_RUN=0 needs administrator rights: run it as a scheduled task with \"Run with highest privileges\"",
        );
    }

    if close_ports {
        let backend = Arc::new(backend);
        let apply = applier(backend, audit.clone(), config.ports.clone(), config.dry_run);
        if let Err(error) = plan(&[], &config.ports) {
            return fatal(&error);
        }
        return match apply(Vec::new()).await {
            Ok(()) => {
                audit.write("closed_ports", json!({ "dryRun": config.dry_run }));
                ExitCode::SUCCESS
            }
            Err(error) => fatal(&format!("Could not close the game ports: {error}")),
        };
    }

    let helper = Helper::new(config.clone(), backend, audit, Arc::new(gateway::now_ms));
    let running = match helper.start().await {
        Ok(running) => running,
        Err(error) => {
            return fatal(&format!(
                "Could not listen on {}:{}: {error}",
                config.bind_host, config.port
            ));
        }
    };
    linelog::info(
        "allowlist helper ready",
        json!({
            "listen": format!("{}:{}", config.bind_host, running.port),
            "dryRun": config.dry_run,
            "ports": config.ports,
            "ttlSeconds": config.ttl.as_secs(),
            "minIntervalMs": config.min_interval.as_millis() as u64,
            "maxEntries": config.max_entries,
            "allowPrivate": config.allow_private,
            "auditLog": config.audit_log.display().to_string(),
            "stateFile": config.state_file.display().to_string(),
        }),
    );
    service_kit::serve::stop_requested().await;
    linelog::info("stopping: closing the game ports", json!({ "signal": "stop" }));
    if tokio::time::timeout(Duration::from_secs(90), helper.close(running))
        .await
        .is_err()
    {
        return fatal("Shutdown took longer than 90 s");
    }
    ExitCode::SUCCESS
}
