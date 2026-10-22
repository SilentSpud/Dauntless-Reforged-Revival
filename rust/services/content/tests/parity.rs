#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The content server's parity with the Node one (MIGRATION_PLAN.md RS2): each
//! `tests/parity/<name>.requests.json` script runs against the Rust server, and every reply's status,
//! headers and body must be what `legacy/ReforgedContent` answered (`<name>.expected.json`, written by
//! `tests/parity/record.cjs` on the same fixture). Requests go out as raw HTTP over TCP, the path
//! exactly as written.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use service_kit::env::Env;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Kept in step with bytesOf() in record.cjs.
fn bytes_of(spec: &Value) -> Vec<u8> {
    if let Some(text) = spec["text"].as_str() {
        return text.as_bytes().to_vec();
    }
    if let Some(hex) = spec["hex"].as_str() {
        return (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
    }
    (0..spec["pattern"].as_u64().unwrap())
        .map(|i| ((i * 7 + 3) & 0xff) as u8)
        .collect()
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn write_file(full: &Path, data: &[u8]) {
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(full).unwrap();
    file.write_all(data).unwrap();
    // Every file's modification time, so ETags made from it are the same on both servers
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .unwrap();
}

/// An object as `JSON.stringify` writes it, or text as it is.
fn text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| service_kit::jsjson::stringify(value), str::to_owned)
}

/// Kept in step with fixture() in record.cjs.
fn fixture(script: &Value, dir: &Path) {
    if let Some(runtime) = script.get("runtime").filter(|r| !r.is_null()) {
        let folder = dir.join("runtime");
        std::fs::create_dir_all(&folder).unwrap();
        let files: Vec<(String, Vec<u8>)> = runtime["files"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, spec)| (name.clone(), bytes_of(spec)))
            .collect();
        for (name, data) in &files {
            write_file(&folder.join(name), data);
        }
        let index = runtime.get("index").cloned().unwrap_or_else(|| {
            json!({
                "version": "20261014-1a2b3c4d",
                "build": {"changelist": 647472},
                "files": files.iter().map(|(name, data)| json!({"name": name, "size": data.len(), "sha256": hex(data)})).collect::<Vec<_>>(),
            })
        });
        // "$sha:<file>" in an index given as text is that file's SHA-256
        let mut index = text(&index);
        for (name, data) in &files {
            index = index.replace(&format!("$sha:{name}"), &hex(data));
        }
        write_file(&folder.join("runtime.json"), index.as_bytes());
    }
    if let Some(branding) = script.get("branding") {
        let art = dir.join("art");
        std::fs::create_dir_all(&art).unwrap();
        for (file, spec) in branding["files"].as_object().into_iter().flatten() {
            write_file(&art.join(file), &bytes_of(spec));
        }
        if let Some(json) = branding.get("json") {
            write_file(&art.join("branding.json"), text(json).as_bytes());
        }
    }
    if let Some(news) = script.get("news") {
        write_file(&dir.join("news.json"), text(news).as_bytes());
    }
}

/// The fake backend: what `keys` says for each key at GetUserInfo, else 401.
async fn fake_backend(keys: Value) -> String {
    let handler = move |request: axum::extract::Request| {
        let keys = keys.clone();
        async move {
            let key = request
                .headers()
                .get("x-reforged-user-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let answer = keys
                .get(key)
                .filter(|_| request.uri() == "/reforged/api/GetUserInfo")
                .cloned()
                .unwrap_or(json!({"status": 401, "body": {}}));
            (
                axum::http::StatusCode::from_u16(answer["status"].as_u64().unwrap() as u16).unwrap(),
                [("content-type", "application/json")],
                text(&answer["body"]),
            )
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().fallback(handler))
            .await
            .unwrap();
    });
    format!("http://{address}")
}

const HEADERS: [&str; 11] = [
    "content-type",
    "content-length",
    "content-range",
    "etag",
    "accept-ranges",
    "cache-control",
    "retry-after",
    "allow",
    "access-control-allow-origin",
    "cross-origin-resource-policy",
    "x-content-type-options",
];

/// One raw HTTP/1.1 exchange on its own connection, as record.cjs makes it.
async fn raw(port: u16, request: &Value, last_etag: &str) -> Vec<u8> {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let headers: String = request["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|pair| {
            format!(
                "{}: {}\r\n",
                pair[0].as_str().unwrap(),
                pair[1].as_str().unwrap().replacen("$etag", last_etag, 1)
            )
        })
        .collect();
    let text = format!(
        "{} {} HTTP/1.1\r\nhost: content\r\n{headers}connection: close\r\n\r\n",
        request["method"].as_str().unwrap_or("GET"),
        request["path"].as_str().unwrap()
    );
    socket.write_all(text.as_bytes()).await.unwrap();
    let mut reply = Vec::new();
    socket.read_to_end(&mut reply).await.unwrap();
    reply
}

fn parse(reply: &[u8], label: String) -> Value {
    let end = reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&reply[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u64 = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let mut found = std::collections::HashMap::new();
    for line in lines {
        let (key, value) = line.split_once(':').unwrap();
        found.insert(key.trim().to_lowercase(), value.trim().to_owned());
    }
    let mut headers = serde_json::Map::new();
    for name in HEADERS {
        if let Some(value) = found.get(name) {
            headers.insert(name.into(), json!(value));
        }
    }
    let body = &reply[end + 4..];
    let shown = match std::str::from_utf8(body) {
        Ok(text) if body.len() <= 4096 && !text.bytes().any(|b| b <= 8) => text.to_owned(),
        _ => format!("<{} bytes sha256 {}>", body.len(), hex(body)),
    };
    json!({"request": label, "status": status, "headers": headers, "body": shown})
}

async fn run(name: &str) {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let script: Value =
        serde_json::from_str(&std::fs::read_to_string(here.join(format!("{name}.requests.json"))).unwrap()).unwrap();
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(here.join(format!("{name}.expected.json"))).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    fixture(&script, dir.path());
    let backend = fake_backend(script["keys"].clone()).await;
    let path = |p: &str| dir.path().join(p).to_string_lossy().into_owned();
    let mut pairs = vec![("BACKEND_URL".to_owned(), backend)];
    if script.get("runtime").is_some_and(|r| !r.is_null()) {
        pairs.push(("CONTENT_RUNTIME_DIR".into(), path("runtime")));
    }
    if script.get("branding").is_some() {
        pairs.push(("CONTENT_BRANDING_DIR".into(), path("art")));
    }
    if script.get("news").is_some() {
        pairs.push(("CONTENT_NEWS_FILE".into(), path("news.json")));
    }
    for (key, value) in script["env"].as_object().into_iter().flatten() {
        pairs.push((key.clone(), value.as_str().unwrap().to_owned()));
    }
    let env = Env::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let config = content::config::Config::from_env(&env).unwrap();
    content::log::set_sink(Some(Box::new(|_, _| {})));
    let app = content::server::prepare(&config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(content::server::serve(listener, app, stopping));
    let mut actual = Vec::new();
    let mut last_etag = String::new();
    for request in script["requests"].as_array().unwrap() {
        if let Some(file) = request["write"].as_str() {
            write_file(&dir.path().join("runtime").join(file), &bytes_of(&request["spec"]));
            continue;
        }
        let reply = raw(port, request, &last_etag).await;
        let label = format!(
            "{} {}",
            request["method"].as_str().unwrap_or("GET"),
            request["path"].as_str().unwrap()
        );
        let result = parse(&reply, label);
        if let Some(etag) = result["headers"]["etag"].as_str() {
            last_etag = etag.to_owned();
        }
        actual.push(result);
    }
    let _ = stop.send(true);
    let _ = server.await;
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
async fn content() {
    run("content").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn content_none() {
    run("content-none").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn content_invalid() {
    run("content-invalid").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn content_index() {
    run("content-index").await;
}
