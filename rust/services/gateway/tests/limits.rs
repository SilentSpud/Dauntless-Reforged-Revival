#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The gateway's limits (`test/limits.test.ts`): body caps, rate limits, connection limits,
//! timeouts and graceful shutdown.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

async fn upstreams() -> (Upstream, Upstream, Ports) {
    let handler: Handler = Arc::new(|seen: Seen| {
        Box::pin(async move {
            match seen.url.as_str() {
                "/slow" => {
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    Some((
                        200,
                        vec![("content-type", "text/plain".to_owned())],
                        b"slow done".to_vec(),
                    ))
                }
                "/hang" => std::future::pending().await,
                _ => None,
            }
        })
    });
    let meta = start_upstream("metagame", Some(handler)).await;
    let content = start_upstream("content", None).await;
    let ports = Ports {
        metagame: meta.port,
        content: content.port,
        ws: dead_port().await,
    };
    (meta, content, ports)
}

fn get() -> Options<'static> {
    Options::default()
}

fn from(local: &'static str) -> Options<'static> {
    Options {
        local: Some(local),
        ..Options::default()
    }
}

/// A raw request over a fresh TLS connection; the reply's status and body, even if the gateway
/// hangs up while the request is still being written.
async fn raw_exchange(port: u16, head: &str, chunks: Vec<Vec<u8>>) -> (u16, String) {
    let stream = tls_connect(port, None).await.unwrap();
    let (mut reader, mut writer) = tokio::io::split(stream);
    // Read while writing: the gateway answers and closes before the body is all sent, and on Windows
    // that close resets the connection, which drops whatever reply the client has not read yet
    let write = async move {
        if writer.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        for chunk in chunks {
            if writer.write_all(&chunk).await.is_err() {
                return;
            }
        }
        // Keep the write half open until the reply is read
        tokio::time::sleep(Duration::from_secs(6)).await;
    };
    let mut data = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut data));
    tokio::select! {
        _ = read => {}
        () = write => {}
    }
    let text = String::from_utf8_lossy(&data).into_owned();
    let status = text.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// A chunked body of `total` bytes in pieces of `size`.
fn chunked(total: usize, size: usize) -> Vec<Vec<u8>> {
    let mut chunks: Vec<Vec<u8>> = (0..total / size)
        .map(|_| {
            let mut chunk = format!("{size:x}\r\n").into_bytes();
            chunk.extend(std::iter::repeat_n(b'a', size));
            chunk.extend_from_slice(b"\r\n");
            chunk
        })
        .collect();
    chunks.push(b"0\r\n\r\n".to_vec());
    chunks
}

const CHUNKED_HEAD: &str = "POST /character HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";

#[tokio::test(flavor = "multi_thread")]
async fn request_bodies() {
    let _serial = serial().await;
    let cert = make_cert();
    let (meta, _content, ports) = upstreams().await;
    let config = test_config(
        &cert,
        &ports,
        &[
            ("GATEWAY_MAX_BODY_BYTES", "4096"),
            ("GATEWAY_RATE_GENERAL", "1000,6000"),
        ],
    );
    let running = start_gateway(&cert, config, None).await;
    let port = running.port;

    // A Content-Length over the cap: 413 before reading it, and the upstream never sees it
    let before = meta.count();
    let over = Options {
        method: Some("POST"),
        body: Some(vec![b'a'; 4097]),
        headers: vec![("content-type", "application/json".into())],
        ..get()
    };
    let reply = https_request(port, "/character", over).await.unwrap();
    assert_eq!(
        (reply.status, reply.json()),
        (413, json!({ "error": "body_too_large" }))
    );
    assert_eq!(reply.headers.get("connection").map(String::as_str), Some("close"));
    assert_eq!(meta.count(), before);

    // Exactly the cap passes
    let exact = Options {
        method: Some("POST"),
        body: Some(vec![b'a'; 4096]),
        ..get()
    };
    let reply = https_request(port, "/character", exact).await.unwrap();
    assert_eq!((reply.status, reply.json()["bodyLength"].as_u64()), (200, Some(4096)));

    // A chunked body is cut at the cap with 413, and the upstream request is aborted
    let before = meta.count();
    let start = log_len();
    let (status, body) = raw_exchange(port, CHUNKED_HEAD, chunked(64 * 1024, 1024)).await;
    assert_eq!(status, 413, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({ "error": "body_too_large" })
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    for seen in &meta.seen()[before..] {
        assert!(
            seen.aborted || seen.body.len() <= 4096,
            "the upstream got more than the cap"
        );
    }
    let cut = log_since(start).into_iter().any(|(_, v)| {
        v["msg"] == "request" && v["reason"] == "body_too_large" && v["bytesIn"].as_u64().unwrap_or(0) > 4096
    });
    assert!(cut, "no access-log line for the cut body");

    // Expect: 100-continue is answered by the gateway: 413 without 100 over the cap, 100 then the
    // answer otherwise
    let expect = |length: usize| async move {
        let mut stream = tls_connect(port, None).await.unwrap();
        let head = format!(
            "POST /character HTTP/1.1\r\nHost: 127.0.0.1\r\nExpect: 100-continue\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        let (mut status, _, mut rest) = read_head(&mut stream).await;
        let continued = status == 100;
        if continued {
            stream.write_all(&vec![b'b'; length]).await.unwrap();
            let (final_status, _, after) = read_head(&mut stream).await;
            status = final_status;
            rest = after;
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
        (status, continued, String::from_utf8_lossy(&rest).into_owned())
    };
    let (status, continued, _) = expect(5000).await;
    assert_eq!((status, continued), (413, false));
    let (status, continued, body) = expect(100).await;
    assert_eq!((status, continued), (200, true));
    let echo: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(echo["bodyLength"], 100);
    assert!(echo["headers"].get("expect").is_none());

    // A chunked body under the cap goes through
    let (status, body) = raw_exchange(port, CHUNKED_HEAD, chunked(3000, 1000)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["bodyLength"], 3000);
    running.close().await;
}

async fn upgrade_status(port: u16, target: &str, local: &'static str) -> u16 {
    upgrade(port, target, &[("Sec-WebSocket-Protocol", "xmpp")], Some(local))
        .await
        .status
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limits() {
    let _serial = serial().await;
    let cert = make_cert();
    let (meta, _content, ports) = upstreams().await;
    let ws = start_ws_upstream().await;
    let ports = Ports { ws: ws.port, ..ports };
    let config = test_config(
        &cert,
        &ports,
        &[
            ("GATEWAY_RATE_GENERAL", "5,60"),
            ("GATEWAY_RATE_CONTENT", "3,60"),
            ("GATEWAY_RATE_REGISTER", "2,0.2"),
            ("GATEWAY_RATE_TOKEN", "2,1"),
            ("GATEWAY_RATE_WS", "3,1"),
        ],
    );
    let running = start_gateway(&cert, config, None).await;
    let port = running.port;
    let status = |target: &'static str, options: Options<'static>| async move {
        https_request(port, target, options).await.unwrap()
    };

    // A burst per address, then 429 with Retry-After without asking the upstream
    for _ in 0..5 {
        assert_eq!(status("/party/invites", from("127.0.0.20")).await.status, 200);
    }
    let before = meta.count();
    let limited = status("/party/invites", from("127.0.0.20")).await;
    assert_eq!(
        (limited.status, limited.json()),
        (429, json!({ "error": "rate_limited" }))
    );
    assert_eq!(limited.headers["retry-after"], "1");
    assert_eq!(meta.count(), before);
    // Another address is not affected
    assert_eq!(status("/party/invites", from("127.0.0.21")).await.status, 200);
    // A refill of one per second lets one more through
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(status("/party/invites", from("127.0.0.20")).await.status, 200);
    assert_eq!(status("/party/invites", from("127.0.0.20")).await.status, 429);

    // Downloads count in their own bucket
    for _ in 0..3 {
        assert_eq!(status("/content/v1/manifest", from("127.0.0.20")).await.status, 200);
    }
    assert_eq!(status("/content/v1/manifest", from("127.0.0.20")).await.status, 429);

    // Register is limited strictly
    let register = || Options {
        method: Some("POST"),
        body: Some(br#"{"Username":"Someone","InviteCode":"ABCD-EFGH-JKMN"}"#.to_vec()),
        local: Some("127.0.0.22"),
        ..get()
    };
    assert_eq!(status("/undaunted/api/Register", register()).await.status, 200);
    assert_eq!(status("/undaunted/api/register", register()).await.status, 200);
    let limited = status("/undaunted/api/Register", register()).await;
    assert_eq!((limited.status, limited.headers["retry-after"].as_str()), (429, "300"));
    assert_eq!(
        status("/undaunted/api/ServerStatus", from("127.0.0.22")).await.status,
        200
    );

    // So is the login token route
    let start = log_len();
    let form = || Options {
        method: Some("POST"),
        body: Some(b"grant_type=exchange_code&exchange_code=x".to_vec()),
        headers: vec![("content-type", "application/x-www-form-urlencoded".into())],
        local: Some("127.0.0.23"),
        ..get()
    };
    assert_eq!(status("/account/api/oauth/token", form()).await.status, 200);
    assert_eq!(status("/account/api/oauth/token", form()).await.status, 200);
    let limited = status("/account/api/oauth/token", form()).await;
    assert_eq!((limited.status, limited.headers["retry-after"].as_str()), (429, "60"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let line = log_since(start)
        .into_iter()
        .find(|(_, v)| v["status"] == 429 && v["reason"] == "rate_limited:token")
        .expect("no log line");
    assert_eq!(line.1["ip"], "127.0.0.23");

    // WebSocket upgrades count in their own bucket: chat reconnects never 429 the game's HTTP
    let start = log_len();
    for _ in 0..3 {
        assert_ne!(upgrade_status(port, "//", "127.0.0.24").await, 429);
    }
    assert_eq!(
        upgrade_status(port, "//", "127.0.0.24").await,
        429,
        "the fourth upgrade is over the ws bucket"
    );
    let heartbeat = Options {
        method: Some("POST"),
        body: Some(b"{}".to_vec()),
        local: Some("127.0.0.24"),
        ..get()
    };
    assert_eq!(
        status("/heartbeat", heartbeat).await.status,
        200,
        "the general bucket is untouched"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        log_since(start)
            .iter()
            .any(|(_, v)| v["status"] == 429 && v["reason"] == "rate_limited:ws")
    );

    // And an empty general bucket still lets chat connect
    for _ in 0..5 {
        assert_eq!(status("/party/invites", from("127.0.0.25")).await.status, 200);
    }
    assert_eq!(status("/party/invites", from("127.0.0.25")).await.status, 429);
    assert_ne!(upgrade_status(port, "/xmpp", "127.0.0.25").await, 429);
    running.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connections() {
    let _serial = serial().await;
    let cert = make_cert();
    let (_meta, _content, ports) = upstreams().await;
    let config = test_config(
        &cert,
        &ports,
        &[
            ("GATEWAY_MAX_CONNECTIONS_PER_IP", "3"),
            ("GATEWAY_RATE_CONNECT", "8,60"),
        ],
    );
    let running = start_gateway(&cert, config, None).await;
    let port = running.port;
    let start = log_len();

    // At most three open connections per address
    let mut open = Vec::new();
    for _ in 0..3 {
        open.push(tls_connect(port, Some("127.0.0.30")).await.unwrap());
    }
    assert!(tls_connect(port, Some("127.0.0.30")).await.is_err());
    // Someone else still gets in
    drop(tls_connect(port, Some("127.0.0.31")).await.unwrap());
    // Closing one frees a slot
    drop(open.remove(0));
    tokio::time::sleep(Duration::from_millis(100)).await;
    open.push(tls_connect(port, Some("127.0.0.30")).await.unwrap());
    drop(open);
    assert!(
        log_since(start)
            .iter()
            .any(|(_, v)| v["msg"] == "connection refused: too many open connections from one address")
    );

    // How fast one address opens new connections
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (mut opened, mut refused) = (0, 0);
    for _ in 0..10 {
        match tls_connect(port, Some("127.0.0.32")).await {
            Ok(stream) => {
                opened += 1;
                drop(stream);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(_) => refused += 1,
        }
    }
    assert_eq!((opened, refused), (8, 2));
    running.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn timeouts() {
    let cert = make_cert();
    let (meta, _content, ports) = upstreams().await;
    let config = test_config(
        &cert,
        &ports,
        &[
            ("GATEWAY_HEADERS_TIMEOUT_MS", "1000"),
            ("GATEWAY_REQUEST_TIMEOUT_MS", "2000"),
            ("GATEWAY_HANDSHAKE_TIMEOUT_MS", "1000"),
            ("GATEWAY_KEEPALIVE_TIMEOUT_MS", "1000"),
            ("GATEWAY_IDLE_TIMEOUT_MS", "3000"),
        ],
    );
    let running = start_gateway(&cert, config, None).await;
    let port = running.port;

    // A client that sends its headers too slowly (slowloris) is dropped with a 408
    let mut stream = tls_connect(port, None).await.unwrap();
    stream
        .write_all(b"GET /party/invites HTTP/1.1\r\nHost: 127.0.0.1\r\n")
        .await
        .unwrap();
    let (reader, mut writer) = tokio::io::split(stream);
    let drip = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if writer.write_all(b"X-Slow: 1\r\n").await.is_err() {
                return;
            }
        }
    });
    let mut reader = reader;
    let (took, received) = wait_close(&mut reader, Duration::from_secs(5)).await;
    drip.abort();
    assert!(
        took >= Duration::from_millis(800) && took < Duration::from_secs(3),
        "closed after {took:?}"
    );
    assert!(
        received.starts_with(b"HTTP/1.1 408"),
        "{}",
        String::from_utf8_lossy(&received)
    );

    // A client that sends its body too slowly is dropped, and the upstream request with it
    let before = meta.count();
    let mut stream = tls_connect(port, None).await.unwrap();
    stream
        .write_all(b"POST /character HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n")
        .await
        .unwrap();
    let (mut reader, mut writer) = tokio::io::split(stream);
    let drip = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if writer.write_all(b"aaaaa").await.is_err() {
                return;
            }
        }
    });
    let (took, _) = wait_close(&mut reader, Duration::from_secs(6)).await;
    drip.abort();
    assert!(
        took >= Duration::from_millis(1800) && took < Duration::from_millis(4500),
        "closed after {took:?}"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        meta.seen()[before..].iter().all(|s| s.aborted),
        "the upstream request was not aborted"
    );

    // A connection that never finishes the TLS handshake is dropped
    let mut tcp = tcp_connect(port, None).await.unwrap();
    let (took, _) = wait_close(&mut tcp, Duration::from_secs(5)).await;
    assert!(took < Duration::from_millis(3500), "closed after {took:?}");

    // Idle keep-alive connections are closed
    let mut stream = tls_connect(port, None).await.unwrap();
    stream
        .write_all(b"GET /party/invites HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    let (took, received) = wait_close(&mut stream, Duration::from_secs(5)).await;
    assert!(
        received.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&received)
    );
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_millis(3500),
        "closed after {took:?}"
    );
    running.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn graceful_shutdown() {
    let cert = make_cert();
    let (_meta, _content, ports) = upstreams().await;

    // A request in flight finishes with Connection: close, new connections are refused, then it stops
    let config = test_config(&cert, &ports, &[("GATEWAY_SHUTDOWN_GRACE_MS", "5000")]);
    let mut running = start_gateway(&cert, config, None).await;
    let port = running.port;
    let in_flight = tokio::spawn(https_request(port, "/slow", get()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    let (stop, task) = running.stopper();
    stop.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(https_request(port, "/party/invites", get()).await.is_err());
    let reply = in_flight.await.unwrap().unwrap();
    assert_eq!((reply.status, reply.text().as_str()), (200, "slow done"));
    assert_eq!(reply.headers.get("connection").map(String::as_str), Some("close"));
    task.await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "close waited for the full grace period"
    );

    // What is still running when the grace period ends is cut
    let config = test_config(&cert, &ports, &[("GATEWAY_SHUTDOWN_GRACE_MS", "500")]);
    let mut running = start_gateway(&cert, config, None).await;
    let port = running.port;
    let hanging = tokio::spawn(https_request(port, "/hang", get()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    let (stop, task) = running.stopper();
    stop.send(true).unwrap();
    task.await.unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_millis(2500),
        "closed after {took:?}"
    );
    assert!(hanging.await.unwrap().is_err(), "the hanging request was answered");
}
