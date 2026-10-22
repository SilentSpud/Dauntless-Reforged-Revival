#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! Download slots and the download log, against a client that stops reading (test/app.test.ts's
//! "per-account and server-wide stream limits" and "the log names the account and bytes").

use std::sync::{Arc, Mutex};
use std::time::Duration;

use content::app::{App, AppState, Deps};
use content::auth::{AccountInfo, AuthCache, Lookup, LookupResult};
use content::branding::Branding;
use content::limits::StreamLimiter;
use content::manifest;
use content::news::News;
use futures_util::FutureExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const GOOD: &str = "UUK_app_test_good_key_0123456789";
const OTHER: &str = "UUK_app_test_other_key_987654321";

async fn start(dir: &std::path::Path) -> (AppState, u16) {
    std::fs::create_dir_all(dir.join("Data")).unwrap();
    let small: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
    // Big enough that a stalled client keeps the server's stream open (socket buffers fill up)
    let big: Vec<u8> = (0..48 * 1024 * 1024u32).map(|i| (i * 31 + 7) as u8).collect();
    std::fs::write(dir.join("Data/small.bin"), &small).unwrap();
    std::fs::write(dir.join("Data/big.bin"), &big).unwrap();
    let sha = |d: &[u8]| Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect::<String>();
    let manifest = manifest::validate(&serde_json::json!({
        "build": "test",
        "totalBytes": small.len() + big.len(),
        "files": [
            {"path": "Data/big.bin", "size": big.len(), "sha256": sha(&big)},
            {"path": "Data/small.bin", "size": small.len(), "sha256": sha(&small)},
        ],
    }))
    .unwrap();
    let lookup: Lookup = Arc::new(|key: String| {
        async move {
            let account = |id: &str, name: &str| {
                LookupResult::Ok(AccountInfo {
                    user_id: id.into(),
                    username: name.into(),
                    is_admin: false,
                })
            };
            match key.as_str() {
                GOOD => account("UID-A", "alpha"),
                OTHER => account("UID-B", "beta"),
                _ => LookupResult::Invalid,
            }
        }
        .boxed()
    });
    let clock: content::auth::Clock = Arc::new(content::now_ms);
    let app = App::new(Deps {
        download_base_url: None,
        index: manifest::index(&manifest),
        states: Mutex::new(content::gamedir::check(dir, &manifest).states),
        manifest,
        game_dir: dir.to_path_buf(),
        auth: AuthCache::new(lookup, Duration::from_secs(300)),
        limiter: StreamLimiter::new(2, 3),
        branding: Branding::new(None, clock.clone()),
        news: Arc::new(News::new(None, None, clock)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopping) = tokio::sync::watch::channel(false);
    std::mem::forget(stop);
    tokio::spawn(content::server::serve(listener, app.clone(), stopping));
    (app, port)
}

/// Starts a download and stops reading after the status line: the stream stays open.
async fn stalled(port: u16, key: &str) -> (u16, tokio::net::TcpStream) {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(
            format!(
                "GET /content/v1/files/Data/big.bin HTTP/1.1\r\nhost: x\r\nx-undaunted-user-api-key: {key}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut head = [0u8; 12];
    socket.read_exact(&mut head).await.unwrap();
    let status = std::str::from_utf8(&head[9..12]).unwrap().parse().unwrap();
    (status, socket)
}

async fn get(port: u16, method: &str, path: &str, key: &str) -> (u16, String) {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nhost: x\r\nx-undaunted-user-api-key: {key}\r\nconnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut reply = Vec::new();
    socket.read_to_end(&mut reply).await.unwrap();
    let text = String::from_utf8_lossy(&reply).into_owned();
    let status = text[9..12].parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
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

#[tokio::test(flavor = "multi_thread")]
async fn per_account_and_server_wide_stream_limits() {
    let dir = tempfile::tempdir().unwrap();
    let (app, port) = start(dir.path()).await;
    let limiter = &app.deps().limiter;
    let (a1_status, a1) = stalled(port, GOOD).await;
    let (a2_status, a2) = stalled(port, GOOD).await;
    assert_eq!((a1_status, a2_status), (200, 200));
    assert_eq!(limiter.active(Some("UID-A")), 2);
    let (status, body) = get(port, "GET", "/content/v1/files/Data/small.bin", GOOD).await;
    assert_eq!((status, body.as_str()), (429, r#"{"error":"too_many_streams"}"#));
    // HEAD doesn't take a slot
    assert_eq!(get(port, "HEAD", "/content/v1/files/Data/small.bin", GOOD).await.0, 200);
    let (b1_status, b1) = stalled(port, OTHER).await;
    assert_eq!(b1_status, 200);
    let (status, body) = get(port, "GET", "/content/v1/files/Data/small.bin", OTHER).await;
    assert_eq!(
        (status, body.as_str()),
        (503, r#"{"error":"server_busy"}"#),
        "server-wide limit of 3 reached"
    );
    drop(a1);
    wait_for("slot release", || limiter.active(Some("UID-A")) == 1).await;
    assert_eq!(get(port, "GET", "/content/v1/files/Data/small.bin", GOOD).await.0, 200);
    drop(a2);
    drop(b1);
    wait_for("all slots released", || limiter.active(None) == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_names_the_account_and_bytes_never_the_key() {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    content::log::set_sink(Some(Box::new(move |line, _| {
        sink.lock().unwrap().push(line.to_owned())
    })));
    let dir = tempfile::tempdir().unwrap();
    let (_app, port) = start(dir.path()).await;
    assert_eq!(get(port, "GET", "/content/v1/files/Data/small.bin", GOOD).await.0, 200);
    let find = || {
        lines.lock().unwrap().iter().find_map(|l| {
            let entry: serde_json::Value = serde_json::from_str(l).unwrap();
            (entry["msg"] == "download" && entry["path"] == "Data/small.bin" && entry["status"] == 200).then_some(entry)
        })
    };
    wait_for("the download log", || find().is_some()).await;
    let download = find().unwrap();
    assert_eq!(download["account"], "UID-A");
    assert_eq!(download["user"], "alpha");
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
    let all = lines.lock().unwrap().join("\n");
    assert!(!all.contains(GOOD) && !all.contains(OTHER), "a key leaked into the log");
}
