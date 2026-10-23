#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The gateway's parity with Reforged's Node one (`legacy/ReforgedGateway`, MIGRATION_PLAN.md RS3): each
//! `tests/parity/<name>.requests.json` script runs against the Rust gateway, and every reply's
//! status, headers, body and access-log lines must be what the Node gateway gave
//! (`<name>.expected.json`, written by `tests/parity/record.cjs` against the same fake upstreams).
//! Requests go out as raw HTTP over TLS, each on its own connection.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use common::*;
use gateway::feed::AllowlistFeed;
use http_body_util::{Full, StreamBody};
use hyper_util::rt::TokioIo;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SECRET: &str = "parity-gateway-secret-0123456789abcdef0123456789abcdef";
const HELPER_SECRET: &str = "parity-helper-secret-0123456789abcdef0123456789abcdef";

/// Kept in step with HEADERS in record.cjs.
const HEADERS: [&str; 12] = [
    "content-type",
    "content-length",
    "transfer-encoding",
    "cache-control",
    "connection",
    "retry-after",
    "x-powered-by",
    "x-reforged-gateway",
    "set-cookie",
    "upgrade",
    "sec-websocket-accept",
    "sec-websocket-protocol",
];

type Body = http_body_util::Either<
    Full<Bytes>,
    StreamBody<
        futures_util::stream::Iter<std::vec::IntoIter<Result<http_body::Frame<Bytes>, std::convert::Infallible>>>,
    >,
>;

fn full(status: u16, headers: &[(&str, &str)], body: &str) -> http::Response<Body> {
    let mut response = http::Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(*name, *value);
    }
    response
        .body(http_body_util::Either::Left(Full::new(Bytes::from(body.to_owned()))))
        .unwrap()
}

/// Kept in step with echo() in record.cjs.
fn echo(upstream: &str, method: &str, url: &str, headers: &http::HeaderMap, body: &[u8]) -> String {
    let mut seen = BTreeMap::new();
    for (name, value) in header_object(headers) {
        if name != "connection" {
            seen.insert(name, value);
        }
    }
    json!({
        "upstream": upstream,
        "method": method,
        "url": url,
        "headers": seen,
        "bodyLength": body.len(),
        "bodySha": sha256_hex(body),
    })
    .to_string()
}

/// The fake backend, content server and allowlist helper (kept in step with record.cjs).
async fn upstream(label: &'static str, allowed: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let allowed = allowed.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                    let allowed = allowed.clone();
                    async move {
                        use http_body_util::BodyExt;
                        let (parts, body) = request.into_parts();
                        let body = body.collect().await.map(|b| b.to_bytes().to_vec()).unwrap_or_default();
                        let url = parts.uri.to_string();
                        let path = url.split('?').next().unwrap_or("").to_owned();
                        let method = parts.method.as_str();
                        let response = match (label, path.as_str()) {
                            ("helper", _) => {
                                let ip: Value = serde_json::from_slice(&body).unwrap();
                                allowed.lock().unwrap().push(ip["ip"].as_str().unwrap().to_owned());
                                full(200, &[("content-type", "application/json")], "{}")
                            }
                            ("backend", status) if status.starts_with("/status/") => {
                                let code: u16 = status["/status/".len()..].parse().unwrap();
                                let frames: Vec<Result<http_body::Frame<Bytes>, std::convert::Infallible>> =
                                    if code == 204 {
                                        Vec::new()
                                    } else {
                                        vec![Ok(http_body::Frame::data(Bytes::from(format!("status {code}"))))]
                                    };
                                // No length: Node's res.end(text) without one is chunked
                                http::Response::builder()
                                    .status(code)
                                    .header("content-type", "text/plain")
                                    .header("x-powered-by", "Express")
                                    .header("x-reforged-gateway", "leak")
                                    .header("set-cookie", "a=1")
                                    .header("set-cookie", "b=2")
                                    .body(http_body_util::Either::Right(StreamBody::new(
                                        futures_util::stream::iter(frames),
                                    )))
                                    .unwrap()
                            }
                            // Paradox's POST /login answers 2xx after its own bearer check;
                            // /game/login is DR's route
                            ("backend", "/login" | "/game/login") => {
                                full(200, &[("content-type", "application/json")], "{}")
                            }
                            ("backend", "/heartbeat") => {
                                let ok = parts
                                    .headers
                                    .get("authorization")
                                    .is_some_and(|v| v == "Bearer good-token");
                                full(
                                    if ok { 200 } else { 401 },
                                    &[("content-type", "text/plain")],
                                    if ok { "20000" } else { "" },
                                )
                            }
                            ("backend", "/account/api/oauth/token") => {
                                full(200, &[("content-type", "application/json")], r#"{"access_token":"x"}"#)
                            }
                            _ => full(
                                200,
                                &[("content-type", "application/json"), ("x-powered-by", "Express")],
                                &echo(label, method, &url, &parts.headers, &body),
                            ),
                        };
                        Ok::<_, std::convert::Infallible>(response)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    port
}

/// Kept in step with parse() in record.cjs.
fn parse(label: &str, data: &[u8]) -> Value {
    let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
        let text: String = data.iter().map(|b| char::from(*b)).collect();
        return json!({ "request": label, "status": 0, "headers": {}, "body": text });
    };
    let head: String = data[..end].iter().map(|b| char::from(*b)).collect();
    let mut lines = head.split("\r\n");
    let status: u64 = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    for line in lines {
        let (key, value) = line.split_once(':').unwrap();
        let (key, value) = (key.trim().to_lowercase(), value.trim().to_owned());
        found
            .entry(key)
            .and_modify(|v| {
                v.push_str(", ");
                v.push_str(&value);
            })
            .or_insert(value);
    }
    let mut headers = Map::new();
    for key in HEADERS {
        if let Some(value) = found.get(key) {
            let value = if key == "connection" || key == "upgrade" {
                value.to_lowercase()
            } else {
                value.clone()
            };
            headers.insert(key.into(), json!(value));
        }
    }
    let mut body = data[end + 4..].to_vec();
    if found
        .get("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        let mut parts = Vec::new();
        let mut at = 0;
        while at < body.len() {
            let Some(line_end) = body[at..].windows(2).position(|w| w == b"\r\n").map(|p| p + at) else {
                break;
            };
            let size = usize::from_str_radix(std::str::from_utf8(&body[at..line_end]).unwrap_or(""), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            parts.extend_from_slice(&body[line_end + 2..(line_end + 2 + size).min(body.len())]);
            at = line_end + 2 + size + 2;
        }
        body = parts;
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
    json!({ "request": label, "status": status, "headers": headers, "body": parsed })
}

/// One request on its own TLS connection: everything read until the gateway closes (or 3 s).
async fn exchange(port: u16, step: &Value) -> Vec<u8> {
    let mut stream = tls_connect(port, step["local"].as_str()).await.unwrap();
    stream
        .write_all(step["raw"].as_str().unwrap().as_bytes())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut data = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut buf)).await {
            Err(_) | Ok(Ok(0)) | Ok(Err(_)) => return data,
            Ok(Ok(n)) => data.extend_from_slice(&buf[..n]),
        }
        if let Some(send) = step["send"].as_str()
            && data.starts_with(b"HTTP/1.1 101")
            && data.windows(4).any(|w| w == b"\r\n\r\n")
        {
            stream.write_all(send.as_bytes()).await.unwrap();
            let until = tokio::time::Instant::now() + Duration::from_millis(300);
            while let Ok(Ok(n)) = tokio::time::timeout_at(until, stream.read(&mut buf)).await {
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            return data;
        }
    }
}

async fn run(name: &str) {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let script: Value =
        serde_json::from_str(&std::fs::read_to_string(here.join(format!("{name}.requests.json"))).unwrap()).unwrap();
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(here.join(format!("{name}.expected.json"))).unwrap()).unwrap();

    let allowed: Arc<Mutex<Vec<String>>> = Arc::default();
    let backend = upstream("backend", allowed.clone()).await;
    let content = upstream("content", allowed.clone()).await;
    let helper = upstream("helper", allowed.clone()).await;
    let ws = start_ws_upstream().await;
    let cert = make_cert();
    let (cert_file, key_file) = (
        cert.cert_file.to_string_lossy().into_owned(),
        cert.key_file.to_string_lossy().into_owned(),
    );
    let urls = [backend, content, ws.port, helper].map(|port| format!("http://127.0.0.1:{port}"));
    let mut pairs: Vec<(String, String)> = vec![
        ("GATEWAY_BIND".into(), "127.0.0.1".into()),
        ("GATEWAY_CERT".into(), cert_file),
        ("GATEWAY_KEY".into(), key_file),
        ("GATEWAY_SECRET".into(), SECRET.into()),
        ("GATEWAY_BACKEND_URL".into(), urls[0].clone()),
        ("GATEWAY_CONTENT_URL".into(), urls[1].clone()),
        ("GATEWAY_WS_URL".into(), urls[2].clone()),
        ("ALLOWLIST_URL".into(), urls[3].clone()),
        ("ALLOWLIST_SECRET".into(), HELPER_SECRET.into()),
    ];
    for (key, value) in script["env"].as_object().into_iter().flatten() {
        pairs.push((key.clone(), value.as_str().unwrap().to_owned()));
    }
    let env = service_kit::env::Env::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let config = gateway::config::Config::from_env(&env).unwrap();
    let feed = AllowlistFeed::new(config.allowlist.as_ref().unwrap());
    let running = start_gateway(&cert, config, Some(feed.clone())).await;
    service_kit::linelog::set_level(service_kit::linelog::Level::Info);

    let mut actual = Vec::new();
    for step in script["requests"].as_array().unwrap() {
        let start = log_len();
        let data = exchange(running.port, step).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut result = parse(step["label"].as_str().unwrap(), &data);
        let log: Vec<Value> = log_since(start)
            .into_iter()
            .map(|(_, line)| line)
            .filter(|line| line["msg"] == "request" || line["msg"] == "websocket closed")
            .map(|mut line| {
                let fields = line.as_object_mut().unwrap();
                fields.remove("t");
                fields.remove("ms");
                line
            })
            .collect();
        result["log"] = json!(log);
        actual.push(result);
    }
    feed.idle().await;
    let mut ips = allowed.lock().unwrap().clone();
    ips.sort();
    actual.push(json!({ "request": "allowlist feed", "status": 0, "headers": {}, "body": ips, "log": [] }));
    running.close().await;

    let expected = expected["results"].as_array().unwrap();
    let differences: Vec<String> = expected
        .iter()
        .zip(&actual)
        .enumerate()
        .filter(|(_, (want, got))| want != got)
        .map(|(i, (want, got))| format!("#{i} {}\n  node: {want}\n  rust: {got}", want["request"]))
        .collect();
    assert_eq!(expected.len(), actual.len(), "reply count");
    assert!(
        differences.is_empty(),
        "{} of {} replies differ:\n{}",
        differences.len(),
        expected.len(),
        differences.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn gateway() {
    run("gateway").await;
}
