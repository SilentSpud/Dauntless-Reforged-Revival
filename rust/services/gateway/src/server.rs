//! Startup and shutdown (`src/server.ts`): in public mode, load the existing certificate and key
//! (never make new ones here: their fingerprint is in every invite) and check them; set up the
//! traffic capture when it is configured; listen, serve until asked to stop.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, json};
use service_kit::env::Env;
use service_kit::linelog;
use tokio::net::TcpListener;

use crate::capture::{CaptureWriter, load_capture_options};
use crate::cert::{self, iso};
use crate::config::Config;
use crate::feed::AllowlistFeed;
use crate::gateway::{Gateway, Tls};

/// The certificate and key, checked: the key belongs to the certificate, which is valid now. A
/// warning when it expires within 30 days.
pub fn load_tls(config: &Config) -> Result<Tls, String> {
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
    Ok(Tls {
        config: cert::server_config(&loaded)?,
        info,
    })
}

/// The capture writer when `CAPTURE_DIR` is set.
pub fn load_capture(env: &Env) -> Result<Option<CaptureWriter>, String> {
    let Some(options) = load_capture_options(env, "gateway").map_err(|e| format!("Traffic capture: {e}"))? else {
        return Ok(None);
    };
    let (dir, retention) = (options.dir.display().to_string(), options.retention_days);
    let writer = CaptureWriter::new(
        options,
        Box::new(|message| linelog::warn(message, json!({}))),
        Arc::new(crate::now_ms),
    )
    .map_err(|e| format!("Traffic capture: {e}"))?;
    linelog::warn(
        "traffic capture is on: every request and answer is recorded, redacted, for the shadow replay (personal data; see docs/security/gateway.md)",
        json!({ "dir": dir, "retentionDays": retention }),
    );
    Ok(Some(writer))
}

pub async fn run(config: Config, capture: Option<CaptureWriter>) -> Result<(), String> {
    let tls = if config.tls { Some(load_tls(&config)?) } else { None };

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
    let mut ready = Map::new();
    ready.insert("listen".into(), json!(format!("{}:{}", config.bind_host, config.port)));
    ready.insert(
        "mode".into(),
        json!(if config.tls {
            "public (TLS)"
        } else {
            "private (plain HTTP inside the tailnet)"
        }),
    );
    if let Some(tls) = &tls {
        ready.insert("fingerprint".into(), json!(tls.info.fingerprint));
        ready.insert("certificateNames".into(), json!(tls.info.subject_alt_name));
        ready.insert("certificateValidUntil".into(), json!(iso(tls.info.not_after)));
    }
    let fields = json!({
        "backend": config.backend.authority(),
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
    });
    if let serde_json::Value::Object(fields) = fields {
        ready.extend(fields);
    }

    let grace = config.timeouts.shutdown_grace;
    let gateway = Gateway::new(config, tls, feed, capture)?;
    linelog::info("gateway ready", serde_json::Value::Object(ready));
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
