//! What every reverse proxy in the repository does the same way (MIGRATION_PLAN.md X5): the gateway
//! (S2) and the launcher relay (L22) are mirror images of each other.
//!
//! - Hop-by-hop headers (RFC 9110 7.6.1), and every header the `Connection` header names, never
//!   cross a proxy in either direction ([`end_to_end`]).
//! - A connection that moves no bytes for a while is cut ([`Idle`]), a stalled download included.
//! - An upgraded connection is joined to the upstream byte for byte until either side ends or both
//!   are silent too long ([`tunnel`]).

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use http::HeaderMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

/// RFC 9110 7.6.1, and the old spellings that still go around.
pub const HOP_BY_HOP: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

pub fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.contains(&name)
}

/// The header names a `Connection` header lists (every value joined, lower case).
pub fn connection_tokens(headers: &HeaderMap) -> HashSet<String> {
    headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The headers that may cross: no hop-by-hop header, none the `Connection` header names, none
/// `drop` says, in their order.
pub fn end_to_end(headers: &HeaderMap, drop: impl Fn(&str) -> bool) -> HeaderMap {
    let named = connection_tokens(headers);
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        let lower = name.as_str();
        if is_hop_by_hop(lower) || named.contains(lower) || drop(lower) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// A connection that ends after `limit` in which nothing could be read or written (Node's
/// `socket.setTimeout`).
pub struct Idle<S> {
    inner: S,
    limit: Duration,
    deadline: Pin<Box<tokio::time::Sleep>>,
}

impl<S> Idle<S> {
    pub fn new(inner: S, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            deadline: Box::pin(tokio::time::sleep(limit)),
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    fn progressed(&mut self) {
        let next = tokio::time::Instant::now() + self.limit;
        self.deadline.as_mut().reset(next);
    }

    fn timed_out(&mut self, cx: &mut Context<'_>) -> bool {
        self.deadline.as_mut().poll(cx).is_ready()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Idle<S> {
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

impl<S: AsyncWrite + Unpin> AsyncWrite for Idle<S> {
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

/// Joins two upgraded connections until either side ends or one is silent for `idle`; the bytes
/// that went each way (client to upstream, upstream to client). Either side ending ends both, as
/// the Node gateway destroys both sockets on either close.
pub async fn tunnel<A, B>(client: A, upstream: B, idle: Duration) -> (u64, u64)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut client_read, mut client_write) = tokio::io::split(Idle::new(client, idle));
    let (mut upstream_read, mut upstream_write) = tokio::io::split(Idle::new(upstream, idle));
    let (mut up, mut down) = (0u64, 0u64);
    let mut up_buf = vec![0u8; 16 * 1024];
    let mut down_buf = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            read = client_read.read(&mut up_buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if upstream_write.write_all(&up_buf[..n]).await.is_err() {
                        break;
                    }
                    up += n as u64;
                }
            },
            read = upstream_read.read(&mut down_buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client_write.write_all(&down_buf[..n]).await.is_err() {
                        break;
                    }
                    down += n as u64;
                }
            },
        }
    }
    let _ = client_write.shutdown().await;
    let _ = upstream_write.shutdown().await;
    (up, down)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn hop_by_hop_and_connection_named_headers_never_cross() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive, X-Secret-Thing"));
        headers.insert("x-secret-thing", HeaderValue::from_static("1"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("x-powered-by", HeaderValue::from_static("Express"));
        headers.append("set-cookie", HeaderValue::from_static("a=1"));
        headers.append("set-cookie", HeaderValue::from_static("b=2"));
        headers.insert("content-type", HeaderValue::from_static("text/plain"));
        let out = end_to_end(&headers, |n| n == "x-powered-by");
        let names: Vec<&str> = out.keys().map(|k| k.as_str()).collect();
        assert_eq!(names, ["set-cookie", "content-type"]);
        assert_eq!(out.get_all("set-cookie").iter().count(), 2);
    }

    #[tokio::test]
    async fn a_tunnel_carries_both_ways_and_ends_with_either_side() {
        let (client, mut client_far) = tokio::io::duplex(1024);
        let (upstream, mut upstream_far) = tokio::io::duplex(1024);
        let joined = tokio::spawn(tunnel(client, upstream, Duration::from_secs(5)));
        client_far.write_all(b"hello").await.unwrap();
        let mut got = [0u8; 5];
        upstream_far.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello");
        upstream_far.write_all(b"hi").await.unwrap();
        let mut back = [0u8; 2];
        client_far.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hi");
        drop(upstream_far);
        assert_eq!(joined.await.unwrap(), (5, 2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_tunnel_is_cut() {
        let (client, _client_far) = tokio::io::duplex(1024);
        let (upstream, _upstream_far) = tokio::io::duplex(1024);
        assert_eq!(tunnel(client, upstream, Duration::from_secs(300)).await, (0, 0));
    }
}
