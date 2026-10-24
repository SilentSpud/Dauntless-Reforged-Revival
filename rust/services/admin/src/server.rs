//! `admin serve`: the timeouts file, the schema, a listener per bind address, and the sweep that
//! lifts timeouts as they run out.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::net::TcpListener;

use crate::app::{AppState, router};
use crate::config::Config;
use crate::log;
use crate::schema::{Admin, build};
use crate::timeouts::Timeouts;
use crate::upstream::{Clock, Upstream};

/// How often due timeouts are lifted: a timeout ends at most this late.
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);
const DRAIN: Duration = Duration::from_secs(5);

pub fn prepare(config: &Config, clock: Clock) -> Result<(AppState, Arc<Admin>), String> {
    let admin = Arc::new(Admin {
        upstream: Upstream::new(
            &config.backend_url,
            &config.director_url,
            &config.admin_key,
            &config.operator_secret,
            clock.clone(),
        ),
        timeouts: Timeouts::load(&config.timeouts_file)?,
        max_hunt_servers: config.max_hunt_servers,
        server_mode: config.server_mode,
        clock,
    });
    let state = AppState {
        schema: build(admin.clone()),
        api_key: config.api_key.as_str().into(),
    };
    Ok((state, admin))
}

pub async fn run(config: Config) -> Result<(), String> {
    let (state, admin) = prepare(&config, Arc::new(crate::now_ms))?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut servers = Vec::new();
    for host in &config.bind_hosts {
        let ip = Config::bind_address(host)
            .ok_or_else(|| format!("Could not listen on {host}:{}: not an address", config.port))?;
        let listener = TcpListener::bind(SocketAddr::new(ip, config.port))
            .await
            .map_err(|e| format!("Could not listen on {host}:{}: {e}", config.port))?;
        let mut stop = stop_rx.clone();
        let app = router(state.clone());
        servers.push(tokio::spawn(async move {
            let stopped = async move {
                let _ = stop.wait_for(|s| *s).await;
            };
            service_kit::serve::serve(listener, app, stopped, DRAIN).await
        }));
    }
    let sweeper = tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        loop {
            tick.tick().await;
            admin.sweep().await;
        }
    });
    log::info(
        "admin API ready",
        json!({
            "listen": config.bind_hosts.iter().map(|h| format!("{h}:{}", config.port)).collect::<Vec<_>>(),
            "backend": config.backend_url,
            "director": config.director_url,
            "timeouts": config.timeouts_file.display().to_string(),
        }),
    );
    service_kit::serve::stop_requested().await;
    log::info("stopping", json!({"signal": "stop"}));
    sweeper.abort();
    let _ = stop_tx.send(true);
    for server in servers {
        if let Ok(Err(error)) = server.await {
            log::warn("listener stopped with an error", json!({"error": error.to_string()}));
        }
    }
    Ok(())
}
