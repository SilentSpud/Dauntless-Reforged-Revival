//! Start-up and the service loop (`src/server.ts`): the legacy key check, the runtime update, the
//! listener, then the hubs in the background and the watchdog once a minute.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use service_kit::linelog::Level;

use crate::config::Config;
use crate::gameservers::Director;
use crate::log;

pub async fn run(config: Config) -> Result<(), String> {
    log::set_level(config.log_level.as_deref());
    // [2026-10-08] MYSTPAX_ became MYSTICPARADOX_; the game servers inherit this environment, so a
    // stale key would silently switch a runtime feature off.
    if !config.legacy_keys.is_empty() {
        return Err(format!(
            "Rename these environment keys to the MYSTICPARADOX_ prefix: {}",
            config.legacy_keys.join(", ")
        ));
    }
    if config.log_cmds_invalid {
        log::warn(&format!(
            "GAMESERVER_LOG_CMDS must be \"Category Verbosity\" pairs separated by commas; using the default ({})",
            crate::config::DEFAULT_LOG_CMDS
        ));
    }
    if let Err(error) = crate::runtime_update::ensure_updated(&config).await {
        if config.runtime_update.required {
            return Err(error);
        }
        log::with_error(
            Level::Warn,
            "Server runtime update failed; continuing with the installed DLL",
            &error,
        );
    }
    std::fs::create_dir_all(&config.gameserver_log_dir)
        .map_err(|e| format!("{}: {e}", config.gameserver_log_dir.display()))?;
    let tables = crate::hunt::load_tables(&config.game_data_dir)?;
    let memory = crate::memory::MemoryAdmission::new(config.memory.clone());
    for problem in &config.memory_problems {
        log::warn(&format!("[MemoryGuard] {problem}"));
    }
    if memory.enabled() {
        let status = memory.status();
        log::info(&format!(
            "[MemoryGuard] on: a new hunt needs {} MB available (now {} MB)",
            status.needed_mb,
            status
                .available_mb
                .map_or_else(|| "unknown".to_owned(), |mb| mb.to_string())
        ));
    }
    if config.p2p_module {
        log::info("[P2P] player-hosted hunt module loaded");
    } else {
        log::info("[P2P] module disabled by MYSTICPARADOX_P2P_MODULE; dedicated hunts only");
    }

    let address = format!("{}:{}", config.bind_host, config.port);
    let socket: SocketAddr = tokio::net::lookup_host(&address)
        .await
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| format!("Could not listen on {address}: not an address"))?;
    let listener = tokio::net::TcpListener::bind(socket)
        .await
        .map_err(|e| format!("Could not listen on {address}: {e}"))?;
    let director = Director::new(config, tables, memory);
    let app = crate::app::router(director.clone());

    let booting = director.clone();
    tokio::spawn(async move {
        booting.startup().await;
        booting.set_persistent_hubs_ready(true);
    });
    let watchdog = director.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(60));
        every.tick().await;
        loop {
            every.tick().await;
            // A pass that fails never takes matchmaking down with it.
            let pass = watchdog.clone();
            if tokio::spawn(async move { pass.run_watchdog().await }).await.is_err() {
                log::error("Gameserver watchdog pass failed");
            }
        }
    });
    log::info(&format!("Mystic Paradox DeployServer on {address}"));
    log::info("Clear Skies, Slayer.");

    let served = service_kit::serve::serve(
        listener,
        app,
        service_kit::serve::stop_requested(),
        Duration::from_secs(5),
    )
    .await;
    director.dispose();
    served.map_err(|e| e.to_string())
}

/// A started Director for the tests: its router and the Director itself.
pub fn prepare(config: Config, tables: crate::hunt::Tables) -> (axum::Router, Arc<Director>) {
    let memory = crate::memory::MemoryAdmission::new(config.memory.clone());
    let director = Director::new(config, tables, memory);
    (crate::app::router(director.clone()), director)
}
