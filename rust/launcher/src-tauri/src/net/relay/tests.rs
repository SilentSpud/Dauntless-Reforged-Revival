//! The relay against a local TLS "gateway" with a throwaway certificate, as `test/relay.test.ts`.

use std::sync::Mutex;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::*;
use crate::net::pinned::certificate_fingerprint;
use crate::net::pinned::tests::{acceptor, certificate};

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
        "/echo" => json(serde_json::json!({ "method": request.method().as_str(), "url": request.uri().to_string() })),
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
        "/head" => {
            let mut response = Response::new(empty());
            response
                .headers_mut()
                .insert(CONTENT_LENGTH, HeaderValue::from_static("1234"));
            response
        }
        "/stream" => {
            let (tx, rx) = tokio::sync::mpsc::channel::<
                Result<hyper::body::Frame<Bytes>, Box<dyn std::error::Error + Send + Sync>>,
            >(4);
            tokio::spawn(async move {
                for (delay, part) in [(0, "one|"), (300, "two|"), (300, "three")] {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    let _ = tx.send(Ok(hyper::body::Frame::data(Bytes::from(part)))).await;
                }
            });
            Response::new(http_body_util::StreamBody::new(tokio_stream::wrappers::ReceiverStream::new(rx)).boxed())
        }
        _ => {
            let mut response = Response::new(empty());
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
        }
    }
}

async fn upstream() -> Upstream {
    let (cert, key) = certificate();
    let fingerprint = certificate_fingerprint(&cert);
    let tls = acceptor(cert, key);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (tls, log) = (tls.clone(), log.clone());
            tokio::spawn(async move {
                let Ok(stream) = tls.accept(tcp).await else { return };
                let service = service_fn(move |request| {
                    let log = log.clone();
                    async move { Ok::<_, Infallible>(route(request, log).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    Upstream {
        port,
        fingerprint,
        seen,
    }
}

async fn relay_for(upstream: &Upstream, fingerprint: Option<&str>) -> (Relay, Arc<Mutex<Vec<Option<String>>>>) {
    let pin_errors = Arc::new(Mutex::new(Vec::new()));
    let errors = pin_errors.clone();
    let mut options = RelayOptions::new(PinnedTarget {
        host: "127.0.0.1".into(),
        port: upstream.port,
        fingerprint: fingerprint.unwrap_or(&upstream.fingerprint).to_owned(),
    });
    options.port = 0;
    options.on_pin_error = Some(Arc::new(move |actual| errors.lock().unwrap().push(actual)));
    (Relay::start(options).await.unwrap(), pin_errors)
}

struct Answer {
    status: u16,
    reason: Option<String>,
    headers: HeaderMap,
    body: Bytes,
}

/// One request to the relay on a connection of its own.
async fn call(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: Option<Bytes>) -> Answer {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(connection);
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, format!("127.0.0.1:{port}"));
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

#[tokio::test]
async fn forwards_method_path_query_headers_and_body_over_the_pinned_connection() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let answer = call(
        relay.port(),
        "POST",
        "/echo?x=1&y=%20",
        &[
            ("x-undaunted-user-api-key", "UUK_x"),
            ("connection", "keep-alive, x-drop"),
            ("x-drop", "1"),
        ],
        Some(Bytes::from("hi")),
    )
    .await;
    assert_eq!(answer.status, 200);
    assert_eq!(
        json(&answer),
        serde_json::json!({ "method": "POST", "url": "/echo?x=1&y=%20" })
    );
    let seen = upstream.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(seen.headers.get(HOST).unwrap(), &format!("127.0.0.1:{}", upstream.port));
    assert_eq!(seen.headers.get("x-undaunted-user-api-key").unwrap(), "UUK_x");
    assert!(
        !seen.headers.contains_key("x-drop"),
        "a header the Connection header names never crosses"
    );
    let body = call(
        relay.port(),
        "POST",
        "/body",
        &[],
        Some(Bytes::from(vec![7u8; 100_000])),
    )
    .await;
    let body = json(&body);
    assert_eq!(
        (body["length"].as_u64(), body["cl"].as_str()),
        (Some(100_000), Some("100000"))
    );
    relay.stop().await;
}

#[tokio::test]
async fn response_reason_repeated_headers_and_no_hop_by_hop() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    let answer = call(relay.port(), "GET", "/headers", &[], None).await;
    assert_eq!((answer.status, answer.reason.as_deref()), (200, Some("Fine Thanks")));
    assert_eq!(answer.headers.get_all("set-cookie").iter().count(), 2);
    assert_eq!(answer.headers.get("x-custom").unwrap(), "yes");
    assert!(!answer.headers.contains_key("keep-alive"));
    assert!(!answer.headers.contains_key("x-private"));
    assert_eq!(&answer.body[..], b"ok");
    let head = call(relay.port(), "HEAD", "/head", &[], None).await;
    assert_eq!(
        (
            head.headers.get(CONTENT_LENGTH).unwrap().to_str().unwrap(),
            head.body.len()
        ),
        ("1234", 0)
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
    while body.frame().await.is_some() {}
    for _ in 0..3 {
        assert_eq!(call(relay.port(), "GET", "/echo", &[], None).await.status, 200);
    }
    assert!(relay.tls_connections() <= 2, "kept alive: {}", relay.tls_connections());
    relay.stop().await;
}

/// The WebSocket handshake and one echoed frame, over a raw socket to the relay.
async fn websocket(port: u16, path: &str, extra: &str) -> (String, Option<String>) {
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let handshake = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    );
    tcp.write_all(handshake.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if tcp.read(&mut byte).await.unwrap() == 0 {
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
    // As libwebsockets 3.0 sends it: GET //, a loopback Origin without a port, protocol xmpp
    let (head, echo) = websocket(
        relay.port(),
        "//",
        "Origin: http://127.0.0.1\r\nSec-WebSocket-Protocol: xmpp\r\n",
    )
    .await;
    assert!(
        head.to_ascii_lowercase().contains("sec-websocket-protocol: xmpp"),
        "{head}"
    );
    assert_eq!(echo.as_deref(), Some("echo:hello"));
    let seen = upstream.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!((seen.method.as_str(), seen.uri.as_str()), ("GET", "//"));
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
        "GET",
        "/echo",
        &[("x-undaunted-user-api-key", "UUK_secret")],
        None,
    )
    .await;
    assert_eq!(
        (answer.status, json(&answer)["error"].as_str()),
        (502, Some("certificate_mismatch"))
    );
    let (head, _) = websocket(relay.port(), "/ws", "").await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert!(
        upstream.seen.lock().unwrap().is_empty(),
        "no request reached the server"
    );
    assert_eq!(
        pin_errors.lock().unwrap().clone(),
        vec![Some(upstream.fingerprint.clone()); 2]
    );
    relay.stop().await;
}

#[tokio::test]
async fn requests_and_websockets_from_web_pages_are_refused() {
    let upstream = upstream().await;
    let (relay, _) = relay_for(&upstream, None).await;
    for headers in [
        vec![("sec-fetch-mode", "cors")],
        vec![("origin", "https://evil.example")],
    ] {
        let answer = call(relay.port(), "GET", "/echo", &headers, None).await;
        assert_eq!(answer.status, 403, "{headers:?}");
    }
    let tcp = TcpStream::connect(("127.0.0.1", relay.port())).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(connection);
    let rebinding = Request::builder()
        .uri("/echo")
        .header(HOST, "evil.example:61000")
        .body(Empty::new())
        .unwrap();
    assert_eq!(sender.send_request(rebinding).await.unwrap().status(), 403);
    let (head, _) = websocket(relay.port(), "/ws", "Origin: https://evil.example\r\n").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(upstream.seen.lock().unwrap().is_empty());
    relay.stop().await;
}

#[tokio::test]
async fn the_owner_check_denies_before_forwarding() {
    let upstream = upstream().await;
    let mut options = RelayOptions::new(PinnedTarget {
        host: "127.0.0.1".into(),
        port: upstream.port,
        fingerprint: upstream.fingerprint.clone(),
    });
    options.port = 0;
    options.authorize = Some(Arc::new(|_, _| Box::pin(async { false })));
    let relay = Relay::start(options).await.unwrap();
    let answer = call(relay.port(), "GET", "/echo", &[], None).await;
    assert_eq!(
        (answer.status, json(&answer)["error"].as_str()),
        (403, Some("local_process_denied"))
    );
    let (head, _) = websocket(relay.port(), "/ws", "").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(upstream.seen.lock().unwrap().is_empty());
    relay.stop().await;
}

#[tokio::test]
async fn a_taken_port_a_server_that_is_down_and_stop_freeing_the_port() {
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut options = RelayOptions::new(PinnedTarget {
        host: "127.0.0.1".into(),
        port: 1,
        fingerprint: "0".repeat(64),
    });
    options.port = port;
    assert_eq!(
        Relay::start(options.clone()).await.err(),
        Some(RelayError::PortBusy(port))
    );
    drop(taken);
    options.connect_timeout = Duration::from_millis(500);
    let relay = Relay::start(options.clone()).await.unwrap();
    let answer = call(port, "GET", "/x", &[], None).await;
    assert_eq!(
        (answer.status, json(&answer)["error"].as_str()),
        (502, Some("upstream_unreachable"))
    );
    relay.stop().await;
    Relay::start(options).await.unwrap().stop().await;
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
    assert_eq!(
        local_request_problem(local, &headers(&[("host", "127.0.0.1:61000")]), false),
        None
    );
    assert_eq!(
        local_request_problem(local, &headers(&[("host", "[::1]:61000")]), false),
        None
    );
    assert_eq!(
        local_request_problem(local, &headers(&[("host", "localhost")]), false),
        None
    );
    assert_eq!(
        local_request_problem(
            "[::ffff:127.0.0.1]:5".parse().unwrap(),
            &headers(&[("host", "127.0.0.1")]),
            false
        ),
        None
    );
    assert_eq!(
        local_request_problem("10.0.0.2:5".parse().unwrap(), &headers(&[("host", "127.0.0.1")]), false),
        Some("not_local")
    );
    assert_eq!(local_request_problem(local, &headers(&[]), false), Some("host"));
    assert_eq!(
        local_request_problem(local, &headers(&[("host", "evil.example")]), false),
        Some("host")
    );
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
        "http://localhost:61000/",
        "127.0.0.1",
        "http://[::1]:5",
    ] {
        let map = headers(&[("host", "127.0.0.1")]);
        let mut map = map;
        map.insert("origin", HeaderValue::from_str(origin).unwrap());
        assert_eq!(local_request_problem(local, &map, true), None, "{origin}");
    }
    for origin in [
        "https://evil.example",
        "null",
        "http://127.0.0.1.evil.example",
        "http://127.0.0.1/path",
    ] {
        let mut map = headers(&[("host", "127.0.0.1")]);
        map.insert("origin", HeaderValue::from_str(origin).unwrap());
        assert_eq!(local_request_problem(local, &map, true), Some("origin"), "{origin}");
    }
    assert_eq!(relay_port_override(Some(" 62000 ")), Some(62000));
    for bad in ["80", "0999", "70000", "6100a", ""] {
        assert_eq!(relay_port_override(Some(bad)), None, "{bad}");
    }
}
