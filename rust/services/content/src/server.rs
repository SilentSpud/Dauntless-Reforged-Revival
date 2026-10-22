//! `content serve` (`src/server.ts`): the settings, the runtime store and the news, then one listener
//! per bind address. There is no game folder to check at start: a runtime that is missing or does not
//! check out is logged, and players are told the host has not published one. A download whose client
//! stopped reading for two minutes is dropped, which frees its slot; at most 512 connections per
//! listener.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tower::ServiceExt;

use crate::app::{App, AppState, Deps};
use crate::auth::{AuthCache, backend_lookup};
use crate::branding::Branding;
use crate::config::Config;
use crate::limits::StreamLimiter;
use crate::log;
use crate::news::News;
use crate::runtime::RuntimeStore;

const IDLE: Duration = Duration::from_secs(120);
const HEADERS_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CONNECTIONS: usize = 512;

/// Everything up to listening: the runtime store (checked once now) and the app.
pub fn prepare(config: &Config) -> AppState {
    let clock: crate::auth::Clock = Arc::new(crate::now_ms);
    let runtime = Arc::new(RuntimeStore::new(config.runtime_dir.clone(), clock.clone()));
    let first = runtime.get();
    if let Some(reason) = first.reason() {
        log::warn(
            "no runtime is offered to players yet; the launcher shows them that the host has to publish one",
            json!({"dir": config.runtime_dir.as_ref().map(|d| d.display().to_string()), "reason": reason}),
        );
    }
    App::new(Deps {
        runtime,
        auth: AuthCache::new(
            backend_lookup(&config.backend_url, Duration::from_secs(5)),
            Duration::from_secs(config.auth_cache_seconds),
        ),
        limiter: StreamLimiter::new(config.max_streams_per_account, config.max_streams_total),
        branding: Branding::new(config.branding_dir.clone(), clock.clone()),
        news: Arc::new(News::new(config.news_file.clone(), config.news_url.clone(), clock)),
    })
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
    let app = prepare(&config);
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut servers = Vec::new();
    for host in &config.bind_hosts {
        let ip = Config::bind_address(host)
            .ok_or_else(|| format!("Could not listen on {host}:{}: not an address", config.port))?;
        let listener = TcpListener::bind(SocketAddr::new(ip, config.port))
            .await
            .map_err(|e| format!("Could not listen on {host}:{}: {e}", config.port))?;
        servers.push(tokio::spawn(serve(listener, app.clone(), stop_rx.clone())));
    }
    log::info(
        "content server ready",
        json!({
            "listen": config.bind_hosts.iter().map(|h| format!("{h}:{}", config.port)).collect::<Vec<_>>(),
            "backend": config.backend_url,
            "runtime": config.runtime_dir.as_ref().map(|d| d.display().to_string()),
            "branding": config.branding_dir.as_ref().map(|d| d.display().to_string()),
            "news": config.news_file.as_ref().map(|d| d.display().to_string()).or_else(|| config.news_url.clone()),
            "maxStreamsPerAccount": config.max_streams_per_account,
            "maxStreamsTotal": config.max_streams_total,
        }),
    );
    // Ctrl+C, SIGTERM, Ctrl+Break (Node's SIGBREAK: a console window closed on Windows) or
    // "shutdown" on stdin
    service_kit::serve::stop_requested().await;
    log::info("stopping", json!({"signal": "stop"}));
    let _ = stop_tx.send(true);
    for server in servers {
        let _ = server.await;
    }
    Ok(())
}
