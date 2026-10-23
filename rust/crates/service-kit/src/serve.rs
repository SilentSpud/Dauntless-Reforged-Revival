//! Listening and stopping, the same for every service (MIGRATION_PLAN.md M5, M10, X7).
//!
//! A failed bind is a fatal error, not a quiet exit. A stop request is `shutdown` on a line of
//! stdin (how the launcher and the server kits stop a service), Ctrl+C / Ctrl+Break, or SIGTERM.
//! Requests still running then get `drain` to finish before their connections are closed.

use std::future::{Future, IntoFuture};
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use tokio::io::AsyncBufReadExt;
use tokio::net::{TcpListener, TcpStream};

/// Both ends of a connection: the client's address and the address it reached, which tells a
/// loopback call from one through a LAN or VPN address on a `0.0.0.0` bind (M10).
#[derive(Debug, Clone, Copy)]
pub struct ConnectionInfo {
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

impl Connected<IncomingStream<'_, TcpListener>> for ConnectionInfo {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        let peer = *stream.remote_addr();
        let local = stream.io().local_addr().unwrap_or(peer);
        Self { peer, local }
    }
}

impl Connected<IncomingStream<'_, NoDelay>> for ConnectionInfo {
    fn connect_info(stream: IncomingStream<'_, NoDelay>) -> Self {
        let peer = *stream.remote_addr();
        let local = stream.io().local_addr().unwrap_or(peer);
        Self { peer, local }
    }
}

/// A listener whose connections send without Nagle's delay (`TCP_NODELAY`), as Node's HTTP server
/// sets them.
pub struct NoDelay(pub TcpListener);

impl Listener for NoDelay {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (TcpStream, SocketAddr) {
        let (stream, address) = Listener::accept(&mut self.0).await;
        if let Err(error) = stream.set_nodelay(true) {
            tracing::debug!("TCP_NODELAY: {error}");
        }
        (stream, address)
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.0.local_addr()
    }
}

/// Resolves when the service is asked to stop.
pub async fn stop_requested() {
    let stdin = async {
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if line.trim() == "shutdown" => return,
                Ok(Some(_)) => continue,
                // stdin closed or unreadable (a service started without one): wait for a signal.
                _ => std::future::pending::<()>().await,
            }
        }
    };
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => drop(signal.recv().await),
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(windows)]
    let terminate = async {
        match tokio::signal::windows::ctrl_break() {
            Ok(mut signal) => drop(signal.recv().await),
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        () = stdin => {}
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// Serves `app` until `stop` resolves, then gives running requests `drain` to finish.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    stop: impl Future<Output = ()> + Send + 'static,
    drain: Duration,
) -> std::io::Result<()> {
    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        NoDelay(listener),
        app.into_make_service_with_connect_info::<ConnectionInfo>(),
    )
    .with_graceful_shutdown(async move {
        stop.await;
        let _ = stopping_tx.send(());
    })
    .into_future();
    let deadline = async {
        if stopping_rx.await.is_ok() {
            tokio::time::sleep(drain).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        result = server => result,
        () = deadline => Ok(()),
    }
}
