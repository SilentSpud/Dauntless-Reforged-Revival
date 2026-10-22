#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The content server's parity with the Node one (MIGRATION_PLAN.md S1): each
//! `tests/parity/<name>.requests.json` script runs against the Rust server, and every reply's status,
//! headers and body must be what the Node server answered (`<name>.expected.json`, written by
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

fn mtime(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn write_file(full: &Path, data: &[u8], modified: SystemTime) {
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(full).unwrap();
    file.write_all(data).unwrap();
    file.set_modified(modified).unwrap();
}

fn under(root: &Path, relative: &str) -> std::path::PathBuf {
    relative.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// Kept in step with fixture() in record.cjs.
fn fixture(script: &Value, dir: &Path) {
    let game = dir.join("game");
    std::fs::create_dir_all(&game).unwrap();
    let mut files: Vec<(String, Vec<u8>)> = script["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(rel, spec)| (rel.clone(), bytes_of(spec)))
        .collect();
    for (rel, data) in &files {
        write_file(&under(&game, rel), data, mtime(1_700_000_000));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let manifest = json!({
        "build": script["build"].as_str().unwrap_or("dauntless_rel-1.4.4_Shipping_2020-10-28_20-11-15_239827"),
        "totalBytes": files.iter().map(|(_, d)| d.len()).sum::<usize>(),
        "files": files.iter().map(|(rel, data)| json!({
            "path": rel,
            "size": data.len(),
            "sha256": Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect::<String>(),
        })).collect::<Vec<_>>(),
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    for rel in script["gone"].as_array().into_iter().flatten() {
        std::fs::remove_file(under(&game, rel.as_str().unwrap())).unwrap();
    }
    for key in ["resized", "extra"] {
        for (rel, spec) in script[key].as_object().into_iter().flatten() {
            write_file(&under(&game, rel), &bytes_of(spec), mtime(1_700_000_000));
        }
    }
    if let Some(branding) = script.get("branding") {
        let art = dir.join("art");
        std::fs::create_dir_all(&art).unwrap();
        for (file, spec) in branding["files"].as_object().into_iter().flatten() {
            write_file(&art.join(file), &bytes_of(spec), mtime(1_700_000_000));
        }
        if let Some(text) = branding.get("json") {
            let text = text
                .as_str()
                .map_or_else(|| service_kit::jsjson::stringify(text), str::to_owned);
            write_file(&art.join("branding.json"), text.as_bytes(), mtime(1_700_000_000));
        }
    }
    if let Some(news) = script.get("news") {
        let text = news
            .as_str()
            .map_or_else(|| service_kit::jsjson::stringify(news), str::to_owned);
        write_file(&dir.join("news.json"), text.as_bytes(), mtime(1_700_000_000));
    }
}

/// The fake metagame: what `keys` says for each key, else 401.
async fn fake_metagame(keys: Value) -> String {
    let handler = move |request: axum::extract::Request| {
        let keys = keys.clone();
        async move {
            let key = request
                .headers()
                .get("x-undaunted-user-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let answer = keys.get(key).cloned().unwrap_or(json!({"status": 401, "body": {}}));
            let body = answer["body"]
                .as_str()
                .map_or_else(|| answer["body"].to_string(), str::to_owned);
            (
                axum::http::StatusCode::from_u16(answer["status"].as_u64().unwrap() as u16).unwrap(),
                [("content-type", "application/json")],
                body,
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
async fn raw(port: u16, request: &Value) -> Vec<u8> {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let headers: String = request["headers"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}: {}\r\n", v.as_str().unwrap()))
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
        _ => format!(
            "<{} bytes sha256 {}>",
            body.len(),
            Sha256::digest(body)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
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
    let metagame = fake_metagame(script["keys"].clone()).await;
    let path = |p: &str| dir.path().join(p).to_string_lossy().into_owned();
    let mut pairs = vec![
        ("CONTENT_GAME_DIR".to_owned(), path("game")),
        ("CONTENT_MANIFEST".to_owned(), path("manifest.json")),
        ("METAGAME_URL".to_owned(), metagame),
    ];
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
    let app = content::server::prepare(&config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(content::server::serve(listener, app, stopping));
    let mut actual = Vec::new();
    for request in script["requests"].as_array().unwrap() {
        if let Some(touched) = request["touch"].as_str() {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(under(&dir.path().join("game"), touched))
                .unwrap();
            file.set_modified(mtime(1_700_000_001)).unwrap();
            continue;
        }
        let reply = raw(port, request).await;
        let label = format!(
            "{} {}",
            request["method"].as_str().unwrap_or("GET"),
            request["path"].as_str().unwrap()
        );
        actual.push(parse(&reply, label));
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
async fn content_plain() {
    run("content-plain").await;
}
