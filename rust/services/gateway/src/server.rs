//! Startup and shutdown (`src/server.ts`): load the existing certificate and key (never make new
//! ones here: their fingerprint is in every invite), check them, listen, serve until asked to stop.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde_json::json;
use service_kit::linelog;
use tokio::net::TcpListener;

use crate::cert::{self, iso};
use crate::config::Config;
use crate::feed::AllowlistFeed;
use crate::gateway::Gateway;

pub async fn run(config: Config) -> Result<(), String> {
    let loaded = cert::load(&config.cert_file, &config.key_file).map_err(|error| {
        format!(
            "Certificate {} / key {}: {error}",
            config.cert_file.display(),
            config.key_file.display()
        )
    })?;
    let info = loaded.info.clone();
    let now = chrono::Utc::now();
    if info.not_after <= now || info.not_before > now {
        return Err(format!(
            "The certificate is not valid now (valid {} to {}). Make a new one with `gateway make-cert`; invites then need the new fingerprint.",
            iso(info.not_before),
            iso(info.not_after)
        ));
    }
    if info.not_after - now < chrono::Duration::days(30) {
        linelog::warn(
            "the certificate expires within 30 days; make a new one and send new invites",
            json!({ "notAfter": iso(info.not_after) }),
        );
    }
    let tls = cert::server_config(&loaded)?;

    let feed = config.allowlist.as_ref().map(AllowlistFeed::new);
    if feed.is_none() {
        linelog::warn(
            "allowlist feed is off (GATEWAY_ALLOWLIST=0): the game ports open for nobody unless the firewall is managed another way",
            json!({}),
        );
    }

    let address = SocketAddr::new(
        config.bind_host.parse::<IpAddr>().map_err(|e| e.to_string())?,
        config.port,
    );
    let listener = TcpListener::bind(address)
        .await
        .map_err(|e| format!("Could not listen on {}:{}: {e}", config.bind_host, config.port))?;

    let rate = &config.limits.rate;
    let spec = |s: &crate::ratelimit::BucketSpec| json!({ "burst": s.burst, "perMinute": s.per_minute });
    linelog::info(
        "gateway ready",
        json!({
            "listen": format!("{}:{}", config.bind_host, config.port),
            "fingerprint": info.fingerprint,
            "certificateNames": info.subject_alt_name,
            "certificateValidUntil": iso(info.not_after),
            "metagame": config.metagame.authority(),
            "content": config.content.authority(),
            "websocket": config.ws.authority(),
            "allowlistFeed": config.allowlist.as_ref().map_or("off", |a| a.url.as_str()),
            "maxBodyBytes": config.limits.max_body_bytes,
            "maxConnections": config.limits.max_connections,
            "maxConnectionsPerIp": config.limits.max_connections_per_ip,
            "rate": {
                "general": spec(&rate.general),
                "content": spec(&rate.content),
                "register": spec(&rate.register),
                "token": spec(&rate.token),
                "ws": spec(&rate.ws),
                "connect": spec(&rate.connect),
            },
        }),
    );

    let grace = config.timeouts.shutdown_grace;
    let gateway = Gateway::new(config, tls, info, feed);
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let served = tokio::spawn(gateway.serve(listener, stopping));
    service_kit::serve::stop_requested().await;
    linelog::info(
        "stopping",
        json!({ "signal": "stop", "graceMs": grace.as_millis() as u64 }),
    );
    let _ = stop.send(true);
    // Never hang on the way out
    let _ = tokio::time::timeout(grace + Duration::from_secs(5), served).await;
    linelog::info("stopped", json!({}));
    Ok(())
}
