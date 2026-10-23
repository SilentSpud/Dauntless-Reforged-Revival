//! The gateway itself (`src/gateway.ts`): TLS on the public port (plain HTTP in private mode), the
//! policy and limits on every request, then a reverse proxy to the backend, the content server or
//! the chat listener, with the traffic capture when it is on.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{CONNECTION, CONTENT_LENGTH, HeaderName, HeaderValue, TRANSFER_ENCODING, UPGRADE};
use http::{HeaderMap, Request, Response, StatusCode};
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use serde_json::{Map, Value, json};
use service_kit::linelog::{self, Throttle};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::{Instant, Sleep};

use crate::capture::{
    BodyTap, Capture, CaptureWriter, ChatTap, ConnectionId, Direction, SharedTap, redact_capture_body,
    redact_capture_headers, redact_capture_target,
};
use crate::cert::CertInfo;
use crate::config::{Config, Endpoint};
use crate::feed::AllowlistFeed;
use crate::guard::{Guard, Phases};
use crate::ip::{peer_address, rate_limit_key};
use crate::policy::{Decision, FeedKind, LimitClass, Upstream, classify, has_bearer};
use crate::ratelimit::{RateLimiter, Take};
use crate::redact::{redact_target, redact_user_agent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Client-supplied address and origin claims are dropped; the gateway sets its own.
const CLIENT_CLAIMS: [&str; 12] = [
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-prefix",
    "forwarded",
    "x-real-ip",
    "x-client-ip",
    "true-client-ip",
    "cf-connecting-ip",
    "x-reforged-gateway",
    // The gateway already answered any 100-continue itself
    "expect",
];

const RESPONSE_DROP: [&str; 2] = ["x-powered-by", "x-reforged-gateway"];

/// `ForwardHeaders`; `proto` is `https`, or `http` in private mode.
pub fn forward_headers(incoming: &HeaderMap, ip: &str, secret: &str, upgrade: bool, proto: &'static str) -> HeaderMap {
    let mut out = proxy::end_to_end(incoming, |name| CLIENT_CLAIMS.contains(&name));
    // Overwritten, never appended: the backend trusts exactly one address, the real peer
    let value = |text: &str| HeaderValue::from_str(text).unwrap_or_else(|_| HeaderValue::from_static(""));
    out.insert("x-forwarded-for", value(ip));
    out.insert("x-forwarded-proto", HeaderValue::from_static(proto));
    out.insert("x-reforged-gateway", value(secret));
    if upgrade {
        out.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
        out.insert(UPGRADE, HeaderValue::from_static("websocket"));
    }
    out
}

/// `ResponseHeaders`.
fn response_headers(incoming: &HeaderMap, close: bool) -> HeaderMap {
    let mut out = proxy::end_to_end(incoming, |name| RESPONSE_DROP.contains(&name));
    if close {
        out.insert(CONNECTION, HeaderValue::from_static("close"));
    }
    out
}

/// Header text as Node reads it (latin1).
fn latin1(value: &HeaderValue) -> String {
    value.as_bytes().iter().map(|b| char::from(*b)).collect()
}

/// Node's `upgrade` event: an `Upgrade` header and `Connection: upgrade`.
fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(UPGRADE) && proxy::connection_tokens(headers).contains("upgrade")
}

/// Node's error code for a failed upstream exchange (`upstream_ECONNREFUSED` in the log).
fn error_code(error: &(dyn std::error::Error + 'static)) -> &'static str {
    let mut source = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            return match io.kind() {
                std::io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
                std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof => "ECONNRESET",
                std::io::ErrorKind::TimedOut => "ETIMEDOUT",
                std::io::ErrorKind::AddrNotAvailable => "EADDRNOTAVAIL",
                _ => "error",
            };
        }
        if let Some(hyper_error) = current.downcast_ref::<hyper::Error>()
            && (hyper_error.is_incomplete_message() || hyper_error.is_closed())
        {
            return "ECONNRESET";
        }
        source = current.source();
    }
    "error"
}

/// Resolves once the flag is true (or its sender is gone).
async fn until(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|set| *set).await;
}

/// One line of the access log.
struct Access {
    ip: String,
    method: String,
    target: String,
    route: &'static str,
    status: u16,
    bytes_in: u64,
    reason: Option<String>,
    ua: Option<String>,
    started: Instant,
    /// The capture record of this request, when the capture is on
    capture: Option<Box<Record>>,
}

/// What the capture keeps of one request while it is answered (`Record`).
struct Record {
    capture: Arc<Capture>,
    conn: ConnectionId,
    target: String,
    req_headers: Value,
    req_type: Option<String>,
    req: SharedTap,
    res: BodyTap,
    res_headers: HeaderMap,
}

fn content_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

impl Record {
    /// One record per answered request: the replay sends the request again and compares the
    /// answer. Upgrades are recorded as chat instead.
    fn write(self, access: &Access, aborted: bool) {
        let max = self.capture.max_body_bytes();
        let req = self.req.lock().unwrap_or_else(PoisonError::into_inner);
        let mut fields = Map::new();
        fields.insert("kind".into(), json!("http"));
        fields.insert("conn".into(), json!(self.capture.connection(&self.conn)));
        fields.insert("seq".into(), json!(self.capture.next_seq()));
        fields.insert("ip".into(), json!(access.ip));
        fields.insert("method".into(), json!(access.method));
        fields.insert("target".into(), json!(self.target));
        fields.insert("route".into(), json!(access.route));
        fields.insert("reqHeaders".into(), self.req_headers.clone());
        fields.insert(
            "reqBody".into(),
            redact_capture_body(req.body(), self.req_type.as_deref(), req.total, max),
        );
        fields.insert("status".into(), json!(access.status));
        fields.insert("resHeaders".into(), redact_capture_headers(&self.res_headers));
        fields.insert(
            "resBody".into(),
            redact_capture_body(
                self.res.body(),
                content_type(&self.res_headers).as_deref(),
                self.res.total,
                max,
            ),
        );
        fields.insert("ms".into(), json!(access.started.elapsed().as_millis() as u64));
        if aborted {
            fields.insert("aborted".into(), json!(true));
        }
        self.capture.write(Value::Object(fields));
    }
}

impl Access {
    fn log(&self, bytes_out: u64, aborted: bool) {
        let mut fields = Map::new();
        fields.insert("ip".into(), json!(self.ip));
        fields.insert("method".into(), json!(self.method));
        fields.insert("target".into(), json!(self.target));
        fields.insert("route".into(), json!(self.route));
        fields.insert("status".into(), json!(self.status));
        fields.insert("bytesIn".into(), json!(self.bytes_in));
        fields.insert("bytesOut".into(), json!(bytes_out));
        fields.insert("ms".into(), json!(self.started.elapsed().as_millis() as u64));
        if let Some(reason) = &self.reason {
            fields.insert("reason".into(), json!(reason));
        }
        if let Some(ua) = &self.ua {
            fields.insert("ua".into(), json!(ua));
        }
        if aborted {
            fields.insert("aborted".into(), json!(true));
        }
        linelog::info("request", Value::Object(fields));
    }
}

/// What a request body did on its way upstream.
#[derive(Default)]
struct Exchange {
    bytes_in: AtomicU64,
    over_cap: AtomicBool,
    timed_out: AtomicBool,
    failed: AtomicBool,
}

/// The client's request body on its way upstream: counted, cut at the cap and at the request time.
struct RequestBody {
    inner: Incoming,
    exchange: Arc<Exchange>,
    cap: u64,
    deadline: Pin<Box<Sleep>>,
    tap: Option<SharedTap>,
}

impl Body for RequestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if let Some(tap) = &self.tap {
                        tap.lock().unwrap_or_else(PoisonError::into_inner).add(data);
                    }
                    let total =
                        self.exchange.bytes_in.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
                    if total > self.cap {
                        self.exchange.over_cap.store(true, Ordering::Relaxed);
                        return Poll::Ready(Some(Err("body too large".into())));
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.exchange.failed.store(true, Ordering::Relaxed);
                Poll::Ready(Some(Err(error.into())))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                if self.deadline.as_mut().poll(cx).is_ready() {
                    self.exchange.timed_out.store(true, Ordering::Relaxed);
                    return Poll::Ready(Some(Err("request timed out".into())));
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

enum Content {
    Full(Option<Bytes>),
    Upstream {
        body: Incoming,
        idle: Duration,
        sleep: Pin<Box<Sleep>>,
    },
}

/// Every answer: written to the access log when it is done or dropped (Node's `res.on("close")`),
/// and the connection then waits for its next request.
pub struct ResponseBody {
    content: Content,
    access: Option<Access>,
    exchange: Option<Arc<Exchange>>,
    phases: Option<Phases>,
    sent: u64,
    expected: Option<u64>,
    ended: bool,
    /// Whether the log counts these bytes (`bytesOut` is what came from an upstream)
    from_upstream: bool,
}

impl ResponseBody {
    fn full(data: Bytes) -> Self {
        let expected = Some(data.len() as u64);
        Self {
            content: Content::Full(Some(data)),
            access: None,
            exchange: None,
            phases: None,
            sent: 0,
            expected,
            ended: false,
            from_upstream: false,
        }
    }

    fn upstream(body: Incoming, idle: Duration) -> Self {
        let expected = body.size_hint().exact();
        Self {
            content: Content::Upstream {
                body,
                idle,
                sleep: Box::pin(tokio::time::sleep(idle)),
            },
            access: None,
            exchange: None,
            phases: None,
            sent: 0,
            expected,
            ended: false,
            from_upstream: true,
        }
    }

    /// An upstream's answer, collected before it is passed on.
    fn relayed(data: Bytes) -> Self {
        let mut body = Self::full(data);
        body.from_upstream = true;
        body
    }

    fn logged(mut self, access: Access, exchange: Option<Arc<Exchange>>, phases: Phases) -> Self {
        self.access = Some(access);
        self.exchange = exchange;
        self.phases = Some(phases);
        self
    }
}

impl Body for ResponseBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        let polled = match &mut this.content {
            Content::Full(data) => Poll::Ready(data.take().map(|d| Ok(Frame::data(d)))),
            Content::Upstream { body, idle, sleep } => match Pin::new(body).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    sleep.as_mut().reset(Instant::now() + *idle);
                    Poll::Ready(Some(Ok(frame)))
                }
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error.into()))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending if sleep.as_mut().poll(cx).is_ready() => {
                    Poll::Ready(Some(Err("upstream timed out".into())))
                }
                Poll::Pending => Poll::Pending,
            },
        };
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.sent += data.len() as u64;
                    if let Some(record) = this.access.as_mut().and_then(|a| a.capture.as_deref_mut()) {
                        record.res.add(data);
                    }
                }
            }
            Poll::Ready(None) => this.ended = true,
            _ => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        match &self.content {
            Content::Full(data) => data.is_none(),
            Content::Upstream { body, .. } => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.content {
            Content::Full(Some(data)) => SizeHint::with_exact(data.len() as u64),
            Content::Full(None) => SizeHint::with_exact(0),
            Content::Upstream { body, .. } => body.size_hint(),
        }
    }
}

impl Drop for ResponseBody {
    fn drop(&mut self) {
        if let Some(access) = &mut self.access {
            if let Some(exchange) = &self.exchange {
                access.bytes_in = exchange.bytes_in.load(Ordering::Relaxed);
            }
            let finished = self.ended || self.expected == Some(self.sent);
            access.log(if self.from_upstream { self.sent } else { 0 }, !finished);
            if let Some(record) = access.capture.take() {
                record.write(access, !finished);
            }
        }
        if let Some(phases) = &self.phases {
            phases.request_done();
        }
    }
}

pub type Reply = Response<ResponseBody>;

struct Limiters {
    general: RateLimiter,
    content: RateLimiter,
    register: RateLimiter,
    token: RateLimiter,
    ws: RateLimiter,
    connect: RateLimiter,
}

impl Limiters {
    fn class(&mut self, class: LimitClass) -> &mut RateLimiter {
        match class {
            LimitClass::General => &mut self.general,
            LimitClass::Content => &mut self.content,
            LimitClass::Register => &mut self.register,
            LimitClass::Token => &mut self.token,
            LimitClass::Ws => &mut self.ws,
        }
    }

    fn sweep(&mut self, now: i64) {
        for limiter in [
            &mut self.general,
            &mut self.content,
            &mut self.register,
            &mut self.token,
            &mut self.ws,
            &mut self.connect,
        ] {
            limiter.sweep(now);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub connections: usize,
    pub tracked_peers: usize,
    pub websockets: usize,
}

/// The certificate and the TLS settings made from it (public mode).
pub struct Tls {
    pub config: Arc<rustls::ServerConfig>,
    pub info: CertInfo,
}

pub struct Gateway {
    config: Config,
    /// `None` in private mode (plain HTTP inside the tailnet)
    certificate: Option<CertInfo>,
    tls: Option<tokio_rustls::TlsAcceptor>,
    feed: Option<AllowlistFeed>,
    capture: Option<Arc<Capture>>,
    client: Client<HttpConnector, RequestBody>,
    limiters: Mutex<Limiters>,
    per_ip: Mutex<HashMap<String, usize>>,
    open: AtomicUsize,
    websockets: AtomicUsize,
    throttle: Throttle,
    closing: watch::Sender<bool>,
}

/// Counts an open connection until dropped.
struct Admitted {
    gateway: Arc<Gateway>,
    key: String,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.gateway.open.fetch_sub(1, Ordering::Relaxed);
        let mut per_ip = self.gateway.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = per_ip.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                per_ip.remove(&self.key);
            }
        }
    }
}

impl Gateway {
    /// A TLS gateway needs its certificate, and a private one has none (`config.tls`).
    pub fn new(
        config: Config,
        tls: Option<Tls>,
        feed: Option<AllowlistFeed>,
        capture: Option<CaptureWriter>,
    ) -> Result<Arc<Self>, String> {
        if config.tls != tls.is_some() {
            return Err(if config.tls {
                "the TLS gateway needs its certificate and key".into()
            } else {
                "a private gateway (GATEWAY_TLS=0) serves plain HTTP and takes no certificate".into()
            });
        }
        let capture = capture.map(|mut writer| {
            writer.add_secret(Some(&config.secret));
            writer.add_secret(config.allowlist.as_ref().map(|a| a.secret.as_str()));
            Arc::new(Capture::new(writer))
        });
        let (tls, certificate) = match tls {
            Some(Tls { config, info }) => (Some(tokio_rustls::TlsAcceptor::from(config)), Some(info)),
            None => (None, None),
        };
        let rates = &config.limits.rate;
        let limiters = Limiters {
            general: RateLimiter::new(rates.general),
            content: RateLimiter::new(rates.content),
            register: RateLimiter::new(rates.register),
            token: RateLimiter::new(rates.token),
            ws: RateLimiter::new(rates.ws),
            connect: RateLimiter::new(rates.connect),
        };
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        let client = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(64)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(TokioTimer::new())
            .build(connector);
        Ok(Arc::new(Self {
            config,
            certificate,
            tls,
            feed,
            capture,
            client,
            limiters: Mutex::new(limiters),
            per_ip: Mutex::default(),
            open: AtomicUsize::new(0),
            websockets: AtomicUsize::new(0),
            throttle: Throttle::new(60_000),
            closing: watch::Sender::new(false),
        }))
    }

    /// `None` in private mode.
    pub fn certificate(&self) -> Option<&CertInfo> {
        self.certificate.as_ref()
    }

    fn proto(&self) -> &'static str {
        if self.config.tls { "https" } else { "http" }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn stats(&self) -> Stats {
        let per_ip = self.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        Stats {
            connections: per_ip.values().sum(),
            tracked_peers: per_ip.len(),
            websockets: self.websockets.load(Ordering::Relaxed),
        }
    }

    fn closing(&self) -> bool {
        *self.closing.borrow()
    }

    fn take(&self, class: LimitClass, key: &str) -> Take {
        self.limiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .class(class)
            .take(key, crate::now_ms())
    }

    fn upstream(&self, name: Upstream) -> &Endpoint {
        match name {
            Upstream::Backend => &self.config.backend,
            Upstream::Content => &self.config.content,
            Upstream::Ws => &self.config.ws,
        }
    }

    /// Serves `listener` until `stop` turns true, then closes as [`Gateway::close`] says.
    pub async fn serve(self: Arc<Self>, listener: TcpListener, mut stop: watch::Receiver<bool>) {
        let mut connections = tokio::task::JoinSet::new();
        let sweeper = {
            let gateway = self.clone();
            tokio::spawn(async move {
                let mut every = tokio::time::interval(Duration::from_secs(60));
                every.tick().await;
                loop {
                    every.tick().await;
                    gateway
                        .limiters
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .sweep(crate::now_ms());
                }
            })
        };
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = until(&mut stop) => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            };
            let Ok((stream, peer)) = accepted else {
                continue;
            };
            if let Some(admitted) = self.admit(&peer) {
                let gateway = self.clone();
                connections.spawn(async move { gateway.connection(stream, peer, admitted).await });
            }
        }
        drop(listener);
        sweeper.abort();
        self.close(&mut connections).await;
    }

    /// Stops accepting, lets requests in flight finish (their answers carry `Connection: close`),
    /// then cuts whatever is left after the grace period. WebSockets have no natural end and are
    /// closed at once; the chat client reconnects.
    async fn close(&self, connections: &mut tokio::task::JoinSet<()>) {
        self.closing.send_replace(true);
        let grace = self.config.timeouts.shutdown_grace;
        let drained = tokio::time::timeout(grace, async { while connections.join_next().await.is_some() {} }).await;
        if drained.is_err() {
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        }
        if let Some(feed) = &self.feed {
            feed.idle().await;
        }
    }

    /// `OnConnection`: the per-address and total connection limits and the new-connection rate.
    fn admit(self: &Arc<Self>, peer: &SocketAddr) -> Option<Admitted> {
        if self.closing() {
            return None;
        }
        let ip = peer_address(Some(&peer.ip().to_string()));
        let key = rate_limit_key(&ip);
        let now = crate::now_ms();
        if self.open.load(Ordering::Relaxed) >= self.config.limits.max_connections {
            if self.throttle.allow("drop", now).is_some() {
                linelog::warn(
                    "connection refused: the gateway is at its connection limit",
                    json!({ "ip": ip, "max": self.config.limits.max_connections }),
                );
            }
            return None;
        }
        let mut per_ip = self.per_ip.lock().unwrap_or_else(PoisonError::into_inner);
        let count = per_ip.get(&key).copied().unwrap_or(0);
        if count >= self.config.limits.max_connections_per_ip {
            if self.throttle.allow(&format!("perip:{key}"), now).is_some() {
                linelog::warn(
                    "connection refused: too many open connections from one address",
                    json!({ "ip": ip, "open": count }),
                );
            }
            return None;
        }
        let rate = self
            .limiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .connect
            .take(&key, now);
        if rate != Take::Ok {
            if self.throttle.allow(&format!("connrate:{key}"), now).is_some() {
                linelog::warn(
                    "connection refused: too many new connections from one address",
                    json!({ "ip": ip }),
                );
            }
            return None;
        }
        per_ip.insert(key.clone(), count + 1);
        self.open.fetch_add(1, Ordering::Relaxed);
        Some(Admitted {
            gateway: self.clone(),
            key,
        })
    }

    async fn connection(self: Arc<Self>, stream: TcpStream, peer: SocketAddr, _admitted: Admitted) {
        let _ = stream.set_nodelay(true);
        let ip = peer_address(Some(&peer.ip().to_string()));
        let Some(acceptor) = self.tls.clone() else {
            return self.serve_connection(stream, ip).await;
        };
        match tokio::time::timeout(self.config.timeouts.handshake, acceptor.accept(stream)).await {
            Ok(Ok(tls)) => self.serve_connection(tls, ip).await,
            Ok(Err(error)) => self.handshake_failed(&ip, &error.to_string()),
            Err(_) => self.handshake_failed(&ip, "timed out"),
        }
    }

    /// HTTP/1.1 on one connection, TLS or (private mode) plain.
    async fn serve_connection<S>(self: Arc<Self>, io: S, ip: String)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let timeouts = &self.config.timeouts;
        let phases = Phases::new(timeouts.headers, timeouts.keep_alive, timeouts.idle);
        let conn: ConnectionId = Arc::new(OnceLock::new());
        let service = {
            let gateway = self.clone();
            let phases = phases.clone();
            let ip = ip.clone();
            hyper::service::service_fn(move |request: Request<Incoming>| {
                let (gateway, phases, ip, conn) = (gateway.clone(), phases.clone(), ip.clone(), conn.clone());
                async move { Ok::<_, Infallible>(gateway.handle(request, ip, conn, phases).await) }
            })
        };
        let connection = hyper::server::conn::http1::Builder::new()
            .timer(TokioTimer::new())
            // The guard keeps the header and keep-alive timers (hyper's would cut keep-alive at 30 s)
            .header_read_timeout(None)
            .max_headers(self.config.limits.max_headers_count)
            .max_header_size(16 * 1024)
            .serve_connection(TokioIo::new(Guard::new(io, phases)), service)
            .with_upgrades();
        tokio::pin!(connection);
        let mut closing = self.closing.subscribe();
        let result = tokio::select! {
            result = connection.as_mut() => result,
            () = until(&mut closing) => {
                connection.as_mut().graceful_shutdown();
                connection.await
            }
        };
        if let Err(error) = result {
            self.client_error(&ip, &error);
        }
    }

    fn handshake_failed(&self, ip: &str, error: &str) {
        if self.throttle.allow(&format!("tls:{ip}"), crate::now_ms()).is_some() {
            linelog::debug("tls handshake failed", json!({ "ip": ip, "error": error }));
        }
    }

    /// Malformed requests, oversized headers and the timeouts (slow clients) end here; hyper has
    /// already answered 400 or 431 where it could.
    fn client_error(&self, ip: &str, error: &hyper::Error) {
        let (status, reason) = if error.is_parse_too_large() {
            (431, "HPE_HEADER_OVERFLOW")
        } else if error.is_parse() || error.is_parse_status() {
            (400, "client_error")
        } else if error.is_timeout()
            || std::error::Error::source(error)
                .and_then(|s| s.downcast_ref::<std::io::Error>())
                .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
        {
            (408, "ERR_HTTP_REQUEST_TIMEOUT")
        } else {
            // A reset, a closed socket, a client that went away: nothing to say
            return;
        };
        if self
            .throttle
            .allow(&format!("clienterror:{ip}"), crate::now_ms())
            .is_some()
        {
            linelog::info(
                "request",
                json!({ "ip": ip, "route": "gateway", "status": status, "reason": reason }),
            );
        }
    }

    fn access(&self, request: &Request<Incoming>, ip: &str, conn: &ConnectionId) -> Access {
        let capture = self.capture.as_ref().map(|capture| {
            Box::new(Record {
                capture: capture.clone(),
                conn: conn.clone(),
                target: redact_capture_target(&request.uri().to_string()),
                req_headers: redact_capture_headers(request.headers()),
                req_type: content_type(request.headers()),
                req: Arc::new(Mutex::new(BodyTap::new(capture.max_body_bytes()))),
                res: BodyTap::new(capture.max_body_bytes()),
                res_headers: HeaderMap::new(),
            })
        });
        Access {
            capture,
            ip: ip.to_owned(),
            method: request.method().as_str().to_owned(),
            target: redact_target(&request.uri().to_string()),
            route: "gateway",
            status: 0,
            bytes_in: 0,
            reason: None,
            ua: redact_user_agent(request.headers().get(http::header::USER_AGENT).map(latin1).as_deref()),
            started: Instant::now(),
        }
    }

    /// `Refuse`: a JSON error. A refused request's body is never read, so a request that has one
    /// gets `Connection: close` instead of a drain.
    #[allow(clippy::too_many_arguments)]
    fn refuse(
        &self,
        request_headers: &HeaderMap,
        mut access: Access,
        phases: Phases,
        status: u16,
        error: &str,
        reason: String,
        extra: &[(&'static str, String)],
    ) -> Reply {
        access.route = "gateway";
        self.refuse_on(request_headers, access, phases, status, error, reason, extra)
    }

    #[allow(clippy::too_many_arguments)]
    fn refuse_on(
        &self,
        request_headers: &HeaderMap,
        mut access: Access,
        phases: Phases,
        status: u16,
        error: &str,
        reason: String,
        extra: &[(&'static str, String)],
    ) -> Reply {
        access.reason = Some(reason);
        access.status = status;
        let has_body = request_headers.contains_key(TRANSFER_ENCODING)
            || request_headers
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| service_kit::js::number(Some(&json!(v))) > 0.0);
        let body = Bytes::from(json!({ "error": error }).to_string());
        let mut response = Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .header("content-length", body.len())
            .header("cache-control", "no-store");
        if has_body || self.closing() {
            response = response.header(CONNECTION, "close");
        }
        for (name, value) in extra {
            response = response.header(*name, value);
        }
        let response = response
            .body(ResponseBody::full(body))
            .unwrap_or_else(|_| Response::new(ResponseBody::full(Bytes::new())));
        if let Some(record) = access.capture.as_deref_mut() {
            record.res_headers = response.headers().clone();
        }
        response.map(|body| body.logged(access, None, phases))
    }

    async fn handle(
        self: Arc<Self>,
        request: Request<Incoming>,
        ip: String,
        conn: ConnectionId,
        phases: Phases,
    ) -> Reply {
        phases.request_started();
        if is_upgrade(request.headers()) {
            return self.upgrade(request, ip, conn, phases).await;
        }
        let access = self.access(&request, &ip, &conn);
        let target = request.uri().to_string();
        let choice = classify(request.method().as_str(), &target, request.headers(), false);
        let limit_class = choice.limit_class();
        if let Take::Refused { retry_after_seconds } = self.take(limit_class, &rate_limit_key(&ip)) {
            return self.refuse(
                request.headers(),
                access,
                phases,
                429,
                "rate_limited",
                format!("rate_limited:{}", limit_class.as_str()),
                &[("retry-after", retry_after_seconds.to_string())],
            );
        }
        let (upstream, feed) = match choice {
            Decision::Reject {
                status, error, reason, ..
            } => return self.refuse(request.headers(), access, phases, status, error, reason.into(), &[]),
            Decision::Proxy { upstream, feed, .. } => (upstream, feed),
        };
        let max_body = self.config.limits.max_body_bytes;
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .map(|v| service_kit::js::number(Some(&json!(v))));
        if declared.is_some_and(|length| length > max_body as f64) {
            return self.refuse(
                request.headers(),
                access,
                phases,
                413,
                "body_too_large",
                "body_too_large".into(),
                &[],
            );
        }
        self.proxy(request, ip, access, phases, upstream, feed).await
    }

    async fn proxy(
        &self,
        request: Request<Incoming>,
        ip: String,
        mut access: Access,
        phases: Phases,
        upstream: Upstream,
        feed: Option<FeedKind>,
    ) -> Reply {
        access.route = upstream.as_str();
        let (parts, body) = request.into_parts();
        let endpoint = self.upstream(upstream);
        let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
        let exchange = Arc::new(Exchange::default());
        let body = RequestBody {
            inner: body,
            exchange: exchange.clone(),
            cap: self.config.limits.max_body_bytes,
            deadline: Box::pin(tokio::time::sleep(self.config.timeouts.request)),
            tap: access.capture.as_ref().map(|record| record.req.clone()),
        };
        let mut upstream_request = Request::new(body);
        *upstream_request.method_mut() = parts.method.clone();
        *upstream_request.headers_mut() =
            forward_headers(&parts.headers, &ip, &self.config.secret, false, self.proto());
        match format!("http://{}{path}", endpoint.authority()).parse() {
            Ok(uri) => *upstream_request.uri_mut() = uri,
            Err(_) => {
                return self.refuse(
                    &parts.headers,
                    access,
                    phases,
                    400,
                    "bad_request",
                    "target_not_origin_form".into(),
                    &[],
                );
            }
        }
        let idle = self.config.timeouts.upstream_idle;
        let answer = tokio::time::timeout(idle, self.client.request(upstream_request)).await;
        let response = match answer {
            Ok(Ok(response)) => response,
            failed => {
                if exchange.over_cap.load(Ordering::Relaxed) {
                    access.bytes_in = exchange.bytes_in.load(Ordering::Relaxed);
                    return self.refuse(
                        &parts.headers,
                        access,
                        phases,
                        413,
                        "body_too_large",
                        "body_too_large".into(),
                        &[],
                    );
                }
                if exchange.timed_out.load(Ordering::Relaxed) {
                    access.bytes_in = exchange.bytes_in.load(Ordering::Relaxed);
                    return self.refuse(
                        &parts.headers,
                        access,
                        phases,
                        408,
                        "request_timeout",
                        "ERR_HTTP_REQUEST_TIMEOUT".into(),
                        &[],
                    );
                }
                let (status, error, reason) = match &failed {
                    Err(_) => (504, "upstream_timeout", "upstream_timeout".to_owned()),
                    Ok(Err(error)) => (502, "bad_gateway", format!("upstream_{}", error_code(error))),
                    Ok(Ok(_)) => unreachable!("handled above"),
                };
                access.bytes_in = exchange.bytes_in.load(Ordering::Relaxed);
                return self.refuse_on(&parts.headers, access, phases, status, error, reason, &[]);
            }
        };
        let status = response.status();
        if let (Some(kind), Some(feed)) = (feed, &self.feed)
            && status.is_success()
            && (kind == FeedKind::Token || has_bearer(&parts.headers))
        {
            feed.report(&ip);
        }
        let headers = response_headers(response.headers(), self.closing());
        if let Some(record) = access.capture.as_deref_mut() {
            record.res_headers = response.headers().clone();
        }
        access.status = status.as_u16();
        let mut reply =
            Response::new(ResponseBody::upstream(response.into_body(), idle).logged(access, Some(exchange), phases));
        *reply.status_mut() = status;
        *reply.headers_mut() = headers;
        reply
    }

    /// WebSocket upgrades (the game's chat): the same checks as any request, then the handshake
    /// goes to the ws upstream and the two connections are joined byte for byte.
    async fn upgrade(
        self: Arc<Self>,
        mut request: Request<Incoming>,
        ip: String,
        conn: ConnectionId,
        phases: Phases,
    ) -> Reply {
        let mut access = self.access(&request, &ip, &conn);
        // Upgrades are recorded as chat (below), not as an HTTP exchange
        access.capture = None;
        let raw_refuse =
            |access: Access, status: u16, error: &str, reason: String, extra: &[(&'static str, String)]| {
                let mut headers = HeaderMap::new();
                headers.insert(CONTENT_LENGTH, HeaderValue::from(0));
                // An upgrade request has no body to drain, but the connection ends anyway
                let mut reply = self.refuse_on(&headers, access, phases.clone(), status, error, reason, extra);
                reply
                    .headers_mut()
                    .insert(CONNECTION, HeaderValue::from_static("close"));
                reply
            };
        if self.closing() {
            return raw_refuse(access, 503, "shutting_down", "shutting_down".into(), &[]);
        }
        let target = request.uri().to_string();
        let choice = classify(request.method().as_str(), &target, request.headers(), true);
        let limit_class = choice.limit_class();
        if let Take::Refused { retry_after_seconds } = self.take(limit_class, &rate_limit_key(&ip)) {
            return raw_refuse(
                access,
                429,
                "rate_limited",
                format!("rate_limited:{}", limit_class.as_str()),
                &[("retry-after", retry_after_seconds.to_string())],
            );
        }
        let upstream = match choice {
            Decision::Reject {
                status, error, reason, ..
            } => return raw_refuse(access, status, error, reason.into(), &[]),
            Decision::Proxy { upstream, .. } => upstream,
        };
        access.route = upstream.as_str();
        let endpoint = self.upstream(upstream).authority();
        let headers = forward_headers(request.headers(), &ip, &self.config.secret, true, self.proto());
        let path = request.uri().path_and_query().map_or("/", |p| p.as_str()).to_owned();
        let chat_target = redact_capture_target(&path);
        let handshake = async {
            let stream = TcpStream::connect(&endpoint).await?;
            let _ = stream.set_nodelay(true);
            let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
                .handshake::<_, http_body_util::Empty<Bytes>>(TokioIo::new(stream))
                .await?;
            tokio::spawn(connection.with_upgrades());
            let mut upstream_request = Request::new(http_body_util::Empty::new());
            *upstream_request.uri_mut() = path.parse()?;
            *upstream_request.headers_mut() = headers;
            Ok::<_, BoxError>(sender.send_request(upstream_request).await?)
        };
        let mut response = match tokio::time::timeout(self.config.timeouts.headers, handshake).await {
            Ok(Ok(response)) => response,
            Err(_) => return raw_refuse(access, 504, "upstream_timeout", "upstream_timeout".into(), &[]),
            Ok(Err(error)) => {
                return raw_refuse(
                    access,
                    502,
                    "bad_gateway",
                    format!("upstream_{}", error_code(error.as_ref())),
                    &[],
                );
            }
        };

        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            // The upstream answered without switching protocols: pass that answer on and hang up
            let status = response.status();
            let mut headers = response_headers(response.headers(), true);
            headers.remove(CONTENT_LENGTH);
            let mut body = Vec::new();
            let mut incoming = response.into_body();
            while let Some(Ok(frame)) = incoming.frame().await {
                if let Some(data) = frame.data_ref()
                    && body.len() < 64 * 1024
                {
                    body.extend_from_slice(data);
                }
            }
            access.status = status.as_u16();
            let mut reply = Response::new(ResponseBody::relayed(Bytes::from(body)).logged(access, None, phases));
            *reply.status_mut() = status;
            *reply.headers_mut() = headers;
            return reply;
        }

        let mut reply = Response::new(ResponseBody::full(Bytes::new()));
        *reply.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        for (name, value) in response.headers() {
            if !RESPONSE_DROP.contains(&name.as_str()) {
                reply.headers_mut().append(HeaderName::clone(name), value.clone());
            }
        }
        access.status = 101;
        access.log(0, false);
        let client_side = hyper::upgrade::on(&mut request);
        let upstream_side = hyper::upgrade::on(&mut response);
        let gateway = self.clone();
        tokio::spawn(async move {
            let (Ok(client), Ok(upstream)) = (client_side.await, upstream_side.await) else {
                return;
            };
            let ws_idle = gateway.config.timeouts.ws_idle;
            phases.upgraded(ws_idle);
            gateway.websockets.fetch_add(1, Ordering::Relaxed);
            let mut closing = gateway.closing.subscribe();
            let tunnel = async {
                let Some(capture) = &gateway.capture else {
                    return proxy::tunnel(TokioIo::new(client), TokioIo::new(upstream), ws_idle).await;
                };
                let id = capture.connection(&conn);
                capture.write(json!({
                    "kind": "chat",
                    "conn": id,
                    "event": "open",
                    "ip": access.ip,
                    "target": chat_target,
                }));
                let moved = proxy::tunnel(
                    ChatTap::new(TokioIo::new(client), capture.clone(), id, Direction::In),
                    ChatTap::new(TokioIo::new(upstream), capture.clone(), id, Direction::Out),
                    ws_idle,
                )
                .await;
                capture.write(json!({ "kind": "chat", "conn": id, "event": "close", "reason": "socket" }));
                moved
            };
            let (bytes_in, bytes_out) = tokio::select! {
                moved = tunnel => moved,
                () = until(&mut closing) => (0, 0),
            };
            gateway.websockets.fetch_sub(1, Ordering::Relaxed);
            linelog::info(
                "websocket closed",
                json!({
                    "ip": access.ip,
                    "target": access.target,
                    "bytesIn": bytes_in,
                    "bytesOut": bytes_out,
                    "ms": access.started.elapsed().as_millis() as u64,
                }),
            );
        });
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_headers_carry_the_gateways_own_claims_only() {
        let mut incoming = HeaderMap::new();
        for (name, value) in [
            ("x-forwarded-for", "1.2.3.4"),
            ("forwarded", "for=1.2.3.4"),
            ("x-real-ip", "1.2.3.4"),
            ("x-reforged-gateway", "guess"),
            ("expect", "100-continue"),
            ("connection", "keep-alive, x-hidden"),
            ("x-hidden", "1"),
            ("upgrade", "websocket"),
            ("authorization", "Bearer abc"),
            ("host", "play.example"),
        ] {
            incoming.append(name, HeaderValue::from_static(value));
        }
        let out = forward_headers(&incoming, "203.0.113.7", "s3cret", false, "https");
        let names: Vec<&str> = out.keys().map(HeaderName::as_str).collect();
        assert_eq!(
            names,
            [
                "authorization",
                "host",
                "x-forwarded-for",
                "x-forwarded-proto",
                "x-reforged-gateway"
            ]
        );
        assert_eq!(out["x-forwarded-for"], "203.0.113.7");
        assert_eq!(out["x-forwarded-proto"], "https");
        assert_eq!(out["x-reforged-gateway"], "s3cret");
        let private = forward_headers(&incoming, "203.0.113.7", "s3cret", false, "http");
        assert_eq!(private["x-forwarded-proto"], "http");
        let upgraded = forward_headers(&incoming, "203.0.113.7", "s3cret", true, "https");
        assert_eq!(
            (upgraded["connection"].as_bytes(), upgraded["upgrade"].as_bytes()),
            (&b"Upgrade"[..], &b"websocket"[..])
        );
    }

    #[test]
    fn upgrades_need_both_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        assert!(!is_upgrade(&headers));
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        assert!(is_upgrade(&headers));
    }

    #[test]
    fn upstream_error_codes() {
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(error_code(&refused), "ECONNREFUSED");
        let wrapped: BoxError = Box::new(std::io::Error::other(refused));
        assert_eq!(error_code(wrapped.as_ref()), "error");
    }
}
