//! Pooled connections to the invite's host for hyper-util's client: TLS pinned to the invite's
//! certificate in public mode, plain TCP in private mode. The relay (L22) and the downloader (L23)
//! keep connections alive through it; every new connection is pinned again.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use hyper::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use super::pinned::{ConnectError, PinnedTarget, pinned_connect};

/// Where every connection goes, whatever URI the request names.
#[derive(Debug, Clone)]
pub enum Host {
    Pinned(PinnedTarget),
    Plain { host: String, port: u16 },
}

pub enum HostIo {
    Pinned(Box<TokioIo<TlsStream<TcpStream>>>),
    Plain(TokioIo<TcpStream>),
}

impl Connection for HostIo {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl hyper::rt::Read for HostIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Pinned(io) => Pin::new(io.as_mut()).poll_read(cx, buf),
            Self::Plain(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl hyper::rt::Write for HostIo {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Pinned(io) => Pin::new(io.as_mut()).poll_write(cx, data),
            Self::Plain(io) => Pin::new(io).poll_write(cx, data),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Pinned(io) => Pin::new(io.as_mut()).poll_flush(cx),
            Self::Plain(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Pinned(io) => Pin::new(io.as_mut()).poll_shutdown(cx),
            Self::Plain(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

#[derive(Clone)]
pub struct HostConnector {
    pub host: Host,
    pub timeout: Duration,
    /// Connections opened so far
    pub opened: Arc<AtomicU64>,
}

impl HostConnector {
    pub fn new(host: Host, timeout: Duration) -> Self {
        Self {
            host,
            timeout,
            opened: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl tower_service::Service<Uri> for HostConnector {
    type Response = HostIo;
    type Error = ConnectError;
    type Future = Pin<Box<dyn Future<Output = Result<HostIo, ConnectError>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let (host, timeout, opened) = (self.host.clone(), self.timeout, self.opened.clone());
        Box::pin(async move {
            let io = match host {
                Host::Pinned(target) => HostIo::Pinned(Box::new(TokioIo::new(pinned_connect(&target, timeout).await?))),
                Host::Plain { host, port } => {
                    let tcp = tokio::time::timeout(timeout, TcpStream::connect((host.as_str(), port)))
                        .await
                        .map_err(|_| ConnectError::Timeout)?
                        .map_err(ConnectError::Io)?;
                    let _ = tcp.set_nodelay(true);
                    HostIo::Plain(TokioIo::new(tcp))
                }
            };
            opened.fetch_add(1, Ordering::Relaxed);
            Ok(io)
        })
    }
}

/// The pin error somewhere in an error's sources, with the fingerprint the server showed.
pub fn pin_error(error: &(dyn std::error::Error + 'static)) -> Option<Option<String>> {
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(ConnectError::Pin { actual }) = error.downcast_ref::<ConnectError>() {
            return Some(actual.clone());
        }
        source = error.source();
    }
    None
}
