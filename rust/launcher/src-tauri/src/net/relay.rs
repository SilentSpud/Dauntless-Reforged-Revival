//! The local relay (DR's contract C4, rows L22, L33; RA6): `src/main/relay.ts`.
//!
//! The game runtime sends the 1.14.7 client's backend calls to `REFORGED_BACKEND_URL`, which the
//! launcher sets to `http://127.0.0.1:61100`: this relay on the player's own PC. Every request (and the
//! chat WebSocket) is forwarded to the server:
//!   - public mode: over TLS to the server's gateway, on a connection pinned to the certificate
//!     fingerprint from the invite. Nothing leaves the PC unencrypted, and nothing is sent to a server
//!     whose certificate does not match.
//!   - private mode: over plain HTTP to the host inside the tailnet (Tailscale encrypts it), or on this PC.
//!
//! Paths go through as the runtime sent them, `/__origin/<host><path>` included: the gateway and the
//! backend unwrap those. The game measures its region by fetching its QoS URL (the server points it at
//! `http://127.0.0.1:61100/QoS`) and expects a fixed "pong" page; the relay answers that itself, and a
//! second, best-effort listener on 127.0.0.1:80 answers it too in case the game ignores the port.
//!
//! The relay runs only while the game runs. It listens on 127.0.0.1 only, refuses requests that look
//! like they come from a web browser (a page could otherwise use it through DNS rebinding), and, with an
//! owner check (`relay_owner.rs`, Windows), connections that do not come from the game's process.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::header::{CONNECTION, CONTENT_LENGTH, HOST, HeaderMap, HeaderValue, TRANSFER_ENCODING, UPGRADE};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_service::Service as _;

use super::connector::{Host, HostConnector, pin_error};
use super::pinned::PinnedTarget;

mod local;

use local::{Origin, request_problem};
pub use local::{QOS_PONG, is_qos_path, relay_port_override};

pub const DEFAULT_RELAY_PORT: u16 = 61100;
/// The extra QoS-only listener's port (`RelayOptions::qos_port`)
pub const DEFAULT_QOS_PORT: u16 = 80;
/// How long an upgraded connection (the chat) may be silent both ways before it is cut
const TUNNEL_IDLE: Duration = Duration::from_secs(24 * 60 * 60);
const UPGRADE_BODY_MAX: usize = 64 * 1024;

type Body = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayError {
    PortBusy(u16),
    ListenFailed(String),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortBusy(port) => write!(f, "port {port} is in use"),
            Self::ListenFailed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RelayError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Whether a local connection (its local and remote address) belongs to the game.
pub type Authorize = Arc<dyn Fn(SocketAddr, SocketAddr) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Where the relay sends the game's traffic: the gateway over pinned TLS (public, with the invite's
/// fingerprint), or the host over plain HTTP inside the tailnet or on this PC (private, no fingerprint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTarget {
    pub host: String,
    pub port: u16,
    pub fingerprint: Option<String>,
}

#[derive(Clone)]
pub struct RelayOptions {
    pub target: RelayTarget,
    pub port: u16,
    /// Also answer QoS pings on 127.0.0.1:<qos_port>. Best effort: a busy port is logged and ignored.
    pub qos_port: Option<u16>,
    pub connect_timeout: Duration,
    pub on_pin_error: Option<Arc<dyn Fn(Option<String>) + Send + Sync>>,
    pub on_log: Option<Arc<dyn Fn(LogLevel, String) + Send + Sync>>,
    /// The owner check (`relay_owner::game_owner_check`); None serves every loopback connection.
    pub authorize: Option<Authorize>,
}

impl RelayOptions {
    pub fn new(target: RelayTarget) -> Self {
        Self {
            target,
            port: DEFAULT_RELAY_PORT,
            qos_port: Some(DEFAULT_QOS_PORT),
            connect_timeout: Duration::from_secs(10),
            on_pin_error: None,
            on_log: None,
            authorize: None,
        }
    }
}

// ---------------------------------------------------------------- local request checks

/// Why a request must be refused, or None (`local::request_problem`).
pub fn local_request_problem(remote: SocketAddr, headers: &HeaderMap, upgrade: bool) -> Option<&'static str> {
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    let origin = match headers.get("origin") {
        None => Origin::Absent,
        Some(value) => value.to_str().map_or(Origin::Unreadable, Origin::Text),
    };
    let fetch_metadata = headers.keys().any(|name| name.as_str().starts_with("sec-fetch-"));
    request_problem(remote.ip(), host, origin, fetch_metadata, upgrade)
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(UPGRADE) && proxy::connection_tokens(headers).contains("upgrade")
}

/// The headers that cross: end to end only, never Host (set from the target) or HTTP/2's settings.
fn forwarded(headers: &HeaderMap) -> HeaderMap {
    proxy::end_to_end(headers, |name| name == "host" || name == "http2-settings")
}

/// The request target as the client sent it (origin form), `/` when there is none.
fn request_target(uri: &Uri) -> &str {
    uri.path_and_query().map_or("/", |path| path.as_str())
}

// ---------------------------------------------------------------- the relay

struct Shared {
    options: RelayOptions,
    host_header: HeaderValue,
    connector: HostConnector,
    client: Client<HostConnector, Incoming>,
    stop: CancellationToken,
    requests: AtomicU64,
    upgrades: AtomicU64,
    failures: AtomicU64,
    qos_answers: AtomicU64,
    upstream_connections: Arc<AtomicU64>,
}

impl Shared {
    fn log(&self, level: LogLevel, message: String) {
        if let Some(log) = &self.options.on_log {
            log(level, message);
        }
    }

    fn pin_failed(&self, actual: Option<String>) {
        self.log(
            LogLevel::Error,
            "the server's certificate does not match the invite; request refused".into(),
        );
        if let Some(callback) = &self.options.on_pin_error {
            callback(actual);
        }
    }
}

pub struct Relay {
    shared: Arc<Shared>,
    port: u16,
    qos_port: Option<u16>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn empty() -> Body {
    Empty::new().map_err(|never| match never {}).boxed()
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).map_err(|never| match never {}).boxed()
}

fn json_error(status: StatusCode, code: &str, close: bool) -> Response<Body> {
    let body = serde_json::json!({ "error": code }).to_string();
    let mut response = Response::new(full(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    if close {
        headers.insert(CONNECTION, HeaderValue::from_static("close"));
    }
    response
}

/// The QoS page: 200, `text/html`, its length, never cached; no body for HEAD.
fn pong(method: &Method) -> Response<Body> {
    let mut response = Response::new(if method == Method::HEAD {
        empty()
    } else {
        full(QOS_PONG)
    });
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static("text/html"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(QOS_PONG.len()));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

fn bare(status: StatusCode) -> Response<Body> {
    let mut response = Response::new(empty());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
}

async fn bind(port: u16) -> Result<TcpListener, RelayError> {
    TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => RelayError::PortBusy(port),
            _ => RelayError::ListenFailed(error.to_string()),
        })
}

impl Relay {
    /// Listens on 127.0.0.1 and forwards until `stop`. Plain HTTP (no fingerprint) only to a Tailscale
    /// address or name or to this PC, as for every other request (`http.rs`).
    pub async fn start(options: RelayOptions) -> Result<Self, RelayError> {
        let target = &options.target;
        let (upstream, mode) = match &target.fingerprint {
            Some(fingerprint) => (
                Host::Pinned(PinnedTarget {
                    host: target.host.clone(),
                    port: target.port,
                    fingerprint: fingerprint.clone(),
                }),
                "pinned TLS",
            ),
            None if protocol::invite::is_private_mode_host(&target.host) => (
                Host::Plain {
                    host: target.host.clone(),
                    port: target.port,
                },
                "plain HTTP, private mode",
            ),
            None => {
                return Err(RelayError::ListenFailed(
                    "plain HTTP only to Tailscale or loopback".into(),
                ));
            }
        };
        let host = if target.fingerprint.is_some() && target.port == 443 {
            target.host.clone()
        } else {
            format!("{}:{}", target.host, target.port)
        };
        let host_header =
            HeaderValue::from_str(&host).map_err(|_| RelayError::ListenFailed("invalid target host".into()))?;
        let connector = HostConnector::new(upstream, options.connect_timeout);
        let upstream_connections = connector.opened.clone();
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(8)
            .set_host(false)
            .build(connector.clone());
        let listener = bind(options.port).await?;
        let port = listener
            .local_addr()
            .map_err(|error| RelayError::ListenFailed(error.to_string()))?
            .port();
        let wanted_qos = options.qos_port;
        let shared = Arc::new(Shared {
            options,
            host_header,
            connector,
            client,
            stop: CancellationToken::new(),
            requests: AtomicU64::new(0),
            upgrades: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            qos_answers: AtomicU64::new(0),
            upstream_connections,
        });
        shared.log(
            LogLevel::Info,
            format!("relay listening on 127.0.0.1:{port} for {host} ({mode})"),
        );
        let mut tasks = vec![tokio::spawn(accept(listener, shared.clone()))];
        // The extra QoS-only listener. Never fatal: another program may hold the port.
        let mut qos_port = None;
        if let Some(wanted) = wanted_qos {
            match bind(wanted).await {
                Ok(qos) => {
                    qos_port = qos.local_addr().ok().map(|address| address.port());
                    tasks.push(tokio::spawn(accept_qos(qos, shared.clone())));
                }
                Err(error) => shared.log(
                    LogLevel::Info,
                    format!(
                        "QoS listener on 127.0.0.1:{wanted} not started ({error}); the relay port still answers QoS"
                    ),
                ),
            }
        }
        Ok(Self {
            shared,
            port,
            qos_port,
            tasks,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Where the extra QoS listener listens, when it started.
    pub fn qos_port(&self) -> Option<u16> {
        self.qos_port
    }

    pub fn requests(&self) -> u64 {
        self.shared.requests.load(Ordering::Relaxed)
    }

    pub fn upgrades(&self) -> u64 {
        self.shared.upgrades.load(Ordering::Relaxed)
    }

    pub fn failures(&self) -> u64 {
        self.shared.failures.load(Ordering::Relaxed)
    }

    /// QoS pings answered here, on either listener.
    pub fn qos_answers(&self) -> u64 {
        self.shared.qos_answers.load(Ordering::Relaxed)
    }

    /// Connections opened to the server so far (kept-alive ones are reused).
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
    pub fn upstream_connections(&self) -> u64 {
        self.shared.upstream_connections.load(Ordering::Relaxed)
    }

    /// Stops listening, cuts every connection and tunnel, and frees both ports.
    pub async fn stop(self) {
        self.shared.stop.cancel();
        for task in self.tasks {
            let _ = task.await;
        }
    }
}

async fn accept(listener: TcpListener, shared: Arc<Shared>) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = shared.stop.cancelled() => break,
        };
        let Ok((tcp, remote)) = accepted else { continue };
        let _ = tcp.set_nodelay(true);
        let local = tcp.local_addr().unwrap_or(remote);
        let shared = shared.clone();
        connections.spawn(async move {
            let allowed = match &shared.options.authorize {
                Some(authorize) => {
                    let allowed = authorize(local, remote).await;
                    if !allowed {
                        shared.log(
                            LogLevel::Warn,
                            "local connection rejected: game process ownership could not be verified".into(),
                        );
                    }
                    allowed
                }
                None => true,
            };
            let stop = shared.stop.clone();
            let service = service_fn(move |request| {
                let shared = shared.clone();
                async move { Ok::<_, Infallible>(handle(request, remote, allowed, shared).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .keep_alive(true)
                .max_buf_size(64 * 1024)
                .preserve_header_case(false)
                .serve_connection(TokioIo::new(tcp), service)
                .with_upgrades();
            tokio::select! {
                _ = connection => {}
                () = stop.cancelled() => {}
            }
        });
    }
    connections.shutdown().await;
}

/// The QoS-only listener: the pong page for `/qos...` and `/` after the same local-request rules, 404
/// for anything else. Nothing is forwarded from here, so it has no owner check.
async fn accept_qos(listener: TcpListener, shared: Arc<Shared>) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = shared.stop.cancelled() => break,
        };
        let Ok((tcp, remote)) = accepted else { continue };
        let shared = shared.clone();
        connections.spawn(async move {
            let stop = shared.stop.clone();
            let service = service_fn(move |request: Request<Incoming>| {
                let shared = shared.clone();
                async move {
                    let target = request_target(request.uri());
                    let refused = local_request_problem(remote, request.headers(), false).is_some()
                        || (!is_qos_path(target) && target != "/");
                    Ok::<_, Infallible>(if refused {
                        json_error(StatusCode::NOT_FOUND, "not_found", false)
                    } else {
                        shared.qos_answers.fetch_add(1, Ordering::Relaxed);
                        pong(request.method())
                    })
                }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .max_buf_size(64 * 1024)
                .serve_connection(TokioIo::new(tcp), service);
            tokio::select! {
                _ = connection => {}
                () = stop.cancelled() => {}
            }
        });
    }
    connections.shutdown().await;
}

async fn handle(request: Request<Incoming>, remote: SocketAddr, allowed: bool, shared: Arc<Shared>) -> Response<Body> {
    let upgrade = is_upgrade(request.headers());
    if !allowed {
        return if upgrade {
            bare(StatusCode::FORBIDDEN)
        } else {
            json_error(StatusCode::FORBIDDEN, "local_process_denied", true)
        };
    }
    if upgrade {
        handle_upgrade(request, remote, shared).await
    } else {
        handle_request(request, remote, shared).await
    }
}

async fn handle_request(request: Request<Incoming>, remote: SocketAddr, shared: Arc<Shared>) -> Response<Body> {
    shared.requests.fetch_add(1, Ordering::Relaxed);
    if let Some(problem) = local_request_problem(remote, request.headers(), false) {
        shared.log(LogLevel::Warn, format!("refused a local request ({problem})"));
        return json_error(StatusCode::FORBIDDEN, "forbidden", false);
    }
    if (request.method() == Method::GET || request.method() == Method::HEAD)
        && is_qos_path(request_target(request.uri()))
    {
        shared.qos_answers.fetch_add(1, Ordering::Relaxed);
        return pong(request.method());
    }
    let chunked = match request.headers().get(TRANSFER_ENCODING) {
        None => false,
        Some(value) if value.to_str().is_ok_and(|text| text.eq_ignore_ascii_case("chunked")) => true,
        Some(_) => return json_error(StatusCode::NOT_IMPLEMENTED, "unsupported_transfer_encoding", false),
    };
    let (parts, body) = request.into_parts();
    // `/__origin/<host><path>` too: the gateway and the backend unwrap it, the relay does not
    let Ok(uri) = format!("http://relay-upstream{}", request_target(&parts.uri)).parse::<Uri>() else {
        return json_error(StatusCode::BAD_REQUEST, "bad_request", false);
    };
    let mut upstream = Request::builder()
        .method(parts.method)
        .uri(uri)
        .version(hyper::Version::HTTP_11);
    if let Some(headers) = upstream.headers_mut() {
        *headers = forwarded(&parts.headers);
        headers.insert(HOST, shared.host_header.clone());
        if chunked {
            headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        }
    }
    let Ok(upstream) = upstream.body(body) else {
        return json_error(StatusCode::BAD_REQUEST, "bad_request", false);
    };
    match shared.client.request(upstream).await {
        Ok(answer) => {
            let (parts, body) = answer.into_parts();
            let mut response = Response::new(
                body.map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
                    .boxed(),
            );
            *response.status_mut() = parts.status;
            *response.headers_mut() = forwarded(&parts.headers);
            if let Some(reason) = parts.extensions.get::<hyper::ext::ReasonPhrase>() {
                response.extensions_mut().insert(reason.clone());
            }
            response
        }
        Err(error) => {
            shared.failures.fetch_add(1, Ordering::Relaxed);
            match pin_error(&error) {
                Some(actual) => {
                    shared.pin_failed(actual);
                    json_error(StatusCode::BAD_GATEWAY, "certificate_mismatch", false)
                }
                None => {
                    shared.log(LogLevel::Warn, format!("upstream request failed: {error}"));
                    json_error(StatusCode::BAD_GATEWAY, "upstream_unreachable", false)
                }
            }
        }
    }
}

async fn handle_upgrade(mut request: Request<Incoming>, remote: SocketAddr, shared: Arc<Shared>) -> Response<Body> {
    shared.upgrades.fetch_add(1, Ordering::Relaxed);
    if let Some(problem) = local_request_problem(remote, request.headers(), true) {
        shared.log(LogLevel::Warn, format!("refused a local upgrade ({problem})"));
        return bare(StatusCode::FORBIDDEN);
    }
    let failed = |shared: &Shared, error: &(dyn std::error::Error + 'static)| {
        shared.failures.fetch_add(1, Ordering::Relaxed);
        match pin_error(error) {
            Some(actual) => shared.pin_failed(actual),
            None => shared.log(LogLevel::Warn, format!("upstream upgrade failed: {error}")),
        }
        bare(StatusCode::BAD_GATEWAY)
    };

    // A connection of its own (pinned TLS, or plain in private mode): an upgraded connection never goes
    // back to the pool
    let stream = match shared
        .connector
        .clone()
        .call(Uri::from_static("http://relay-upstream/"))
        .await
    {
        Ok(stream) => stream,
        Err(error) => return failed(&shared, &error),
    };
    let (mut sender, connection) = match hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(stream).await {
        Ok(pair) => pair,
        Err(error) => return failed(&shared, &error),
    };
    tokio::spawn(connection.with_upgrades());

    let mut upstream = Request::builder()
        .method(request.method().clone())
        .uri(request.uri().clone());
    if let Some(headers) = upstream.headers_mut() {
        *headers = forwarded(request.headers());
        headers.insert(HOST, shared.host_header.clone());
        if let Some(protocol) = request.headers().get(UPGRADE) {
            headers.insert(UPGRADE, protocol.clone());
        }
        headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    }
    let Ok(upstream) = upstream.body(Empty::new()) else {
        return bare(StatusCode::BAD_REQUEST);
    };
    let mut answer = match sender.send_request(upstream).await {
        Ok(answer) => answer,
        Err(error) => return failed(&shared, &error),
    };

    if answer.status() == StatusCode::SWITCHING_PROTOCOLS {
        let mut response = Response::new(empty());
        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        for (name, value) in answer.headers() {
            let lower = name.as_str();
            if lower == "connection" || lower == "upgrade" || !proxy::is_hop_by_hop(lower) {
                response.headers_mut().append(name.clone(), value.clone());
            }
        }
        let client = hyper::upgrade::on(&mut request);
        let server = hyper::upgrade::on(&mut answer);
        let stop = shared.stop.clone();
        tokio::spawn(async move {
            let (Ok(client), Ok(server)) = (client.await, server.await) else {
                return;
            };
            tokio::select! {
                _ = proxy::tunnel(TokioIo::new(client), TokioIo::new(server), TUNNEL_IDLE) => {}
                () = stop.cancelled() => {}
            }
        });
        return response;
    }

    // The server did not switch protocols: pass its answer on and close
    let status = answer.status();
    let headers = proxy::end_to_end(answer.headers(), |name| name == "content-length");
    let mut body = Vec::new();
    let mut incoming = answer.into_body();
    while let Some(Ok(frame)) = incoming.frame().await {
        if let Some(chunk) = frame.data_ref()
            && body.len() + chunk.len() <= UPGRADE_BODY_MAX
        {
            body.extend_from_slice(chunk);
        }
    }
    let mut response = Response::new(full(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response
}

#[cfg(test)]
mod tests;
