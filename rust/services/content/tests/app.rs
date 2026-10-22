#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The content server over HTTP (test/app.test.ts), plus download slots and the download log against
//! a client that stops reading (from Dauntless Revived's streams test).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use content::app::{App, AppState, Deps};
use content::auth::{AccountInfo, AuthCache, Lookup, LookupResult};
use content::branding::Branding;
use content::limits::StreamLimiter;
use content::news::News;
use content::runtime::{RuntimeStore, sha256_hex};
use futures_util::FutureExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const KEY: &str = "RRK_cccccccccccccccccccccccccccccccccccccccccccccccc";
const OTHER: &str = "RRK_dddddddddddddddddddddddddddddddddddddddddddddddd";

/// Every log line of this test binary (the log is one per process).
fn lines() -> &'static Mutex<Vec<String>> {
    static LINES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LINES.get_or_init(|| {
        content::log::set_sink(Some(Box::new(|line, _| {
            lines().lock().unwrap().push(line.to_owned());
        })));
        Mutex::new(Vec::new())
    })
}

/// test/helpers.ts `Publish`, with the DLL's size given.
fn publish(dir: &Path, dll: Vec<u8>, loader: Vec<u8>, version: &str) -> (Vec<u8>, Vec<u8>) {
    std::fs::write(dir.join("MysticParadox.dll"), &dll).unwrap();
    std::fs::write(dir.join("winmm.dll"), &loader).unwrap();
    let index = json!({
        "version": version,
        "build": {"changelist": 647472},
        "files": [
            {"name": "winmm.dll", "size": loader.len(), "sha256": sha256_hex(&loader)},
            {"name": "MysticParadox.dll", "size": dll.len(), "sha256": sha256_hex(&dll)},
        ],
    });
    std::fs::write(dir.join("runtime.json"), index.to_string()).unwrap();
    (dll, loader)
}

struct Started {
    app: AppState,
    port: u16,
    lookups: Arc<AtomicUsize>,
}

async fn start(runtime_dir: &Path, news_file: Option<&Path>, per_account: usize, total: usize) -> Started {
    lines();
    let lookups = Arc::new(AtomicUsize::new(0));
    let counted = lookups.clone();
    let lookup: Lookup = Arc::new(move |key: String| {
        counted.fetch_add(1, Ordering::SeqCst);
        async move {
            let account = |id: &str, name: &str| {
                LookupResult::Ok(AccountInfo {
                    user_id: id.into(),
                    username: name.into(),
                    is_admin: false,
                })
            };
            match key.as_str() {
                KEY => account("uid-1", "Aurora"),
                OTHER => account("uid-2", "Borealis"),
                _ => LookupResult::Invalid,
            }
        }
        .boxed()
    });
    let clock: content::auth::Clock = Arc::new(content::now_ms);
    let app = App::new(Deps {
        runtime: Arc::new(RuntimeStore::new(Some(runtime_dir.into()), clock.clone())),
        auth: AuthCache::new(lookup, Duration::from_secs(300)),
        limiter: StreamLimiter::new(per_account, total),
        branding: Branding::new(None, clock.clone()),
        news: Arc::new(News::new(news_file.map(Path::to_path_buf), None, clock)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopping) = tokio::sync::watch::channel(false);
    std::mem::forget(stop);
    tokio::spawn(content::server::serve(listener, app.clone(), stopping));
    Started { app, port, lookups }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A raw request: the path goes on the wire exactly as given (no dot-segment clean-up, no encoding).
async fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> Reply {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    socket
        .write_all(format!("{method} {path} HTTP/1.1\r\nhost: x\r\n{extra}connection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut reply = Vec::new();
    socket.read_to_end(&mut reply).await.unwrap();
    let end = reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&reply[..end]).into_owned();
    let mut head_lines = head.split("\r\n");
    let status = head_lines.next().unwrap()[9..12].parse().unwrap();
    let headers = head_lines
        .map(|l| {
            let (k, v) = l.split_once(':').unwrap();
            (k.trim().to_lowercase(), v.trim().to_owned())
        })
        .collect();
    Reply {
        status,
        headers,
        body: reply[end + 4..].to_vec(),
    }
}

/// Starts a download and stops reading after the status line: the stream stays open.
async fn stalled(port: u16, key: &str) -> (u16, tokio::net::TcpStream) {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(
            format!("GET /content/v1/runtime/files/MysticParadox.dll HTTP/1.1\r\nhost: x\r\nx-reforged-user-api-key: {key}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut head = [0u8; 12];
    socket.read_exact(&mut head).await.unwrap();
    let status = std::str::from_utf8(&head[9..12]).unwrap().parse().unwrap();
    (status, socket)
}

async fn wait_for(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

const DLL: &str = "/content/v1/runtime/files/MysticParadox.dll";

/// test/app.test.ts's `before`: the runtime from `Publish(Dir)` and one news item.
async fn published() -> (tempfile::TempDir, Started, Vec<u8>, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let (dll, loader) = publish(dir.path(), vec![1; 5001], vec![2; 3001], "20261014-1a2b3c4d");
    let news = dir.path().join("news.json");
    std::fs::write(
        &news,
        json!({"items": [{"date": "2026-10-14", "title": "Hello", "body": "First hunt on Friday."}]}).to_string(),
    )
    .unwrap();
    let started = start(dir.path(), Some(&news), 4, 48).await;
    (dir, started, dll, loader)
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_runtime_manifest_without_a_key_with_an_etag() {
    let (_dir, server, dll, _) = published().await;
    let reply = request(server.port, "GET", "/content/v1/runtime/manifest", &[]).await;
    assert_eq!(reply.status, 200);
    let body = reply.json();
    assert_eq!(body["version"], "20261014-1a2b3c4d");
    assert_eq!(body["files"][0]["name"], "MysticParadox.dll");
    assert_eq!(body["files"][1]["name"], "winmm.dll");
    assert_eq!(body["files"][0]["sha256"], sha256_hex(&dll));
    let etag = reply.header("etag").unwrap().to_owned();
    assert_eq!(etag, format!("\"{}\"", sha256_hex(&reply.body)));
    let again = request(
        server.port,
        "GET",
        "/content/v1/runtime/manifest",
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(again.status, 304);
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_a_runtime_file_only_with_a_known_account_key() {
    let (_dir, server, dll, loader) = published().await;
    assert_eq!(request(server.port, "GET", DLL, &[]).await.status, 401);
    let unknown = format!("RRK_{}", "0".repeat(48));
    assert_eq!(
        request(server.port, "GET", DLL, &[("x-reforged-user-api-key", &unknown)])
            .await
            .status,
        401
    );
    let reply = request(server.port, "GET", DLL, &[("x-reforged-user-api-key", KEY)]).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, dll);
    assert_eq!(reply.header("content-type"), Some("application/octet-stream"));
    let reply = request(
        server.port,
        "GET",
        "/content/v1/runtime/files/winmm.dll",
        &[("x-reforged-user-api-key", KEY)],
    )
    .await;
    assert_eq!(reply.body, loader);
    assert!(
        !lines().lock().unwrap().iter().any(|l| l.contains(KEY)),
        "the key is never logged"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_nothing_but_the_two_runtime_files() {
    let (_dir, server, _, _) = published().await;
    for target in [
        "/content/v1/runtime/files/runtime.json",
        "/content/v1/runtime/files/../runtime.json",
        "/content/v1/runtime/files/%2e%2e/runtime.json",
        "/content/v1/runtime/files/winmm.DLL",
        "/content/v1/runtime/files/",
        "/content/v1/runtime/files/MysticParadox.dll/x",
        "/content/v1/runtime/",
        "/content/v1/files/x",
        "/",
        "/content",
    ] {
        let reply = request(server.port, "GET", target, &[("x-reforged-user-api-key", KEY)]).await;
        assert_eq!(reply.status, 404, "{target}");
    }
    assert_eq!(
        request(server.port, "POST", "/content/v1/runtime/manifest", &[])
            .await
            .status,
        405
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_host_s_news_and_an_empty_art_pack() {
    let (_dir, server, _, _) = published().await;
    let news = request(server.port, "GET", "/content/v1/news", &[]).await.json();
    assert_eq!(news["items"][0]["title"], "Hello");
    let art = request(server.port, "GET", "/content/v1/branding", &[]).await.json();
    assert_eq!(art, json!({"backgrounds": [], "accent": null}));
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_404_no_runtime_before_the_host_publishes() {
    let (_dir, published_server, _, _) = published().await;
    request(published_server.port, "GET", DLL, &[("x-reforged-user-api-key", KEY)]).await;
    assert!(published_server.lookups.load(Ordering::SeqCst) >= 1);

    let empty = tempfile::tempdir().unwrap();
    let server = start(empty.path(), None, 4, 48).await;
    let reply = request(server.port, "GET", "/content/v1/runtime/manifest", &[]).await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json(), json!({"error": "no_runtime"}));
}

// ---- Download slots and the log (DR's streams test, on the runtime files) ----

/// A runtime whose DLL is big enough that a stalled client keeps the server's stream open (socket
/// buffers fill up).
async fn big() -> (tempfile::TempDir, Started) {
    let dir = tempfile::tempdir().unwrap();
    let dll: Vec<u8> = (0..48 * 1024 * 1024u32).map(|i| (i * 31 + 7) as u8).collect();
    let loader: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
    publish(dir.path(), dll, loader, "big");
    let started = start(dir.path(), None, 2, 3).await;
    (dir, started)
}

#[tokio::test(flavor = "multi_thread")]
async fn per_account_and_server_wide_stream_limits() {
    let (_dir, server) = big().await;
    let port = server.port;
    let limiter = &server.app.deps().limiter;
    let small = "/content/v1/runtime/files/winmm.dll";
    let (a1_status, a1) = stalled(port, KEY).await;
    let (a2_status, a2) = stalled(port, KEY).await;
    assert_eq!((a1_status, a2_status), (200, 200));
    assert_eq!(limiter.active(Some("uid-1")), 2);
    let reply = request(port, "GET", small, &[("x-reforged-user-api-key", KEY)]).await;
    assert_eq!(
        (reply.status, reply.body.as_slice()),
        (429, &br#"{"error":"too_many_streams"}"#[..])
    );
    assert_eq!(reply.header("retry-after"), Some("2"));
    // HEAD doesn't take a slot
    let head = request(port, "HEAD", small, &[("x-reforged-user-api-key", KEY)]).await;
    assert_eq!((head.status, head.header("content-length")), (200, Some("5000")));
    let (b1_status, b1) = stalled(port, OTHER).await;
    assert_eq!(b1_status, 200);
    let reply = request(port, "GET", small, &[("x-reforged-user-api-key", OTHER)]).await;
    assert_eq!(
        (reply.status, reply.body.as_slice()),
        (503, &br#"{"error":"server_busy"}"#[..]),
        "server-wide limit of 3 reached"
    );
    drop(a1);
    wait_for("slot release", || limiter.active(Some("uid-1")) == 1).await;
    assert_eq!(
        request(port, "GET", small, &[("x-reforged-user-api-key", KEY)])
            .await
            .status,
        200
    );
    drop(a2);
    drop(b1);
    wait_for("all slots released", || limiter.active(None) == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_names_the_account_and_bytes_never_the_key() {
    let (_dir, server) = big().await;
    let reply = request(
        server.port,
        "GET",
        "/content/v1/runtime/files/winmm.dll",
        &[("x-reforged-user-api-key", OTHER), ("x-forwarded-for", "203.0.113.9")],
    )
    .await;
    assert_eq!(reply.status, 200);
    let find = || {
        lines().lock().unwrap().iter().find_map(|l| {
            let entry: Value = serde_json::from_str(l).unwrap();
            (entry["msg"] == "download" && entry["ip"] == "203.0.113.9").then_some(entry)
        })
    };
    wait_for("the download log", || find().is_some()).await;
    let download = find().unwrap();
    assert_eq!(download["route"], "runtime-file");
    assert_eq!(download["path"], "winmm.dll");
    assert_eq!(download["user"], "Borealis");
    assert_eq!(download["ip"], "203.0.113.9");
    assert_eq!(download["bytes"], 5000);
    assert_eq!(download["expected"], 5000);
    assert_eq!(download["complete"], true);
    let keys: Vec<&str> = download.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "t", "level", "msg", "method", "route", "path", "status", "ms", "ip", "account", "user", "bytes",
            "expected", "complete"
        ]
    );
    let all = lines().lock().unwrap().join("\n");
    assert!(!all.contains(KEY) && !all.contains(OTHER), "a key leaked into the log");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_changed_under_the_list_is_refused_and_picked_up_again() {
    let dir = tempfile::tempdir().unwrap();
    publish(dir.path(), vec![1; 5001], vec![2; 3001], "v1");
    let server = start(dir.path(), None, 4, 48).await;
    let key = [("x-reforged-user-api-key", KEY)];
    assert_eq!(request(server.port, "GET", DLL, &key).await.status, 200);
    // A new publish in progress: the DLL has another size than the list the server checked
    std::fs::write(dir.path().join("MysticParadox.dll"), vec![3u8; 10]).unwrap();
    let reply = request(server.port, "GET", DLL, &key).await;
    assert_eq!(reply.status, 503);
    assert_eq!(reply.json(), json!({"error": "file_unavailable"}));
    assert_eq!(reply.header("retry-after"), Some("10"));
    // The store forgot its check: the next request sees the list no longer matches
    let manifest = request(server.port, "GET", "/content/v1/runtime/manifest", &[]).await;
    assert_eq!(
        (manifest.status, manifest.json()),
        (503, json!({"error": "runtime_invalid"}))
    );
    publish(dir.path(), vec![4; 6000], vec![5; 3000], "v2");
    server.app.deps().runtime.invalidate();
    let manifest = request(server.port, "GET", "/content/v1/runtime/manifest", &[]).await;
    assert_eq!(manifest.json()["version"], "v2");
    assert_eq!(request(server.port, "GET", DLL, &key).await.body, vec![4; 6000]);
}
