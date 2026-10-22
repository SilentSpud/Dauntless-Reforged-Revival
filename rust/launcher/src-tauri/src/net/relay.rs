//! The local relay for public mode (contract C4, row L22): `src/main/relay.ts`.
//!
//! The 1.4.4 client only speaks plain HTTP, so in public mode it is pointed at this relay on
//! `http://127.0.0.1:61000` on the player's own PC. Every request (and the chat WebSocket) is forwarded
//! over TLS to the server's gateway, on a connection pinned to the invite's certificate. Nothing leaves
//! the PC unencrypted, and nothing is sent to a server whose certificate does not match.
//!
//! The relay runs only while the game runs. It listens on 127.0.0.1 only and refuses requests that look
//! like they come from a web browser (a page could otherwise use it through DNS rebinding), and, with an
//! owner check, connections that do not come from the game's process.

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
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::connector::{Host, HostConnector, pin_error};
use super::pinned::{PinnedTarget, pinned_connect};

pub const DEFAULT_RELAY_PORT: u16 = 61000;
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

#[derive(Clone)]
pub struct RelayOptions {
    pub target: PinnedTarget,
    pub port: u16,
    pub connect_timeout: Duration,
    pub on_pin_error: Option<Arc<dyn Fn(Option<String>) + Send + Sync>>,
    pub on_log: Option<Arc<dyn Fn(LogLevel, String) + Send + Sync>>,
    pub authorize: Option<Authorize>,
}

impl RelayOptions {
    pub fn new(target: PinnedTarget) -> Self {
        Self {
            target,
            port: DEFAULT_RELAY_PORT,
            connect_timeout: Duration::from_secs(10),
            on_pin_error: None,
            on_log: None,
            authorize: None,
        }
    }
}

/// The hidden override for the relay port (`DAUNTLESS_REVIVED_RELAY_PORT`; tests and rehearsals on a PC
/// where 61000 is taken): 4 or 5 digits, 1024 to 65535.
pub fn relay_port_override(value: Option<&str>) -> Option<u16> {
    let text = value?.trim();
    let shaped =
        (4..=5).contains(&text.len()) && !text.starts_with('0') && text.bytes().all(|byte| byte.is_ascii_digit());
    text.parse::<u16>().ok().filter(|port| shaped && *port >= 1024)
}

// ---------------------------------------------------------------- local request checks

fn is_loopback_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == "localhost" || name == "::1" {
        return true;
    }
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() == 4
        && parts[0] == "127"
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// "127.0.0.1:61000", "localhost", "[::1]:61000" -> the host part
fn host_part(value: &str) -> &str {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('[')
        && let Some((host, rest)) = inner.split_once(']')
        && (rest.is_empty()
            || rest
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())))
    {
        return host;
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => value,
    }
}

/// `/^(?:[a-z][a-z0-9+.-]*:\/\/)?(\[[^\]]+\]|[^/:]+)(?::\d+)?\/?$/i`: the host an Origin names.
fn origin_host(origin: &str) -> Option<&str> {
    let mut rest = origin.trim();
    if let Some((scheme, after)) = rest.split_once("://") {
        let mut chars = scheme.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
        if valid {
            rest = after;
        }
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, after) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        (host, after)
    } else {
        let end = rest.find([':', '/']).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        (&rest[..end], &rest[end..])
    };
    let port_ok = after.is_empty()
        || after
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    port_ok.then_some(host)
}

/// Why a request must be refused, or None. The game sends `Host: 127.0.0.1:<port>` and no browser
/// headers. A web page cannot avoid sending `Sec-Fetch-*` on plain requests, and a browser WebSocket
/// always carries the page's Origin; DNS rebinding shows up as a foreign Host.
pub fn local_request_problem(remote: SocketAddr, headers: &HeaderMap, upgrade: bool) -> Option<&'static str> {
    let ip = match remote.ip() {
        std::net::IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or(std::net::IpAddr::V6(v6), std::net::IpAddr::V4),
        ip => ip,
    };
    if !ip.is_loopback() {
        return Some("not_local");
    }
    let host = headers.get(HOST).and_then(|value| value.to_str().ok());
    if !host.is_some_and(|host| is_loopback_name(host_part(host))) {
        return Some("host");
    }
    if !upgrade && headers.keys().any(|name| name.as_str().starts_with("sec-fetch-")) {
        return Some("browser");
    }
    if let Some(origin) = headers.get("origin") {
        let host = origin.to_str().ok().and_then(origin_host);
        if !host.is_some_and(is_loopback_name) {
            return Some("origin");
        }
    }
    None
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(UPGRADE) && proxy::connection_tokens(headers).contains("upgrade")
}

/// The headers that cross: end to end only, never Host (set from the target) or HTTP/2's settings.
fn forwarded(headers: &HeaderMap) -> HeaderMap {
    proxy::end_to_end(headers, |name| name == "host" || name == "http2-settings")
}

// ---------------------------------------------------------------- the relay

struct Shared {
    options: RelayOptions,
    host_header: HeaderValue,
    client: Client<HostConnector, Incoming>,
    stop: CancellationToken,
    requests: AtomicU64,
    upgrades: AtomicU64,
    failures: AtomicU64,
    tls_connections: Arc<AtomicU64>,
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
    task: tokio::task::JoinHandle<()>,
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

impl Relay {
    /// Listens on 127.0.0.1 and forwards until `stop`.
    pub async fn start(options: RelayOptions) -> Result<Self, RelayError> {
        let target = &options.target;
        let host = if target.port == 443 {
            target.host.clone()
        } else {
            format!("{}:{}", target.host, target.port)
        };
        let host_header =
            HeaderValue::from_str(&host).map_err(|_| RelayError::ListenFailed("invalid target host".into()))?;
        let connector = HostConnector::new(Host::Pinned(target.clone()), options.connect_timeout);
        let tls_connections = connector.opened.clone();
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(8)
            .set_host(false)
            .build(connector);
        let listener = TcpListener::bind(("127.0.0.1", options.port))
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => {
                    RelayError::PortBusy(options.port)
                }
                _ => RelayError::ListenFailed(error.to_string()),
            })?;
        let port = listener
            .local_addr()
            .map_err(|error| RelayError::ListenFailed(error.to_string()))?
            .port();
        let shared = Arc::new(Shared {
            options,
            host_header,
            client,
            stop: CancellationToken::new(),
            requests: AtomicU64::new(0),
            upgrades: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            tls_connections,
        });
        shared.log(
            LogLevel::Info,
            format!("relay listening on 127.0.0.1:{port} for {host}"),
        );
        let task = tokio::spawn(accept(listener, shared.clone()));
        Ok(Self { shared, port, task })
    }

    pub fn port(&self) -> u16 {
        self.port
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

    /// TLS connections opened to the server so far (kept-alive ones are reused).
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
    pub fn tls_connections(&self) -> u64 {
        self.shared.tls_connections.load(Ordering::Relaxed)
    }

    /// Stops listening, cuts every connection and tunnel, and frees the port.
    pub async fn stop(self) {
        self.shared.stop.cancel();
        let _ = self.task.await;
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
    let chunked = match request.headers().get(TRANSFER_ENCODING) {
        None => false,
        Some(value) if value.to_str().is_ok_and(|text| text.eq_ignore_ascii_case("chunked")) => true,
        Some(_) => return json_error(StatusCode::NOT_IMPLEMENTED, "unsupported_transfer_encoding", false),
    };
    let (parts, body) = request.into_parts();
    let path = parts.uri.path_and_query().map_or("/", |path| path.as_str());
    let Ok(uri) = format!("http://relay-upstream{path}").parse::<Uri>() else {
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

    // A connection of its own: an upgraded connection never goes back to the pool
    let stream = match pinned_connect(&shared.options.target, shared.options.connect_timeout).await {
        Ok(stream) => stream,
        Err(error) => return failed(&shared, &error),
    };
    shared.tls_connections.fetch_add(1, Ordering::Relaxed);
    let (mut sender, connection) =
        match hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(stream)).await {
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
