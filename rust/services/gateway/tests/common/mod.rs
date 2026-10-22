#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)] // test helpers (test/helpers.ts)

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use gateway::capture::CaptureWriter;
use gateway::cert::{self, CertInfo};
use gateway::config::Config;
use gateway::feed::AllowlistFeed;
use gateway::gateway::{Gateway, Tls as GatewayTls};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;

pub const GATEWAY_SECRET: &str = "test-gateway-secret-0123456789abcdef0123456789abcdef";
pub const FAKE_JWT: &str =
    "eyJhbGciOiJSUzI1NiJ9.eyJ1c2VySWQiOiJVSUQtdGVzdCJ9.c2lnbmF0dXJlLW5vdC1yZWFsLXNpZ25hdHVyZS1ub3QtcmVhbA";

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Tests that read the shared log run one at a time.
pub async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await
}

// ---- the log ----

/// Every line logged so far: the raw text and its JSON.
pub type Log = Arc<Mutex<Vec<(String, Value)>>>;

pub fn log_lines() -> Log {
    static LINES: OnceLock<Log> = OnceLock::new();
    LINES
        .get_or_init(|| {
            let lines: Log = Arc::default();
            let sink = lines.clone();
            service_kit::linelog::set_sink(Some(Box::new(move |raw, _| {
                sink.lock()
                    .unwrap()
                    .push((raw.to_owned(), serde_json::from_str(raw).unwrap()));
            })));
            service_kit::linelog::set_level(service_kit::linelog::Level::Debug);
            lines
        })
        .clone()
}

pub fn log_len() -> usize {
    log_lines().lock().unwrap().len()
}

pub fn log_since(start: usize) -> Vec<(String, Value)> {
    log_lines().lock().unwrap()[start..].to_vec()
}

// ---- certificates ----

pub struct TestCert {
    pub dir: tempfile::TempDir,
    pub cert_file: std::path::PathBuf,
    pub key_file: std::path::PathBuf,
    pub cert_pem: String,
    pub info: CertInfo,
}

pub fn make_cert() -> TestCert {
    let (cert_pem, key_pem) = cert::make_certificate(cert::alt_names(&["127.0.0.1".into()]).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cert_file = dir.path().join("gateway-cert.pem");
    let key_file = dir.path().join("gateway-key.pem");
    std::fs::write(&cert_file, &cert_pem).unwrap();
    std::fs::write(&key_file, key_pem).unwrap();
    let info = cert::load(&cert_file, &key_file).unwrap().info;
    TestCert {
        dir,
        cert_file,
        key_file,
        cert_pem,
        info,
    }
}

// ---- fake upstreams ----

#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub aborted: bool,
    pub done: bool,
}

pub type Answer = Option<(u16, Vec<(&'static str, String)>, Vec<u8>)>;
pub type Handler = Arc<dyn Fn(Seen) -> Pin<Box<dyn Future<Output = Answer> + Send>> + Send + Sync>;

pub struct Upstream {
    pub port: u16,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Upstream {
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    pub fn last(&self) -> Seen {
        self.seen.lock().unwrap().last().cloned().unwrap()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Headers as Node's `IncomingHttpHeaders` shows them: lower case, repeats joined with ", ".
pub fn header_object(headers: &http::HeaderMap) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (name, value) in headers {
        let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
        out.entry(name.as_str().to_owned())
            .and_modify(|v| {
                v.push_str(", ");
                v.push_str(&text);
            })
            .or_insert(text);
    }
    out
}

/// A plain HTTP upstream that records every request and answers with a JSON echo, unless `handler`
/// answers.
pub async fn start_upstream(name: &'static str, handler: Option<Handler>) -> Upstream {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let record = seen.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (record, handler) = (record.clone(), handler.clone());
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                    let (record, handler) = (record.clone(), handler.clone());
                    async move {
                        let index = {
                            let mut seen = record.lock().unwrap();
                            seen.push(Seen {
                                method: request.method().to_string(),
                                url: request.uri().to_string(),
                                headers: header_object(request.headers()),
                                ..Seen::default()
                            });
                            seen.len() - 1
                        };
                        let body = match request.into_body().collect().await {
                            Ok(body) => body.to_bytes().to_vec(),
                            Err(_) => {
                                record.lock().unwrap()[index].aborted = true;
                                return Err::<http::Response<Full<Bytes>>, &str>("aborted");
                            }
                        };
                        let current = {
                            let mut seen = record.lock().unwrap();
                            seen[index].body = body.clone();
                            seen[index].done = true;
                            seen[index].clone()
                        };
                        let answer = match &handler {
                            Some(handler) => handler(current.clone()).await,
                            None => None,
                        };
                        let (status, headers, body) = answer.unwrap_or_else(|| {
                            let text = json!({
                                "upstream": name,
                                "method": current.method,
                                "url": current.url,
                                "headers": current.headers,
                                "bodyLength": current.body.len(),
                                "bodySha": sha256_hex(&current.body),
                            })
                            .to_string();
                            (
                                200,
                                vec![
                                    ("content-type", "application/json".into()),
                                    ("x-powered-by", "Express".into()),
                                ],
                                text.into_bytes(),
                            )
                        });
                        let mut response = http::Response::builder().status(status);
                        for (name, value) in headers {
                            response = response.header(name, value);
                        }
                        Ok(response.body(Full::new(Bytes::from(body))).unwrap())
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    Upstream { port, seen, task }
}

/// A port nothing listens on.
pub async fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

// ---- the gateway ----

pub struct Ports {
    pub backend: u16,
    pub content: u16,
    pub ws: u16,
}

/// `TestConfig`: loopback, the test secret, no allowlist feed, short grace.
pub fn test_config(cert: &TestCert, ports: &Ports, extra: &[(&str, &str)]) -> Config {
    let (backend, content, ws) = (
        format!("http://127.0.0.1:{}", ports.backend),
        format!("http://127.0.0.1:{}", ports.content),
        format!("http://127.0.0.1:{}", ports.ws),
    );
    let cert_file = cert.cert_file.to_string_lossy().into_owned();
    let key_file = cert.key_file.to_string_lossy().into_owned();
    let mut pairs: Vec<(&str, &str)> = vec![
        ("GATEWAY_BIND", "127.0.0.1"),
        ("GATEWAY_CERT", &cert_file),
        ("GATEWAY_KEY", &key_file),
        ("GATEWAY_SECRET", GATEWAY_SECRET),
        ("GATEWAY_BACKEND_URL", &backend),
        ("GATEWAY_CONTENT_URL", &content),
        ("GATEWAY_WS_URL", &ws),
        ("GATEWAY_ALLOWLIST", "0"),
        ("GATEWAY_SHUTDOWN_GRACE_MS", "2000"),
    ];
    pairs.extend_from_slice(extra);
    Config::from_env(&service_kit::env::Env::from_pairs(pairs)).unwrap()
}

pub struct Running {
    pub port: u16,
    pub gateway: Arc<Gateway>,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Running {
    /// `Close`: resolves when the gateway has stopped.
    pub async fn close(mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    pub fn stopper(&mut self) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
        (self.stop.clone(), self.task.take().unwrap())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

pub async fn start_gateway(cert: &TestCert, config: Config, feed: Option<AllowlistFeed>) -> Running {
    start_gateway_with(Some(cert), config, feed, None).await
}

/// A gateway with or without a certificate (private mode), and with the traffic capture.
pub async fn start_gateway_with(
    cert: Option<&TestCert>,
    config: Config,
    feed: Option<AllowlistFeed>,
    capture: Option<CaptureWriter>,
) -> Running {
    log_lines();
    let tls = cert.map(|cert| {
        let loaded = cert::load(&cert.cert_file, &cert.key_file).unwrap();
        GatewayTls {
            config: cert::server_config(&loaded).unwrap(),
            info: loaded.info,
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let gateway = Gateway::new(config, tls, feed, capture).unwrap();
    let (stop, stopping) = watch::channel(false);
    let task = tokio::spawn(gateway.clone().serve(listener, stopping));
    Running {
        port,
        gateway,
        stop,
        task: Some(task),
    }
}

// ---- clients ----

/// Accepts any certificate (as `rejectUnauthorized: false`), records its fingerprint, and refuses it
/// when it is not the pinned one, before any request byte goes out.
#[derive(Debug)]
struct Pinning {
    pin: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Pinning {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint = sha256_hex(end_entity);
        *self.seen.lock().unwrap() = Some(fingerprint.clone());
        match &self.pin {
            Some(pin) if *pin != fingerprint => Err(rustls::Error::General(format!(
                "certificate fingerprint {fingerprint} is not the pinned {pin}"
            ))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

pub type Tls = tokio_rustls::client::TlsStream<TcpStream>;

pub async fn tcp_connect(port: u16, local: Option<&str>) -> std::io::Result<TcpStream> {
    let socket = TcpSocket::new_v4()?;
    if let Some(local) = local {
        socket.bind(SocketAddr::new(local.parse::<IpAddr>().unwrap(), 0))?;
    }
    socket.connect(SocketAddr::from(([127, 0, 0, 1], port))).await
}

fn client_config(verifier: Arc<dyn ServerCertVerifier>) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// A TLS connection; the fingerprint the server showed.
pub async fn tls_connect_pinned(port: u16, local: Option<&str>, pin: Option<&str>) -> std::io::Result<(Tls, String)> {
    let seen = Arc::new(Mutex::new(None));
    let verifier = Arc::new(Pinning {
        pin: pin.map(str::to_owned),
        seen: seen.clone(),
        provider: Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
    });
    let tcp = tcp_connect(port, local).await?;
    let connector = tokio_rustls::TlsConnector::from(client_config(verifier));
    let stream = connector
        .connect(ServerName::try_from("127.0.0.1").unwrap().to_owned(), tcp)
        .await?;
    let fingerprint = seen.lock().unwrap().clone().unwrap_or_default();
    Ok((stream, fingerprint))
}

pub async fn tls_connect(port: u16, local: Option<&str>) -> std::io::Result<Tls> {
    tls_connect_pinned(port, local, None).await.map(|(s, _)| s)
}

/// A TLS connection that validates the certificate the ordinary way, with `ca` as the only root.
pub async fn tls_connect_ca(port: u16, ca_pem: &str) -> std::io::Result<Tls> {
    use rustls::pki_types::pem::PemObject;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(ca_pem.as_bytes()).unwrap())
        .unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = tcp_connect(port, None).await?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("127.0.0.1").unwrap().to_owned(), tcp)
        .await
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub fingerprint: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Default, Clone)]
pub struct Options<'a> {
    pub method: Option<&'a str>,
    pub headers: Vec<(&'a str, String)>,
    pub body: Option<Vec<u8>>,
    pub local: Option<&'a str>,
    pub pin: Option<&'a str>,
}

/// `HttpsRequest`: one request on a fresh connection; the target goes on the wire exactly as given.
pub async fn https_request(port: u16, target: &str, options: Options<'_>) -> Result<Reply, String> {
    let (tls, fingerprint) = tls_connect_pinned(port, options.local, options.pin)
        .await
        .map_err(|e| e.to_string())?;
    request_on(tls, target, &options).await.map(|mut reply| {
        reply.fingerprint = fingerprint;
        reply
    })
}

/// One plain HTTP request on a fresh connection (a private-mode gateway).
pub async fn plain_request(port: u16, target: &str, options: Options<'_>) -> Result<Reply, String> {
    let tcp = tcp_connect(port, options.local).await.map_err(|e| e.to_string())?;
    request_on(tcp, target, &options).await
}

pub async fn request_on<S>(io: S, target: &str, options: &Options<'_>) -> Result<Reply, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .handshake::<_, Full<Bytes>>(TokioIo::new(io))
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(connection);
    let mut request = http::Request::builder()
        .method(options.method.unwrap_or("GET"))
        .uri(target)
        .header("host", "127.0.0.1");
    for (name, value) in &options.headers {
        request = request.header(*name, value);
    }
    let body = options.body.clone().unwrap_or_default();
    let request = request.body(Full::new(Bytes::from(body))).map_err(|e| e.to_string())?;
    let response = sender.send_request(request).await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let headers = header_object(response.headers());
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes()
        .to_vec();
    Ok(Reply {
        status,
        headers,
        body,
        fingerprint: String::new(),
    })
}

/// Reads a response head off a raw stream (status, headers in order, and anything read past it).
pub async fn read_head<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("no head in 5 s")
            .unwrap();
        assert!(n > 0, "closed before the head: {}", String::from_utf8_lossy(&data));
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.trim().to_owned(), value.trim().to_owned())
        })
        .collect();
    (status, headers, data[end + 4..].to_vec())
}

/// Waits until the peer closes; how long that took.
pub async fn wait_close<S: tokio::io::AsyncRead + Unpin>(stream: &mut S, limit: Duration) -> (Duration, Vec<u8>) {
    let started = tokio::time::Instant::now();
    let mut received = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match tokio::time::timeout_at(started + limit, stream.read(&mut buf)).await {
            Err(_) => panic!("still open after {limit:?}"),
            Ok(Ok(0)) | Ok(Err(_)) => return (started.elapsed(), received),
            Ok(Ok(n)) => received.extend_from_slice(&buf[..n]),
        }
    }
}

// ---- a WebSocket upstream ----

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub fn ws_accept(key: &str) -> String {
    use base64::Engine;
    use sha1::Digest as _;
    let digest = sha1::Sha1::digest(format!("{key}{WS_GUID}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// The request target and headers of each handshake.
pub type WsSeen = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

pub struct WsUpstream {
    pub port: u16,
    pub seen: WsSeen,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for WsUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl WsUpstream {
    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    pub fn last(&self) -> (String, HashMap<String, String>) {
        self.seen.lock().unwrap().last().cloned().unwrap()
    }
}

/// A minimal WebSocket server: completes the handshake (choosing the first protocol asked for, as
/// the chat server does) and echoes bytes back with a prefix; `/refuse` gets a 404.
pub async fn start_ws_upstream() -> WsUpstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: WsSeen = Arc::default();
    let record = seen.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
            let record = record.clone();
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                let end = loop {
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end;
                    }
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&data[..end]).into_owned();
                let mut lines = head.split("\r\n");
                let url = lines.next().unwrap().split(' ').nth(1).unwrap_or("").to_owned();
                let mut headers = HashMap::new();
                for line in lines {
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_lowercase(), value.trim().to_owned());
                    }
                }
                record.lock().unwrap().push((url.clone(), headers.clone()));
                if url == "/refuse" {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nno room")
                        .await;
                    return;
                }
                let accept = ws_accept(headers.get("sec-websocket-key").map_or("", String::as_str));
                let protocol = headers
                    .get("sec-websocket-protocol")
                    .map(|asked| {
                        format!(
                            "Sec-WebSocket-Protocol: {}\r\n",
                            asked.split(',').next().unwrap().trim()
                        )
                    })
                    .unwrap_or_default();
                let reply = format!(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n{protocol}Sec-WebSocket-Accept: {accept}\r\n\r\n"
                );
                if stream.write_all(reply.as_bytes()).await.is_err() {
                    return;
                }
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            let mut echo = b"echo:".to_vec();
                            echo.extend_from_slice(&buf[..n]);
                            if stream.write_all(&echo).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    WsUpstream { port, seen, task }
}

/// An upgrade request over a fresh TLS connection: the status, the headers, the connection (for a
/// 101) and the body otherwise.
pub struct Upgraded {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub stream: Tls,
    pub rest: Vec<u8>,
    pub key: String,
}

impl Upgraded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The rest of a non-101 answer.
    pub async fn body(mut self) -> String {
        let mut rest = self.rest;
        let _ = self.stream.read_to_end(&mut rest).await;
        String::from_utf8_lossy(&rest).into_owned()
    }
}

pub async fn upgrade(port: u16, target: &str, extra: &[(&str, &str)], local: Option<&str>) -> Upgraded {
    let key = "dGhlIHNhbXBsZSBub25jZQ==".to_owned();
    let mut stream = tls_connect(port, local).await.unwrap();
    let mut text = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
    );
    for (name, value) in extra {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    stream.write_all(text.as_bytes()).await.unwrap();
    let (status, headers, rest) = read_head(&mut stream).await;
    Upgraded {
        status,
        headers,
        stream,
        rest,
        key,
    }
}

/// `ReforgedApiRoutes`: every route the backend registers under /reforged/api, read from its
/// source (paradox/backend/src/routes/reforgedApi.ts, patch 0002 in docs/paradox-patches.md).
pub fn reforged_api_routes() -> Vec<(String, String)> {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../paradox/backend/src/routes/reforgedApi.ts");
    let source = std::fs::read_to_string(file).unwrap();
    let pattern = regex::Regex::new(r#"(?m)^\s*reforgedApiRouter\.(get|post|put|delete|patch)\(\s*"([^"]+)""#).unwrap();
    let parameter = regex::Regex::new(r":[A-Za-z]+").unwrap();
    pattern
        .captures_iter(&source)
        .filter(|found| found[2].starts_with("/reforged/api/"))
        .map(|found| {
            (
                found[1].to_uppercase(),
                parameter.replace_all(&found[2], "ABCD-EFGH-JKMN").into_owned(),
            )
        })
        .collect()
}

pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub fn object(value: &Value) -> &Map<String, Value> {
    value.as_object().unwrap()
}
