//! The relay against a local TLS "gateway" with a throwaway certificate (and a plain HTTP host for private
//! mode), as Reforged's `test/relay.test.ts`, plus the QoS page, `/__origin/` paths and the owner check.

use std::sync::Mutex;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::*;
use crate::net::pinned::certificate_fingerprint;
use crate::net::pinned::tests::{acceptor, certificate};
use crate::net::relay_owner;

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    uri: String,
    headers: HeaderMap,
}

struct Upstream {
    port: u16,
    fingerprint: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Upstream {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn accept_key(key: &str) -> String {
    use base64::Engine as _;
    let digest = sha1_smol::Sha1::from(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
        .digest()
        .bytes();
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Minimal WebSocket echo over an upgraded connection: masked client text frames, answers "echo:<text>".
async fn ws_echo(io: hyper::upgrade::Upgraded) {
    let mut io = TokioIo::new(io);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let Ok(n) = io.read(&mut chunk).await else { return };
        if n == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..n]);
        while buffer.len() >= 2 {
            let opcode = buffer[0] & 0x0f;
            let length = usize::from(buffer[1] & 0x7f);
            let masked = buffer[1] & 0x80 != 0;
            let start = 2 + if masked { 4 } else { 0 };
            if buffer.len() < start + length {
                break;
            }
            let mut payload = buffer[start..start + length].to_vec();
            if masked {
                for (i, byte) in payload.iter_mut().enumerate() {
                    *byte ^= buffer[2 + i % 4];
                }
            }
            buffer.drain(..start + length);
            if opcode == 0x8 {
                let _ = io.write_all(&[0x88, 0x00]).await;
                return;
            }
            if opcode == 0x1 {
                let out = [b"echo:".as_slice(), &payload].concat();
                let _ = io.write_all(&[[0x81, out.len() as u8].as_slice(), &out].concat()).await;
            }
        }
    }
}

type Frames = tokio::sync::mpsc::Sender<Result<hyper::body::Frame<Bytes>, Box<dyn std::error::Error + Send + Sync>>>;

fn streamed() -> (Frames, Body) {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    (
        tx,
        http_body_util::StreamBody::new(tokio_stream::wrappers::ReceiverStream::new(rx)).boxed(),
    )
}

async fn route(mut request: Request<Incoming>, seen: Arc<Mutex<Vec<Seen>>>) -> Response<Body> {
    seen.lock().unwrap().push(Seen {
        method: request.method().to_string(),
        uri: request.uri().to_string(),
        headers: request.headers().clone(),
    });
    let json = |value: serde_json::Value| {
        let mut response = Response::new(full(value.to_string()));
        response
            .headers_mut()
            .insert("content-type", HeaderValue::from_static("application/json"));
        response
    };
    if request.headers().contains_key(UPGRADE) {
        let path = request.uri().path().to_owned();
        let Some(key) = request
            .headers()
            .get("sec-websocket-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
        else {
            return bare(StatusCode::BAD_REQUEST);
        };
        // "/ws" for the tests' own client; "//" is the game's chat connection (libwebsockets 3.0)
        if path != "/ws" && path != "//" {
            let mut response = Response::new(full("no such socket"));
            *response.status_mut() = StatusCode::BAD_REQUEST;
            return response;
        }
        let protocol = request
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or("").trim().to_owned());
        let upgraded = hyper::upgrade::on(&mut request);
        tokio::spawn(async move {
            if let Ok(io) = upgraded.await {
                ws_echo(io).await;
            }
        });
        let mut response = Response::new(empty());
        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        let headers = response.headers_mut();
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(
            "sec-websocket-accept",
            HeaderValue::from_str(&accept_key(&key)).unwrap(),
        );
        headers.insert("x-upstream", HeaderValue::from_static("yes"));
        if let Some(protocol) = protocol {
            headers.insert("sec-websocket-protocol", HeaderValue::from_str(&protocol).unwrap());
        }
        return response;
    }
    match request.uri().path() {
        "/body" => {
            let te = request
                .headers()
                .get(TRANSFER_ENCODING)
                .map(|v| v.to_str().unwrap().to_owned());
            let cl = request
                .headers()
                .get(CONTENT_LENGTH)
                .map(|v| v.to_str().unwrap().to_owned());
            let body = request.into_body().collect().await.unwrap().to_bytes();
            let sha256: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
            json(serde_json::json!({ "length": body.len(), "sha256": sha256, "te": te, "cl": cl }))
        }
        "/headers" => {
            let mut response = Response::new(full("ok"));
            response
                .extensions_mut()
                .insert(hyper::ext::ReasonPhrase::try_from(&b"Fine Thanks"[..]).unwrap());
            let headers = response.headers_mut();
            headers.append("set-cookie", HeaderValue::from_static("a=1; Path=/"));
            headers.append("set-cookie", HeaderValue::from_static("b=2; Path=/"));
            headers.insert("x-custom", HeaderValue::from_static("yes"));
            headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
            headers.insert(CONNECTION, HeaderValue::from_static("x-private"));
            headers.insert("x-private", HeaderValue::from_static("hop-only"));
            headers.insert("content-type", HeaderValue::from_static("text/plain"));
            response
        }
        "/teapot" => {
            let mut response = Response::new(empty());
            *response.status_mut() = StatusCode::IM_A_TEAPOT;
            response
                .extensions_mut()
                .insert(hyper::ext::ReasonPhrase::try_from(&b"I'm a teapot"[..]).unwrap());
            response
        }
        "/head" => {
            let mut response = Response::new(empty());
            response
                .headers_mut()
                .insert(CONTENT_LENGTH, HeaderValue::from_static("1234"));
            response
        }
        "/stream" => {
            let (tx, body) = streamed();
            tokio::spawn(async move {
                for (delay, part) in [(0, "one|"), (300, "two|"), (300, "three")] {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    let _ = tx.send(Ok(hyper::body::Frame::data(Bytes::from(part)))).await;
                }
            });
            Response::new(body)
        }
        "/stream-in" => {
            // Answers "got-first|" as soon as the first part arrives, then "done:<bytes>" at the end
            let (tx, body) = streamed();
            let mut incoming = request.into_body();
            tokio::spawn(async move {
                let mut total = 0;
                while let Some(Ok(frame)) = incoming.frame().await {
                    if let Some(data) = frame.data_ref() {
                        if total == 0 {
                            let _ = tx.send(Ok(hyper::body::Frame::data(Bytes::from("got-first|")))).await;
                        }
                        total += data.len();
                    }
                }
                let _ = tx
                    .send(Ok(hyper::body::Frame::data(Bytes::from(format!("done:{total}")))))
                    .await;
            });
            Response::new(body)
        }
        // Everything else answers what it was asked, so a test can see the path as the server got it
        _ => json(serde_json::json!({ "method": request.method().as_str(), "url": request.uri().to_string() })),
    }
}

/// A local "gateway" (TLS with a throwaway certificate) or a plain HTTP host (private mode).
async fn upstream_with(tls: bool) -> Upstream {
    let (cert, key) = certificate();
    let fingerprint = certificate_fingerprint(&cert);
    let tls = tls.then(|| acceptor(cert, key));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (tls, log) = (tls.clone(), log.clone());
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let log = log.clone();
                    async move { Ok::<_, Infallible>(route(request, log).await) }
                });
                let builder = hyper::server::conn::http1::Builder::new();
                match tls {
                    Some(tls) => {
                        let Ok(stream) = tls.accept(tcp).await else { return };
                        let _ = builder
                            .serve_connection(TokioIo::new(stream), service)
                            .with_upgrades()
                            .await;
                    }
                    None => {
                        let _ = builder
                            .serve_connection(TokioIo::new(tcp), service)
                            .with_upgrades()
                            .await;
                    }
                }
            });
        }
    });
    Upstream {
        port,
        fingerprint,
        seen,
    }
}

async fn upstream() -> Upstream {
    upstream_with(true).await
}

fn options_for(upstream: &Upstream, fingerprint: Option<&str>) -> RelayOptions {
    let mut options = RelayOptions::new(RelayTarget {
        host: "127.0.0.1".into(),
        port: upstream.port,
        fingerprint: Some(fingerprint.unwrap_or(&upstream.fingerprint).to_owned()),
    });
    options.port = 0;
    options.qos_port = None;
    options
}

async fn relay_for(upstream: &Upstream, fingerprint: Option<&str>) -> (Relay, Arc<Mutex<Vec<Option<String>>>>) {
    let pin_errors = Arc::new(Mutex::new(Vec::new()));
    let errors = pin_errors.clone();
    let mut options = options_for(upstream, fingerprint);
    options.on_pin_error = Some(Arc::new(move |actual| errors.lock().unwrap().push(actual)));
    (Relay::start(options).await.unwrap(), pin_errors)
}

struct Answer {
    status: u16,
    reason: Option<String>,
    headers: HeaderMap,
    body: Bytes,
}

/// One request to the relay on a connection of its own, with `Host: 127.0.0.1:<port>` unless `headers`
/// names a Host.
async fn call(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: Option<Bytes>) -> Answer {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(connection);
    let mut request = Request::builder().method(method).uri(path);
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("host")) {
        request = request.header(HOST, format!("127.0.0.1:{port}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = sender
        .send_request(request.body(Full::new(body.unwrap_or_default())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let reason = response
        .extensions()
        .get::<hyper::ext::ReasonPhrase>()
        .map(|r| String::from_utf8_lossy(r.as_bytes()).into_owned());
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        reason,
        headers,
        body,
    }
}

fn json(answer: &Answer) -> serde_json::Value {
    serde_json::from_slice(&answer.body).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn values(headers: &HeaderMap, name: &str) -> Vec<String> {
    headers
        .get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn forwards_method_path_query_headers_and_body_over_the_pinned_connection() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let body: Vec<u8> = (0..256 * 1024).map(|i| (i * 7 % 251) as u8).collect();
    let length = body.len().to_string();
    let answer = call(
        relay.port(),
        "POST",
        "/body?x=1&y=%20z",
        &[
            ("content-type", "application/octet-stream"),
            ("content-length", &length),
            ("x-test", "a"),
            ("x-dup", "one"),
            ("x-dup", "two"),
            ("authorization", "bearer game-token"),
            ("connection", "keep-alive, x-conn-named"),
            ("x-conn-named", "must-not-pass"),
            ("proxy-authorization", "Basic abc"),
            ("te", "trailers"),
        ],
        Some(Bytes::from(body.clone())),
    )
    .await;
    assert_eq!(answer.status, 200);
    let reply = json(&answer);
    assert_eq!(reply["length"].as_u64(), Some(body.len() as u64));
    assert_eq!(reply["sha256"].as_str(), Some(sha256_hex(&body).as_str()));
    assert_eq!(reply["cl"].as_str(), Some(length.as_str()));
    let seen = upstream.seen().last().cloned().unwrap();
    assert_eq!((seen.method.as_str(), seen.uri.as_str()), ("POST", "/body?x=1&y=%20z"));
    assert_eq!(values(&seen.headers, "host"), [format!("127.0.0.1:{}", upstream.port)]);
    assert_eq!(values(&seen.headers, "x-test"), ["a"]);
    assert_eq!(values(&seen.headers, "x-dup"), ["one", "two"]);
    assert_eq!(values(&seen.headers, "authorization"), ["bearer game-token"]);
    for gone in ["x-conn-named", "proxy-authorization", "te"] {
        assert!(!seen.headers.contains_key(gone), "{gone} never crosses");
    }
    relay.stop().await;
}

#[tokio::test]
async fn origin_paths_pass_through_unchanged() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    for path in [
        "/__origin/content.example.com/api/v1/news?lang=pt-BR",
        "/__origin/127.0.0.1:61100/x%2Fy",
        "/__origin/",
    ] {
        let answer = call(relay.port(), "GET", path, &[], None).await;
        assert_eq!(json(&answer)["url"].as_str(), Some(path), "{path}");
    }
    relay.stop().await;
}

#[tokio::test]
async fn a_chunked_request_body_is_streamed_on_as_chunked() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let parts: Vec<Bytes> = [10_000usize, 50_000, 3]
        .iter()
        .map(|n| Bytes::from(vec![b'x'; *n]))
        .collect();
    let all = parts.concat();
    let tcp = TcpStream::connect(("127.0.0.1", relay.port())).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(connection);
    let (tx, body) = streamed();
    let request = Request::builder()
        .method("POST")
        .uri("/body")
        .header(HOST, "127.0.0.1")
        .body(body)
        .unwrap();
    tokio::spawn(async move {
        for part in parts {
            let _ = tx.send(Ok(hyper::body::Frame::data(part))).await;
        }
    });
    let answer = sender.send_request(request).await.unwrap();
    let reply: serde_json::Value =
        serde_json::from_slice(&answer.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(reply["te"].as_str(), Some("chunked"));
    assert!(reply["cl"].is_null());
    assert_eq!(reply["length"].as_u64(), Some(all.len() as u64));
    assert_eq!(reply["sha256"].as_str(), Some(sha256_hex(&all).as_str()));
    relay.stop().await;
}

#[tokio::test]
async fn response_reason_repeated_headers_and_no_hop_by_hop() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let answer = call(relay.port(), "GET", "/headers", &[], None).await;
    assert_eq!((answer.status, answer.reason.as_deref()), (200, Some("Fine Thanks")));
    assert_eq!(values(&answer.headers, "set-cookie"), ["a=1; Path=/", "b=2; Path=/"]);
    assert_eq!(answer.headers.get("x-custom").unwrap(), "yes");
    assert!(!answer.headers.contains_key("keep-alive"));
    assert!(!answer.headers.contains_key("x-private"));
    assert_eq!(&answer.body[..], b"ok");
    let teapot = call(relay.port(), "GET", "/teapot", &[], None).await;
    // hyper keeps a reason phrase only when it is not the canonical one, so None here is "I'm a teapot"
    let reason = teapot.reason.as_deref().or(StatusCode::IM_A_TEAPOT.canonical_reason());
    assert_eq!((teapot.status, reason), (418, Some("I'm a teapot")));
    relay.stop().await;
}

#[tokio::test]
async fn head_keeps_the_content_length_and_sends_no_body() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let head = call(relay.port(), "HEAD", "/head", &[], None).await;
    assert_eq!(
        (
            head.status,
            head.headers.get(CONTENT_LENGTH).unwrap().to_str().unwrap(),
            head.body.len()
        ),
        (200, "1234", 0)
    );
    relay.stop().await;
}

#[tokio::test]
async fn responses_are_streamed_and_connections_kept_alive() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let tcp = TcpStream::connect(("127.0.0.1", relay.port())).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(connection);
    let request = Request::builder()
        .uri("/stream")
        .header(HOST, "127.0.0.1")
        .body(Empty::new())
        .unwrap();
    let started = std::time::Instant::now();
    let mut body = sender.send_request(request).await.unwrap().into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], b"one|");
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "the first part came before the rest"
    );
    let mut rest = Vec::new();
    while let Some(Ok(frame)) = body.frame().await {
        rest.extend_from_slice(frame.data_ref().unwrap());
    }
    assert_eq!(&rest[..], b"two|three");
    assert!(started.elapsed() >= Duration::from_millis(550));
    for _ in 0..5 {
        assert_eq!(call(relay.port(), "GET", "/echo", &[], None).await.status, 200);
    }
    assert!(
        relay.upstream_connections() <= 2,
        "kept alive: {}",
        relay.upstream_connections()
    );
    relay.stop().await;
}

#[tokio::test]
async fn request_bodies_are_streamed_the_server_sees_the_first_part_early() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let tcp = TcpStream::connect(("127.0.0.1", relay.port())).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(connection);
    let (tx, body) = streamed();
    let request = Request::builder()
        .method("POST")
        .uri("/stream-in")
        .header(HOST, "127.0.0.1")
        .body(body)
        .unwrap();
    tx.send(Ok(hyper::body::Frame::data(Bytes::from("part1"))))
        .await
        .unwrap();
    let mut answer = sender.send_request(request).await.unwrap().into_body();
    let first = answer.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], b"got-first|");
    // Only now, after the server answered the first part, send the rest
    tx.send(Ok(hyper::body::Frame::data(Bytes::from("part2"))))
        .await
        .unwrap();
    drop(tx);
    let mut rest = Vec::new();
    while let Some(Ok(frame)) = answer.frame().await {
        rest.extend_from_slice(frame.data_ref().unwrap());
    }
    assert_eq!(&rest[..], b"done:10");
    relay.stop().await;
}

/// The WebSocket handshake and one echoed frame, over a raw socket to the relay.
async fn websocket(port: u16, path: &str, extra: &str) -> (String, Option<String>) {
    websocket_with_host(port, path, &format!("Host: 127.0.0.1:{port}\r\n{extra}")).await
}

async fn websocket_with_host(port: u16, path: &str, headers: &str) -> (String, Option<String>) {
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let handshake = format!(
        "GET {path} HTTP/1.1\r\n{headers}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    tcp.write_all(handshake.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if tcp.read(&mut byte).await.unwrap_or(0) == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    if !head.starts_with("HTTP/1.1 101") {
        return (head, None);
    }
    let mask = [1u8, 2, 3, 4];
    let payload: Vec<u8> = b"hello".iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
    tcp.write_all(&[[0x81, 0x80 | 5].as_slice(), &mask, &payload].concat())
        .await
        .unwrap();
    let mut frame = [0u8; 12];
    tcp.read_exact(&mut frame).await.unwrap();
    (head, Some(String::from_utf8_lossy(&frame[2..]).into_owned()))
}

#[tokio::test]
async fn websockets_reach_the_server_including_the_games_chat() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let (head, echo) = websocket(relay.port(), "/ws", "").await;
    assert!(head.to_ascii_lowercase().contains("x-upstream: yes"), "{head}");
    assert_eq!(echo.as_deref(), Some("echo:hello"));
    let seen = upstream.seen().last().cloned().unwrap();
    assert_eq!(values(&seen.headers, "host"), [format!("127.0.0.1:{}", upstream.port)]);
    assert_eq!(values(&seen.headers, "upgrade"), ["websocket"]);
    assert!(values(&seen.headers, "connection")[0].eq_ignore_ascii_case("upgrade"));
    assert!(seen.headers.contains_key("sec-websocket-key"));
    // As libwebsockets 3.0 sends it: GET //, `Host: 127.0.0.1` and a loopback Origin without a port,
    // protocol xmpp
    let (head, echo) = websocket_with_host(
        relay.port(),
        "//",
        "Pragma: no-cache\r\nCache-Control: no-cache\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1\r\nSec-WebSocket-Protocol: xmpp\r\n",
    )
    .await;
    assert!(
        head.to_ascii_lowercase()
            .contains("\r\nsec-websocket-protocol: xmpp\r\n"),
        "{head}"
    );
    assert_eq!(echo.as_deref(), Some("echo:hello"));
    let seen = upstream.seen().last().cloned().unwrap();
    assert_eq!((seen.method.as_str(), seen.uri.as_str()), ("GET", "//"));
    assert_eq!(values(&seen.headers, "sec-websocket-protocol"), ["xmpp"]);
    assert_eq!(relay.upgrades(), 2);
    relay.stop().await;
}

#[tokio::test]
async fn a_server_with_another_certificate_gets_nothing() {
    let upstream = upstream().await;
    let (other, _) = certificate();
    let (relay, pin_errors) = relay_for(&upstream, Some(&certificate_fingerprint(&other))).await;
    let answer = call(
        relay.port(),
        "POST",
        "/body",
        &[
            ("content-type", "application/json"),
            ("authorization", "bearer secret-token"),
        ],
        Some(Bytes::from(r#"{"secret":"must not leave the PC"}"#)),
    )
    .await;
    assert_eq!(
        (answer.status, json(&answer)),
        (502, serde_json::json!({ "error": "certificate_mismatch" }))
    );
    let (head, _) = websocket(relay.port(), "/ws", "").await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert!(upstream.seen().is_empty(), "no request reached the server");
    assert_eq!(
        pin_errors.lock().unwrap().clone(),
        vec![Some(upstream.fingerprint.clone()); 2],
        "the error names the certificate that was actually presented"
    );
    relay.stop().await;
}

#[tokio::test]
async fn requests_and_websockets_from_web_pages_are_refused() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    for headers in [
        vec![("host", "evil.example")],
        vec![("sec-fetch-site", "cross-site"), ("sec-fetch-mode", "no-cors")],
        vec![("origin", "https://evil.example")],
    ] {
        let answer = call(relay.port(), "GET", "/echo", &headers, None).await;
        assert_eq!(answer.status, 403, "{headers:?}");
    }
    let port = relay.port();
    let (head, _) = websocket(port, "/ws", "Origin: https://evil.example\r\n").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    let (head, _) = websocket_with_host(
        port,
        "/ws",
        &format!("Host: evil.example:{port}\r\nOrigin: http://evil.example:{port}\r\n"),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(upstream.seen().is_empty());
    let local_origin = format!("http://127.0.0.1:{port}");
    assert_eq!(
        call(port, "GET", "/echo", &[("origin", &local_origin)], None)
            .await
            .status,
        200
    );
    let localhost = format!("localhost:{port}");
    assert_eq!(
        call(port, "GET", "/echo", &[("host", &localhost)], None).await.status,
        200
    );
    relay.stop().await;
}

#[tokio::test]
async fn qos_is_answered_locally_and_never_forwarded() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    for path in ["/QoS", "/qos", "/QOS/", "/QoS?region=eu"] {
        let answer = call(relay.port(), "GET", path, &[], None).await;
        assert_eq!((answer.status, &answer.body[..]), (200, QOS_PONG.as_bytes()), "{path}");
        assert_eq!(answer.headers.get("content-type").unwrap(), "text/html");
        assert_eq!(answer.headers.get("cache-control").unwrap(), "no-store");
        assert_eq!(answer.headers.get(CONTENT_LENGTH).unwrap(), &QOS_PONG.len().to_string());
    }
    let head = call(relay.port(), "HEAD", "/QoS", &[], None).await;
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(head.headers.get(CONTENT_LENGTH).unwrap(), &QOS_PONG.len().to_string());
    assert!(upstream.seen().is_empty(), "QoS never reaches the server");
    assert_eq!(relay.qos_answers(), 5);
    // Not QoS: forwarded as any other request
    for (method, path) in [("GET", "/qosx"), ("POST", "/QoS"), ("GET", "/api/qos")] {
        let answer = call(relay.port(), method, path, &[], None).await;
        assert_eq!(json(&answer)["url"].as_str(), Some(path), "{method} {path}");
    }
    assert_eq!(upstream.seen().len(), 3);
    // The local rules come first: a web page gets no pong either
    let answer = call(relay.port(), "GET", "/QoS", &[("origin", "https://evil.example")], None).await;
    assert_eq!(answer.status, 403);
    assert_eq!(relay.qos_answers(), 5);
    relay.stop().await;
}

#[tokio::test]
async fn the_qos_listener_answers_qos_and_the_root_only_and_closes_with_the_relay() {
    let upstream = upstream().await;
    let mut options = options_for(&upstream, None);
    options.qos_port = Some(0);
    let relay = Relay::start(options).await.unwrap();
    let qos = relay.qos_port().unwrap();
    for path in ["/", "/QoS", "/qos?x=1"] {
        let answer = call(qos, "GET", path, &[], None).await;
        assert_eq!((answer.status, &answer.body[..]), (200, QOS_PONG.as_bytes()), "{path}");
    }
    for (path, headers) in [
        ("/echo", vec![]),
        ("/QoS", vec![("host", "evil.example")]),
        ("/QoS", vec![("sec-fetch-mode", "no-cors")]),
    ] {
        let answer = call(qos, "GET", path, &headers, None).await;
        assert_eq!(
            (answer.status, json(&answer)),
            (404, serde_json::json!({ "error": "not_found" })),
            "{path} {headers:?}"
        );
    }
    assert!(upstream.seen().is_empty());
    assert_eq!(relay.qos_answers(), 3);
    relay.stop().await;
    assert!(
        TcpStream::connect(("127.0.0.1", qos)).await.is_err(),
        "freed with the relay"
    );
}

#[tokio::test]
async fn a_busy_qos_port_is_logged_and_ignored() {
    let upstream = upstream().await;
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let log = logs.clone();
    let mut options = options_for(&upstream, None);
    options.qos_port = Some(taken.local_addr().unwrap().port());
    options.on_log = Some(Arc::new(move |level, message| {
        log.lock().unwrap().push((level, message))
    }));
    let relay = Relay::start(options).await.unwrap();
    assert_eq!(relay.qos_port(), None);
    assert_eq!(call(relay.port(), "GET", "/QoS", &[], None).await.status, 200);
    let logs = logs.lock().unwrap().clone();
    assert!(logs[0].1.ends_with("(pinned TLS)"), "{logs:?}");
    assert!(
        logs.iter()
            .any(|(level, message)| *level == LogLevel::Info && message.contains("QoS listener")),
        "{logs:?}"
    );
    relay.stop().await;
}

#[tokio::test]
async fn private_mode_carries_requests_and_the_chat_over_plain_http() {
    let upstream = upstream_with(false).await;
    let logs = Arc::new(Mutex::new(Vec::new()));
    let log = logs.clone();
    let mut options = options_for(&upstream, None);
    options.target.fingerprint = None;
    options.on_log = Some(Arc::new(move |_, message| log.lock().unwrap().push(message)));
    let relay = Relay::start(options).await.unwrap();
    let answer = call(relay.port(), "GET", "/__origin/x.example/y?z=1", &[], None).await;
    assert_eq!(json(&answer)["url"].as_str(), Some("/__origin/x.example/y?z=1"));
    let (_, echo) = websocket(relay.port(), "//", "Sec-WebSocket-Protocol: xmpp\r\n").await;
    assert_eq!(echo.as_deref(), Some("echo:hello"));
    // The port is always named in private mode, 443 or not
    let seen = upstream.seen();
    assert!(
        seen.iter()
            .all(|s| values(&s.headers, "host") == [format!("127.0.0.1:{}", upstream.port)])
    );
    assert_eq!(call(relay.port(), "GET", "/QoS", &[], None).await.status, 200);
    assert!(logs.lock().unwrap()[0].ends_with("(plain HTTP, private mode)"));
    relay.stop().await;
}

#[tokio::test]
async fn plain_http_only_to_tailscale_or_this_pc() {
    for host in ["203.0.113.9", "10.0.0.5", "example.com"] {
        let mut options = RelayOptions::new(RelayTarget {
            host: host.into(),
            port: 8080,
            fingerprint: None,
        });
        options.port = 0;
        options.qos_port = None;
        assert_eq!(
            Relay::start(options).await.err(),
            Some(RelayError::ListenFailed(
                "plain HTTP only to Tailscale or loopback".into()
            )),
            "{host}"
        );
    }
    for host in ["100.64.0.1", "my-pc.tail-scale.ts.net", "localhost"] {
        let mut options = RelayOptions::new(RelayTarget {
            host: host.into(),
            port: 8080,
            fingerprint: None,
        });
        options.port = 0;
        options.qos_port = None;
        Relay::start(options).await.unwrap().stop().await;
    }
}

#[tokio::test]
async fn the_owner_check_denies_before_forwarding() {
    let upstream = upstream().await;
    let mut options = options_for(&upstream, None);
    options.authorize = Some(Arc::new(|_, _| Box::pin(async { false })));
    let relay = Relay::start(options).await.unwrap();
    let answer = call(relay.port(), "GET", "/echo", &[], None).await;
    assert_eq!(
        (answer.status, json(&answer)["error"].as_str()),
        (403, Some("local_process_denied"))
    );
    assert_eq!(answer.headers.get(CONNECTION).unwrap(), "close");
    assert_eq!(call(relay.port(), "GET", "/QoS", &[], None).await.status, 403);
    let (head, _) = websocket(relay.port(), "/ws", "").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(upstream.seen().is_empty());
    relay.stop().await;
}

/// RD21 on a real socket: with the Windows owner check, a connection from a process other than the game
/// the launcher registered is refused, and the game's own is served.
#[cfg(windows)]
#[tokio::test]
async fn on_windows_only_the_registered_game_process_is_served() {
    let upstream = upstream().await;
    // Some other process that is running: the "game" the launcher started is not this test
    let mut other =
        std::process::Command::new(crate::platform::launch::system_root().join("System32").join("ping.exe"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
    let game = other.id();
    let mut options = options_for(&upstream, None);
    options.authorize = relay_owner::game_owner_check(Arc::new(move || Some(game)));
    assert!(options.authorize.is_some(), "Windows has an owner check");
    let relay = Relay::start(options).await.unwrap();
    let answer = call(relay.port(), "GET", "/echo", &[], None).await;
    assert_eq!(
        (answer.status, json(&answer)["error"].as_str()),
        (403, Some("local_process_denied"))
    );
    let (head, _) = websocket(relay.port(), "//", "").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(
        upstream.seen().is_empty(),
        "nothing from another process reaches the server"
    );
    relay.stop().await;
    let _ = other.kill();
    let _ = other.wait();

    // The same relay with this process registered as the game
    let me = std::process::id();
    let mut options = options_for(&upstream, None);
    options.authorize = relay_owner::game_owner_check(Arc::new(move || Some(me)));
    let relay = Relay::start(options).await.unwrap();
    assert_eq!(call(relay.port(), "GET", "/echo", &[], None).await.status, 200);
    let (_, echo) = websocket(relay.port(), "/ws", "").await;
    assert_eq!(echo.as_deref(), Some("echo:hello"));
    relay.stop().await;
}

#[cfg(not(windows))]
#[tokio::test]
async fn elsewhere_the_relay_serves_loopback_only_without_an_owner_check() {
    let upstream = upstream().await;
    let mut options = options_for(&upstream, None);
    options.authorize = relay_owner::game_owner_check(Arc::new(|| Some(1)));
    assert!(options.authorize.is_none(), "no owner check until RA10");
    let relay = Relay::start(options).await.unwrap();
    assert_eq!(call(relay.port(), "GET", "/echo", &[], None).await.status, 200);
    assert_eq!(
        call(relay.port(), "GET", "/echo", &[("host", "evil.example")], None)
            .await
            .status,
        403
    );
    relay.stop().await;
}

#[tokio::test]
async fn a_taken_port_a_server_that_is_down_and_stop_freeing_the_port() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let mut second = options_for(&upstream, None);
    second.port = relay.port();
    assert_eq!(
        Relay::start(second).await.err(),
        Some(RelayError::PortBusy(relay.port()))
    );
    relay.stop().await;

    // Something that is not a relay at all holding the port
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut options = RelayOptions::new(RelayTarget {
        host: "127.0.0.1".into(),
        port: 1,
        fingerprint: Some("0".repeat(64)),
    });
    options.port = port;
    options.qos_port = None;
    assert_eq!(
        Relay::start(options.clone()).await.err(),
        Some(RelayError::PortBusy(port))
    );
    drop(taken);
    options.connect_timeout = Duration::from_millis(500);
    let relay = Relay::start(options.clone()).await.unwrap();
    let answer = call(port, "GET", "/anything", &[], None).await;
    assert_eq!(
        (answer.status, json(&answer)),
        (502, serde_json::json!({ "error": "upstream_unreachable" }))
    );
    assert_eq!(relay.failures(), 1);
    relay.stop().await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    // The port can be used again
    Relay::start(options).await.unwrap().stop().await;
}

#[test]
fn forwarded_headers_drop_hop_by_hop_and_connection_named_ones() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("host", "a"),
        ("connection", "Upgrade, X-Named"),
        ("upgrade", "websocket"),
        ("x-named", "1"),
        ("keep-alive", "timeout=5"),
        ("x-keep", "2"),
        ("transfer-encoding", "chunked"),
        ("http2-settings", "x"),
    ] {
        headers.append(name, HeaderValue::from_static(value));
    }
    let kept = forwarded(&headers);
    let kept: Vec<&str> = kept.keys().map(|name| name.as_str()).collect();
    assert_eq!(kept, ["x-keep"]);
}

#[test]
fn local_request_rules() {
    let local: SocketAddr = "127.0.0.1:5000".parse().unwrap();
    let headers = |pairs: &[(&'static str, &'static str)]| {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    };
    for host in [
        "127.0.0.1:61100",
        "[::1]:61100",
        "localhost",
        "LOCALHOST:1",
        "127.1.2.3",
    ] {
        assert_eq!(
            local_request_problem(local, &headers(&[("host", host)]), false),
            None,
            "{host}"
        );
    }
    assert_eq!(
        local_request_problem(
            "[::ffff:127.0.0.1]:5".parse().unwrap(),
            &headers(&[("host", "127.0.0.1")]),
            false
        ),
        None
    );
    assert_eq!(
        local_request_problem("[::1]:5".parse().unwrap(), &headers(&[("host", "127.0.0.1")]), false),
        None
    );
    assert_eq!(
        local_request_problem("10.0.0.2:5".parse().unwrap(), &headers(&[("host", "127.0.0.1")]), false),
        Some("not_local")
    );
    assert_eq!(local_request_problem(local, &headers(&[]), false), Some("host"));
    for host in ["evil.example", "127.0.0.1.evil.example", "127.0.0.1:x", "127.0.0", ""] {
        assert_eq!(
            local_request_problem(local, &headers(&[("host", host)]), false),
            Some("host"),
            "{host}"
        );
    }
    assert_eq!(
        local_request_problem(
            local,
            &headers(&[("host", "127.0.0.1"), ("sec-fetch-site", "x")]),
            false
        ),
        Some("browser")
    );
    assert_eq!(
        local_request_problem(local, &headers(&[("host", "127.0.0.1"), ("sec-fetch-site", "x")]), true),
        None
    );
    for origin in [
        "http://127.0.0.1",
        "http://localhost:61100/",
        "127.0.0.1",
        "http://[::1]:5",
    ] {
        let mut map = headers(&[("host", "127.0.0.1")]);
        map.insert("origin", HeaderValue::from_str(origin).unwrap());
        assert_eq!(local_request_problem(local, &map, true), None, "{origin}");
    }
    for origin in [
        "https://evil.example",
        "null",
        "http://127.0.0.1.evil.example",
        "http://127.0.0.1/path",
        "http://[]",
        "",
    ] {
        let mut map = headers(&[("host", "127.0.0.1")]);
        map.insert("origin", HeaderValue::from_str(origin).unwrap());
        assert_eq!(local_request_problem(local, &map, true), Some("origin"), "{origin}");
    }
    let mut map = headers(&[("host", "127.0.0.1")]);
    map.insert("origin", HeaderValue::from_bytes(b"http://127.0.0.1\xff").unwrap());
    assert_eq!(local_request_problem(local, &map, false), Some("origin"));
}

#[test]
fn qos_paths_and_the_port_override() {
    for path in ["/qos", "/QoS", "/QOS/", "/qos/x", "/qos?x=1"] {
        assert!(is_qos_path(path), "{path}");
    }
    for path in ["/", "/qosx", "/qo", "qos", "/api/qos", "//qos", "/q\u{f6}s"] {
        assert!(!is_qos_path(path), "{path}");
    }
    assert_eq!(QOS_PONG, "<!DOCTYPE html><html><body>pong</body></html>");
    assert_eq!(DEFAULT_RELAY_PORT, 61100);
    assert_eq!(relay_port_override(Some(" 62000 ")), Some(62000));
    assert_eq!(relay_port_override(Some("1024")), Some(1024));
    for bad in ["80", "0999", "70000", "6100a", "", "1023"] {
        assert_eq!(relay_port_override(Some(bad)), None, "{bad}");
    }
    assert_eq!(relay_port_override(None), None);
}
