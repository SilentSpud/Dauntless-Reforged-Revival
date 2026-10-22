//! Requests to the host from the invite, and nowhere else (row L20): `src/main/http.ts`. Redirects are
//! never followed, so a request that carries the account key cannot be bounced to another address.
//!
//! Private mode (invite v1): plain HTTP over Tailscale, which encrypts the traffic itself. Public mode
//! (invite v2): HTTP/1.1 over TLS to the server's gateway, pinned to the invite's certificate
//! (`pinned.rs`). Each request has a connection of its own, as today.

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::pinned::{ConnectError, PinnedTarget, pinned_connect};
use crate::core::invite::{is_private_mode_host, is_valid_host};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// The certificate fingerprint in public mode, None for plain HTTP
    pub pin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    Timeout,
    Network(String),
    TooLarge,
    Aborted,
    BadUrl(&'static str),
    /// The server's certificate is not the invite's (the "not your friend's server" message)
    Pin {
        actual: Option<String>,
    },
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("request timed out"),
            Self::Network(message) => f.write_str(message),
            Self::TooLarge => f.write_str("response too large"),
            Self::Aborted => f.write_str("request aborted"),
            Self::BadUrl(message) => f.write_str(message),
            Self::Pin { .. } => f.write_str("the server's certificate does not match the invite"),
        }
    }
}

impl std::error::Error for HttpError {}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug, Clone)]
pub struct RequestOptions {
    pub method: Method,
    pub headers: Vec<(&'static str, String)>,
    /// Sent as JSON
    pub body: Option<String>,
    pub timeout: Duration,
    pub max_bytes: usize,
    pub cancel: Option<CancellationToken>,
}

impl RequestOptions {
    pub fn get(timeout: Duration, max_bytes: usize) -> Self {
        Self {
            method: Method::GET,
            headers: Vec::new(),
            body: None,
            timeout,
            max_bytes,
            cancel: None,
        }
    }
}

pub fn check_endpoint(endpoint: &Endpoint) -> Result<(), HttpError> {
    if !is_valid_host(&endpoint.host) || endpoint.port == 0 {
        return Err(HttpError::BadUrl("invalid endpoint"));
    }
    match &endpoint.pin {
        Some(pin) if !protocol::invite::is_valid_fingerprint(pin) => {
            Err(HttpError::BadUrl("invalid certificate fingerprint"))
        }
        // Plain HTTP (private mode) only into the tailnet or to this PC; anything else must be pinned TLS
        None if !is_private_mode_host(&endpoint.host) => {
            Err(HttpError::BadUrl("plain HTTP only to Tailscale or loopback"))
        }
        _ => Ok(()),
    }
}

pub fn host_header(endpoint: &Endpoint) -> String {
    if endpoint.pin.is_some() && endpoint.port == 443 {
        endpoint.host.clone()
    } else {
        format!("{}:{}", endpoint.host, endpoint.port)
    }
}

pub fn pinned_target(endpoint: &Endpoint) -> Option<PinnedTarget> {
    endpoint.pin.as_ref().map(|pin| PinnedTarget {
        host: endpoint.host.clone(),
        port: endpoint.port,
        fingerprint: pin.clone(),
    })
}

fn connect_error(error: ConnectError) -> HttpError {
    match error {
        ConnectError::Pin { actual } => HttpError::Pin { actual },
        ConnectError::Timeout => HttpError::Timeout,
        ConnectError::Io(error) => HttpError::Network(error.to_string()),
    }
}

async fn exchange<S>(stream: S, request: Request<Full<Bytes>>, max_bytes: usize) -> Result<HttpResponse, HttpError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|error| HttpError::Network(error.to_string()))?;
    let driver = tokio::spawn(connection);
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| HttpError::Network(error.to_string()));
    let result = async {
        let response = response?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut body = response.into_body();
        let mut data = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|error| HttpError::Network(error.to_string()))?;
            if let Some(chunk) = frame.data_ref() {
                if data.len() + chunk.len() > max_bytes {
                    return Err(HttpError::TooLarge);
                }
                data.extend_from_slice(chunk);
            }
        }
        Ok(HttpResponse {
            status,
            headers,
            body: Bytes::from(data),
        })
    }
    .await;
    driver.abort();
    result
}

/// One request to the endpoint, with a deadline for the whole of it (connect, answer and body).
pub async fn request(
    endpoint: &Endpoint,
    path_and_query: &str,
    options: RequestOptions,
) -> Result<HttpResponse, HttpError> {
    check_endpoint(endpoint)?;
    if !path_and_query.starts_with('/') {
        return Err(HttpError::BadUrl("path must start with /"));
    }
    let mut builder = Request::builder().method(options.method.clone()).uri(path_and_query);
    let headers = builder.headers_mut().ok_or(HttpError::BadUrl("invalid request"))?;
    let value = |text: &str| HeaderValue::from_str(text).map_err(|_| HttpError::BadUrl("invalid header"));
    headers.insert("accept", HeaderValue::from_static("application/json"));
    headers.insert("host", value(&host_header(endpoint))?);
    for (name, text) in &options.headers {
        headers.insert(*name, value(text)?);
    }
    let body = match &options.body {
        Some(text) => {
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            headers.insert("content-length", value(&text.len().to_string())?);
            Full::new(Bytes::from(text.clone()))
        }
        None => Full::new(Bytes::new()),
    };
    let request = builder.body(body).map_err(|_| HttpError::BadUrl("invalid request"))?;

    let work = async {
        match pinned_target(endpoint) {
            Some(target) => {
                let stream = pinned_connect(&target, options.timeout).await.map_err(connect_error)?;
                exchange(stream, request, options.max_bytes).await
            }
            None => {
                let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
                    .await
                    .map_err(|error| HttpError::Network(error.to_string()))?;
                let _ = stream.set_nodelay(true);
                exchange(stream, request, options.max_bytes).await
            }
        }
    };
    let cancel = options.cancel.clone().unwrap_or_default();
    if cancel.is_cancelled() {
        return Err(HttpError::Aborted);
    }
    tokio::select! {
        result = tokio::time::timeout(options.timeout, work) => result.unwrap_or(Err(HttpError::Timeout)),
        () = cancel.cancelled() => Err(HttpError::Aborted),
    }
}

/// The body as JSON, or None (`parseJsonBody`): an empty body that does not say it is JSON is no JSON.
pub fn parse_json_body(response: &HttpResponse) -> Option<Value> {
    let kind = response
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !kind.contains("json") && response.body.is_empty() {
        return None;
    }
    serde_json::from_str(&String::from_utf8_lossy(&response.body)).ok()
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;
    use crate::net::pinned::certificate_fingerprint;
    use crate::net::pinned::tests::{acceptor, certificate};
    use hyper::service::service_fn;
    use hyper::{Response, StatusCode};
    use tokio::net::TcpListener;

    /// Serves HTTP/1.1 on a free loopback port, plain or over TLS; answers with `answer`.
    async fn serve(
        tls: Option<tokio_rustls::TlsAcceptor>,
        answer: fn(Request<hyper::body::Incoming>) -> Response<Full<Bytes>>,
    ) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let tls = tls.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| async move { Ok::<_, Infallible>(answer(request)) });
                    let http = hyper::server::conn::http1::Builder::new();
                    match tls {
                        Some(tls) => {
                            if let Ok(stream) = tls.accept(tcp).await {
                                let _ = http.serve_connection(TokioIo::new(stream), service).await;
                            }
                        }
                        None => {
                            let _ = http.serve_connection(TokioIo::new(tcp), service).await;
                        }
                    }
                });
            }
        });
        port
    }

    fn echo(request: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
        let host = request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body =
            serde_json::json!({ "path": request.uri().to_string(), "host": host, "method": request.method().as_str() });
        Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .unwrap()
    }

    fn options() -> RequestOptions {
        RequestOptions::get(Duration::from_secs(5), 1 << 20)
    }

    #[tokio::test]
    async fn plain_http_to_loopback() {
        let port = serve(None, echo).await;
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
            pin: None,
        };
        let response = request(&endpoint, "/undaunted/api/ServerStatus?x=1", options())
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        let body = parse_json_body(&response).unwrap();
        assert_eq!(body["path"], "/undaunted/api/ServerStatus?x=1");
        assert_eq!(body["host"], format!("127.0.0.1:{port}"));
    }

    #[tokio::test]
    async fn pinned_tls_and_a_wrong_pin() {
        let (cert, key) = certificate();
        let fingerprint = certificate_fingerprint(&cert);
        let port = serve(Some(acceptor(cert, key)), echo).await;
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
            pin: Some(fingerprint.clone()),
        };
        let response = request(&endpoint, "/x", options()).await.unwrap();
        assert_eq!(parse_json_body(&response).unwrap()["host"], format!("127.0.0.1:{port}"));
        let wrong = Endpoint {
            pin: Some("0".repeat(64)),
            ..endpoint
        };
        assert_eq!(
            request(&wrong, "/x", options()).await.unwrap_err(),
            HttpError::Pin {
                actual: Some(fingerprint)
            }
        );
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_size_is_capped() {
        fn redirect(_: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
            Response::builder()
                .status(StatusCode::FOUND)
                .header("location", "http://203.0.113.1/")
                .body(Full::new(Bytes::from("x".repeat(100))))
                .unwrap()
        }
        let port = serve(None, redirect).await;
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
            pin: None,
        };
        assert_eq!(request(&endpoint, "/", options()).await.unwrap().status, 302);
        let small = RequestOptions {
            max_bytes: 99,
            ..options()
        };
        assert_eq!(request(&endpoint, "/", small).await.unwrap_err(), HttpError::TooLarge);
    }

    #[tokio::test]
    async fn timeouts_and_aborts() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
            pin: None,
        };
        let quick = RequestOptions {
            timeout: Duration::from_millis(200),
            ..options()
        };
        assert_eq!(request(&endpoint, "/", quick).await.unwrap_err(), HttpError::Timeout);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let aborted = RequestOptions {
            cancel: Some(cancel),
            ..options()
        };
        assert_eq!(request(&endpoint, "/", aborted).await.unwrap_err(), HttpError::Aborted);
        drop(listener);
    }

    #[test]
    fn endpoints_are_checked() {
        let fp = "3f".repeat(32);
        let ok = |host: &str, pin: Option<&str>| {
            check_endpoint(&Endpoint {
                host: host.into(),
                port: 443,
                pin: pin.map(str::to_owned),
            })
            .is_ok()
        };
        assert!(ok("100.64.0.1", None));
        assert!(ok("localhost", None));
        assert!(!ok("203.0.113.9", None), "plain HTTP only into the tailnet");
        assert!(ok("203.0.113.9", Some(&fp)));
        assert!(!ok("203.0.113.9", Some("abc")));
        assert!(!ok("bad host", Some(&fp)));
        let public = Endpoint {
            host: "play.example.org".into(),
            port: 443,
            pin: Some(fp.clone()),
        };
        assert_eq!(host_header(&public), "play.example.org");
        assert_eq!(host_header(&Endpoint { port: 8443, ..public }), "play.example.org:8443");
        assert_eq!(
            host_header(&Endpoint {
                host: "100.64.0.1".into(),
                port: 443,
                pin: None
            }),
            "100.64.0.1:443"
        );
    }
}
