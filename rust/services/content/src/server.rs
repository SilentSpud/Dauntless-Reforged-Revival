//! `content serve` (`src/server.ts`): the settings, the manifest and the startup check of the game
//! folder, then one listener per bind address. A download whose client stopped reading for two
//! minutes is dropped, which frees its slot; at most 512 connections per listener.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tower::ServiceExt;

use crate::app::{App, AppState, Deps};
use crate::auth::{AuthCache, metagame_lookup};
use crate::branding::Branding;
use crate::config::Config;
use crate::gamedir::{self, FileState};
use crate::limits::StreamLimiter;
use crate::log;
use crate::manifest::{self, EXPECTED_BUILD};
use crate::news::News;

const IDLE: Duration = Duration::from_secs(120);
const HEADERS_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CONNECTIONS: usize = 512;

fn state_json(state: Option<&FileState>) -> serde_json::Value {
    match state {
        Some(FileState::SizeMismatch { actual }) => json!({"status": "size_mismatch", "actual": actual}),
        Some(other) => json!({"status": other.status()}),
        None => serde_json::Value::Null,
    }
}

/// Everything up to listening: the manifest, the game folder check and the app. The reason it
/// can't start is an error.
pub fn prepare(config: &Config) -> Result<AppState, String> {
    let shown = config
        .manifest_path
        .as_ref()
        .map_or_else(|| "built-in".to_owned(), |p| p.display().to_string());
    let manifest = manifest::load(config.manifest_path.as_deref()).map_err(|e| format!("Manifest {shown}: {e}"))?;
    if manifest.build != EXPECTED_BUILD {
        log::warn(
            "manifest is for a different build than this server was written for",
            json!({"build": manifest.build}),
        );
    }
    // Startup check: every manifest file present with the right size. Files that fail are not served.
    let report = gamedir::check(&config.game_dir, &manifest);
    log::info(
        "game folder checked (sizes only; content verify hashes everything)",
        json!({
            "gameDir": config.game_dir.display().to_string(),
            "build": manifest.build,
            "files": manifest.files.len(),
            "ok": report.ok,
            "missing": report.missing.len(),
            "mismatched": report.mismatched.len(),
            "bytesOk": report.bytes_ok,
            "totalBytes": manifest.total_bytes,
        }),
    );
    if report.ok == 0 {
        return Err(format!(
            "None of the {} manifest files is in {}. Is CONTENT_GAME_DIR the folder that contains Archon\\?",
            manifest.files.len(),
            config.game_dir.display()
        ));
    }
    for path in report.missing.iter().take(20) {
        log::warn("missing game file (not served)", json!({"path": path}));
    }
    for path in report.mismatched.iter().take(20) {
        log::warn(
            "game file has the wrong size or is not a regular file (not served)",
            json!({"path": path, "state": state_json(report.states.get(path))}),
        );
    }
    if report.missing.len() + report.mismatched.len() > 40 {
        log::warn(
            "more problems than listed; run content verify for the full list",
            json!({}),
        );
    }
    let clock: crate::auth::Clock = Arc::new(crate::now_ms);
    Ok(App::new(Deps {
        download_base_url: config.download_base_url.clone(),
        index: manifest::index(&manifest),
        manifest,
        game_dir: config.game_dir.clone(),
        states: Mutex::new(report.states),
        auth: AuthCache::new(
            metagame_lookup(&config.metagame_url, Duration::from_secs(5)),
            Duration::from_secs(config.auth_cache_seconds),
        ),
        limiter: StreamLimiter::new(config.max_streams_per_account, config.max_streams_total),
        branding: Branding::new(config.branding_dir.clone(), clock.clone()),
        news: Arc::new(News::new(config.news_file.clone(), config.news_url.clone(), clock)),
    }))
}

/// A connection that ends after two minutes in which nothing could be read or written (Node's
/// `server.setTimeout(120000)`).
struct Idle {
    inner: TcpStream,
    deadline: Pin<Box<tokio::time::Sleep>>,
}

impl Idle {
    fn new(inner: TcpStream) -> Self {
        Self {
            inner,
            deadline: Box::pin(tokio::time::sleep(IDLE)),
        }
    }

    fn progressed(&mut self) {
        self.deadline.as_mut().reset(tokio::time::Instant::now() + IDLE);
    }

    fn timed_out(&mut self, cx: &mut Context<'_>) -> bool {
        self.deadline.as_mut().poll(cx).is_ready()
    }
}

impl AsyncRead for Idle {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                self.progressed();
                Poll::Ready(result)
            }
            Poll::Pending if self.timed_out(cx) => Poll::Ready(Err(std::io::ErrorKind::TimedOut.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Idle {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, data) {
            Poll::Ready(result) => {
                self.progressed();
                Poll::Ready(result)
            }
            Poll::Pending if self.timed_out(cx) => Poll::Ready(Err(std::io::ErrorKind::TimedOut.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

use std::future::Future;

/// Serves `app` on `listener` until `stop` resolves.
pub async fn serve(listener: TcpListener, app: AppState, mut stop: tokio::sync::watch::Receiver<bool>) {
    let router = crate::app::router(app);
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stop.wait_for(|s| *s) => break,
        };
        let Ok((stream, peer)) = accepted else {
            continue;
        };
        // Past the limit a connection is closed at once, as Node's maxConnections does
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let _ = stream.set_nodelay(true);
        let router = router.clone();
        let mut stopping = stop.clone();
        connections.spawn(async move {
            let _slot = slot;
            let service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                let mut request = request.map(axum::body::Body::new);
                request
                    .extensions_mut()
                    .insert(axum::extract::ConnectInfo::<SocketAddr>(peer));
                router.clone().oneshot(request)
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADERS_TIMEOUT)
                .serve_connection(TokioIo::new(Idle::new(stream)), service);
            tokio::pin!(connection);
            tokio::select! {
                _ = connection.as_mut() => {}
                _ = stopping.wait_for(|s| *s) => {}
            }
        });
    }
    connections.abort_all();
}

/// Starts every listener and serves until asked to stop.
pub async fn run(config: Config) -> Result<(), String> {
    let app = prepare(&config)?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut servers = Vec::new();
    for host in &config.bind_hosts {
        let address = if host == "localhost" {
            "127.0.0.1"
        } else {
            host.as_str()
        };
        let ip: std::net::IpAddr = address
            .parse()
            .map_err(|_| format!("Could not listen on {host}:{}: not an address", config.port))?;
        let listener = TcpListener::bind(SocketAddr::new(ip, config.port))
            .await
            .map_err(|e| format!("Could not listen on {host}:{}: {e}", config.port))?;
        log::info(
            &format!("Dauntless Revived content server on {host}:{}", config.port),
            json!({}),
        );
        servers.push(tokio::spawn(serve(listener, app.clone(), stop_rx.clone())));
    }
    log::info(
        "content server ready",
        json!({
            "metagame": config.metagame_url,
            "branding": config.branding_dir.as_ref().map(|d| d.display().to_string()),
            "news": config.news_file.as_ref().map(|d| d.display().to_string()),
            "maxStreamsPerAccount": config.max_streams_per_account,
            "maxStreamsTotal": config.max_streams_total,
        }),
    );
    service_kit::serve::stop_requested().await;
    log::info("stopping", json!({"signal": "stop"}));
    let _ = stop_tx.send(true);
    for server in servers {
        let _ = server.await;
    }
    Ok(())
}
