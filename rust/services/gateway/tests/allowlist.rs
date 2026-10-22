#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The allowlist helper (`test/allowlist.test.ts`, `test/e2e.test.ts`): over HTTP, its expiry, a
//! restart, and with the gateway in front of it, in process and as the two binaries. Every helper
//! here runs in DRY-RUN: nothing in this file ever changes a firewall.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use common::*;
use gateway::allowlist::config::AllowlistConfig;
use gateway::allowlist::firewall::{Backend, NftSettings};
use gateway::allowlist::helper::{AuditLog, Helper, Running};
use gateway::config::Allowlist;
use gateway::feed::AllowlistFeed;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};

const SECRET: &str = "allowlist-test-secret-0123456789abcdef0123456789";

fn helper_config(dir: &Path, extra: &[(&str, &str)]) -> AllowlistConfig {
    let audit = dir.join("audit.log").to_string_lossy().into_owned();
    let state = dir.join("state.json").to_string_lossy().into_owned();
    let mut pairs: Vec<(&str, &str)> = vec![
        ("ALLOWLIST_DRY_RUN", "1"),
        ("ALLOWLIST_SECRET", SECRET),
        ("ALLOWLIST_PORT", "1"),
        ("ALLOWLIST_MIN_INTERVAL_MS", "200"),
        ("ALLOWLIST_AUDIT_LOG", &audit),
        ("ALLOWLIST_STATE_FILE", &state),
    ];
    pairs.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
    pairs.extend_from_slice(extra);
    let mut config = AllowlistConfig::from_env(&service_kit::env::Env::from_pairs(pairs)).unwrap();
    config.port = 0;
    config
}

/// The audit log's complete lines: a helper process may be writing the last one as it is read.
fn audit_events(file: &Path) -> Vec<Value> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .split_inclusive('\n')
        .filter(|l| l.ends_with('\n') && !l.trim().is_empty())
        .map(|l| serde_json::from_str(l.trim_end()).unwrap())
        .collect()
}

fn dry_runs(file: &Path) -> Vec<Vec<String>> {
    audit_events(file)
        .into_iter()
        .filter(|e| e["event"] == "dry_run")
        .map(|e| serde_json::from_value(e["ips"].clone()).unwrap())
        .collect()
}

struct Started {
    helper: Arc<Helper>,
    running: Option<Running>,
    audit_file: PathBuf,
    state_file: PathBuf,
    port: u16,
}

impl Started {
    async fn close(mut self) {
        self.helper.close(self.running.take().unwrap()).await;
    }
}

async fn start(config: AllowlistConfig, clock: Arc<AtomicI64>) -> Started {
    log_lines();
    let audit_file = config.audit_log.clone();
    let state_file = config.state_file.clone();
    let audit = Arc::new(AuditLog::new(Some(audit_file.clone())));
    let backend = Backend::dry_run(NftSettings::from_env(&service_kit::env::Env::from_pairs([])).unwrap());
    let helper = Helper::new(config, backend, audit, Arc::new(move || clock.load(Ordering::SeqCst)));
    let running = helper.start().await.unwrap();
    let port = running.port;
    Started {
        helper,
        running: Some(running),
        audit_file,
        state_file,
        port,
    }
}

/// One plain HTTP request to the helper: the status and the JSON body.
async fn call(port: u16, method: &str, path: &str, body: Option<&str>, secret: Option<&str>) -> (u16, Value) {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(connection);
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1")
        .header("content-type", "application/json");
    if let Some(secret) = secret {
        request = request.header("x-allowlist-secret", secret);
    }
    let request = request
        .body(Full::new(Bytes::from(body.unwrap_or("").to_owned())))
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn allow(port: u16, ip: Value) -> (u16, Value) {
    call(
        port,
        "POST",
        "/allow",
        Some(&json!({ "ip": ip }).to_string()),
        Some(SECRET),
    )
    .await
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_helper_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(1_800_000_000_000));
    let started = start(helper_config(dir.path(), &[]), clock.clone()).await;
    let (port, audit) = (started.port, started.audit_file.clone());
    let helper = started.helper.clone();

    // Closes the ports at start (nothing restored), as a dry run
    helper.sync().settled(Duration::from_secs(5)).await;
    let events = audit_events(&audit);
    assert_eq!(
        (events[0]["event"].as_str(), events[0]["dryRun"].as_bool()),
        (Some("start"), Some(true))
    );
    assert_eq!(dry_runs(&audit), [Vec::<String>::new()]);
    let applied = events.iter().find(|e| e["event"] == "rule_applied").unwrap();
    assert_eq!(applied["enabled"], false);

    // The secret, the right path and method, and a small JSON body
    let body = json!({ "ip": "203.0.113.7" }).to_string();
    assert_eq!(call(port, "POST", "/allow", Some(&body), None).await.0, 401);
    let wrong = format!("{}x", &SECRET[..SECRET.len() - 1]);
    assert_eq!(call(port, "POST", "/allow", Some(&body), Some(&wrong)).await.0, 401);
    assert_eq!(call(port, "GET", "/status", None, None).await.0, 401);
    assert_eq!(call(port, "POST", "/nope", Some("{}"), Some(SECRET)).await.0, 404);
    assert_eq!(call(port, "GET", "/allow", None, Some(SECRET)).await.0, 405);
    assert_eq!(call(port, "POST", "/status", Some("{}"), Some(SECRET)).await.0, 405);
    let big = format!("{{\"ip\":\"{}\"}}", "1".repeat(2000));
    assert_eq!(call(port, "POST", "/allow", Some(&big), Some(SECRET)).await.0, 413);
    assert_eq!(
        call(port, "POST", "/allow", Some("not json"), Some(SECRET)).await,
        (400, json!({ "error": "bad_request" }))
    );
    assert!(helper.list().is_empty());
    assert!(!std::fs::read_to_string(&audit).unwrap().contains(SECRET));

    // Ranges, garbage, 0.0.0.0, broadcast, multicast and private addresses, with the reason
    for (given, reason) in [
        (json!("0.0.0.0"), "unspecified"),
        (json!("::"), "unspecified"),
        (json!("0.0.0.0/0"), "range_not_allowed"),
        (json!("203.0.113.0/24"), "range_not_allowed"),
        (json!("203.0.113.1-203.0.113.5"), "range_not_allowed"),
        (json!("Any"), "range_not_allowed"),
        (json!("255.255.255.255"), "reserved"),
        (json!("239.1.1.1"), "multicast"),
        (json!("10.0.0.1"), "private"),
        (json!("192.168.0.10"), "private"),
        (json!("100.100.100.100"), "private"),
        (json!("127.0.0.1"), "private"),
        (json!("fe80::1"), "private"),
        (json!("garbage"), "not_an_ip"),
        (json!("203.0.113.7 "), "not_an_ip"),
        (json!(123), "not_a_string"),
        (json!(null), "not_a_string"),
    ] {
        assert_eq!(
            allow(port, given.clone()).await,
            (400, json!({ "error": "invalid_ip", "reason": reason })),
            "{given}"
        );
    }
    assert_eq!(
        call(port, "POST", "/allow", Some("{}"), Some(SECRET)).await.1["reason"],
        "not_a_string"
    );
    assert!(helper.list().is_empty());
    assert!(audit_events(&audit).iter().filter(|e| e["event"] == "refused").count() >= 17);

    // A public address: the rule opens to exactly it, and a repeat refreshes it
    let before = dry_runs(&audit).len();
    assert_eq!(
        allow(port, json!("203.0.113.7")).await,
        (200, json!({ "ip": "203.0.113.7", "added": true, "ttlSeconds": 600 }))
    );
    assert_eq!(
        allow(port, json!("203.0.113.7")).await,
        (200, json!({ "ip": "203.0.113.7", "added": false, "ttlSeconds": 600 }))
    );
    helper.sync().settled(Duration::from_secs(5)).await;
    let runs = dry_runs(&audit);
    assert_eq!(runs.len(), before + 1);
    assert_eq!(runs.last().unwrap(), &strings(&["203.0.113.7"]));
    assert!(
        audit_events(&audit)
            .iter()
            .any(|e| e["event"] == "added" && e["ip"] == "203.0.113.7")
    );

    // Debounced: addresses arriving together become one change, the interval after the last
    tokio::time::sleep(Duration::from_millis(250)).await;
    let before = dry_runs(&audit).len();
    tokio::join!(allow(port, json!("198.51.100.2")), allow(port, json!("2001:DB8::5")));
    helper.sync().settled(Duration::from_secs(5)).await;
    allow(port, json!("8.8.8.8")).await;
    helper.sync().settled(Duration::from_secs(5)).await;
    let runs = dry_runs(&audit);
    let new = &runs[before..];
    assert!((2..=3).contains(&new.len()), "{} rule changes", new.len());
    assert_eq!(
        new.last().unwrap(),
        &strings(&["8.8.8.8", "198.51.100.2", "203.0.113.7", "2001:db8::5"])
    );
    let times: Vec<i64> = audit_events(&audit)
        .iter()
        .filter(|e| e["event"] == "rule_applied")
        .map(|e| {
            chrono::DateTime::parse_from_rfc3339(e["t"].as_str().unwrap())
                .unwrap()
                .timestamp_millis()
        })
        .collect();
    for pair in times.windows(2) {
        assert!(pair[1] - pair[0] >= 150, "two rule changes closer than the interval");
    }
    let (status, body) = call(port, "GET", "/status", None, Some(SECRET)).await;
    assert_eq!(status, 200);
    let listed: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["ip"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["8.8.8.8", "198.51.100.2", "203.0.113.7", "2001:db8::5"]);
    assert_eq!(
        (body["dryRun"].as_bool(), body["lastApply"]["enabled"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(body["lastApply"]["ok"], true);

    // Addresses expire 10 minutes after their last report; with none left the ports close
    clock.fetch_add(9 * 60_000, Ordering::SeqCst);
    allow(port, json!("8.8.8.8")).await;
    clock.fetch_add(60_000 + 1000, Ordering::SeqCst);
    helper.expire_now();
    helper.sync().settled(Duration::from_secs(5)).await;
    assert_eq!(helper.list(), ["8.8.8.8"]);
    assert_eq!(dry_runs(&audit).last().unwrap(), &strings(&["8.8.8.8"]));
    let mut expired: Vec<String> = audit_events(&audit)
        .iter()
        .filter(|e| e["event"] == "expired")
        .map(|e| e["ip"].as_str().unwrap().to_owned())
        .collect();
    expired.sort();
    assert_eq!(expired, ["198.51.100.2", "2001:db8::5", "203.0.113.7"]);
    clock.fetch_add(10 * 60_000, Ordering::SeqCst);
    helper.expire_now();
    helper.sync().settled(Duration::from_secs(5)).await;
    assert!(helper.list().is_empty());
    assert!(dry_runs(&audit).last().unwrap().is_empty());
    started.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn private_addresses_only_when_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let started = start(
        helper_config(dir.path(), &[("ALLOWLIST_ALLOW_PRIVATE", "1")]),
        Arc::new(AtomicI64::new(gateway::now_ms())),
    )
    .await;
    assert_eq!(allow(started.port, json!("127.0.0.1")).await.0, 200);
    assert_eq!(allow(started.port, json!("0.0.0.0")).await.0, 400);
    started.helper.sync().settled(Duration::from_secs(5)).await;
    assert_eq!(dry_runs(&started.audit_file).last().unwrap(), &strings(&["127.0.0.1"]));
    started.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_closes_and_saves_and_start_restores() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(gateway::now_ms()));
    let first = start(helper_config(dir.path(), &[]), clock.clone()).await;
    allow(first.port, json!("203.0.113.50")).await;
    first.helper.sync().settled(Duration::from_secs(5)).await;
    let (audit, state) = (first.audit_file.clone(), first.state_file.clone());
    first.close().await;
    let events = audit_events(&audit);
    assert!(dry_runs(&audit).last().unwrap().is_empty());
    assert_eq!(events.last().unwrap()["event"], "stop");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
    assert_eq!(saved["entries"][0]["ip"], "203.0.113.50");
    assert_eq!(saved["entries"].as_array().unwrap().len(), 1);

    let second = start(helper_config(dir.path(), &[]), clock).await;
    second.helper.sync().settled(Duration::from_secs(5)).await;
    let starts: Vec<Value> = audit_events(&audit)
        .into_iter()
        .filter(|e| e["event"] == "start")
        .collect();
    assert_eq!(starts.last().unwrap()["restored"], 1);
    assert_eq!(dry_runs(&audit).last().unwrap(), &strings(&["203.0.113.50"]));
    second.close().await;
}

/// `e2e.test.ts`: the gateway and the helper together, in process.
#[tokio::test(flavor = "multi_thread")]
async fn a_logged_in_player_gets_the_game_ports() {
    let _serial = serial().await;
    let dir = tempfile::tempdir().unwrap();
    let helper = start(
        helper_config(dir.path(), &[("ALLOWLIST_ALLOW_PRIVATE", "1")]),
        Arc::new(AtomicI64::new(gateway::now_ms())),
    )
    .await;
    let handler: Handler = Arc::new(|seen: Seen| {
        Box::pin(async move {
            let path = seen.url.split('?').next().unwrap().to_owned();
            if path == "/game/login" {
                let status: u16 = seen.url.split("status=").nth(1).map_or(200, |s| s.parse().unwrap());
                return Some((status, Vec::new(), Vec::new()));
            }
            if path == "/heartbeat" {
                let ok = seen.headers.get("authorization").map(String::as_str) == Some("Bearer good-token");
                return Some((
                    if ok { 200 } else { 401 },
                    vec![("content-type", "text/plain".into())],
                    if ok { b"20000".to_vec() } else { Vec::new() },
                ));
            }
            None
        })
    });
    let meta = start_upstream("metagame", Some(handler)).await;
    let cert = make_cert();
    let ports = Ports {
        metagame: meta.port,
        content: dead_port().await,
        ws: dead_port().await,
    };
    let feed = AllowlistFeed::new(&Allowlist {
        url: format!("http://127.0.0.1:{}", helper.port),
        secret: SECRET.into(),
        refresh: Duration::from_secs(60),
        timeout: Duration::from_secs(2),
        additional_urls: Vec::new(),
    });
    let running = start_gateway(&cert, test_config(&cert, &ports, &[]), Some(feed.clone())).await;
    let port = running.port;
    let post = |local: &'static str, bearer: Option<&str>| Options {
        method: Some("POST"),
        body: Some(b"{}".to_vec()),
        local: Some(local),
        headers: bearer
            .map(|b| vec![("authorization", b.to_owned())])
            .unwrap_or_default(),
        ..Options::default()
    };

    // A logged-in heartbeat opens the ports for exactly that address; a failed one does not
    assert_eq!(
        https_request(port, "/heartbeat", post("127.0.0.42", Some("Bearer stolen")))
            .await
            .unwrap()
            .status,
        401
    );
    assert_eq!(
        https_request(port, "/heartbeat", post("127.0.0.60", Some("Bearer good-token")))
            .await
            .unwrap()
            .status,
        200
    );
    feed.idle().await;
    helper.helper.sync().settled(Duration::from_secs(5)).await;
    assert_eq!(helper.helper.list(), ["127.0.0.60"]);
    assert_eq!(dry_runs(&helper.audit_file).last().unwrap(), &strings(&["127.0.0.60"]));
    assert!(!std::fs::read_to_string(&helper.audit_file).unwrap().contains(SECRET));

    // Game login opens them only for a bearer-authenticated successful answer
    let before = feed.sent();
    for status in [400, 401, 403, 500] {
        let reply = https_request(
            port,
            &format!("/game/login?status={status}"),
            post("127.0.0.43", Some("Bearer good-token")),
        )
        .await
        .unwrap();
        assert_eq!(reply.status, status);
    }
    for bearer in [None, Some("Basic ignored")] {
        assert_eq!(
            https_request(port, "/game/login", post("127.0.0.43", bearer))
                .await
                .unwrap()
                .status,
            200
        );
    }
    feed.idle().await;
    assert_eq!(feed.sent(), before);
    assert_eq!(
        https_request(port, "/game/login", post("127.0.0.44", Some("Bearer good-token")))
            .await
            .unwrap()
            .status,
        200
    );
    feed.idle().await;
    helper.helper.sync().settled(Duration::from_secs(5)).await;
    assert_eq!(feed.sent(), before + 1);
    assert!(helper.helper.list().contains(&"127.0.0.44".to_owned()));
    assert!(!helper.helper.list().contains(&"127.0.0.43".to_owned()));

    // A gateway with the wrong helper secret gets 401 and says so in its log
    let start_at = log_len();
    let wrong = AllowlistFeed::new(&Allowlist {
        url: format!("http://127.0.0.1:{}", helper.port),
        secret: "x".repeat(40),
        refresh: Duration::from_secs(60),
        timeout: Duration::from_secs(2),
        additional_urls: Vec::new(),
    });
    wrong.report("203.0.113.99");
    wrong.idle().await;
    assert!(!helper.helper.list().contains(&"203.0.113.99".to_owned()));
    assert!(log_since(start_at).iter().any(|(_, v)| {
        v["level"] == "warn"
            && v["msg"]
                .as_str()
                .unwrap_or("")
                .contains("allowlist helper did not take an address")
            && v["detail"] == "status 401"
    }));
    assert!(
        audit_events(&helper.audit_file)
            .iter()
            .any(|e| e["event"] == "unauthorized")
    );
    running.close().await;
    helper.close().await;
}

// ---- the binaries as processes ----

fn command(binary: &str, dir: &Path, env: &[(&str, String)]) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command.current_dir(dir).env_clear();
    for name in ["SystemRoot", "PATH", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
    command
}

#[test]
fn the_helper_wants_an_explicit_dry_run_setting() {
    let dir = tempfile::tempdir().unwrap();
    let output = command(
        env!("CARGO_BIN_EXE_allowlist"),
        dir.path(),
        &[("ALLOWLIST_SECRET", SECRET.into())],
    )
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ALLOWLIST_DRY_RUN must be set"));
}

#[test]
fn close_ports_closes_once_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.log");
    let output = command(
        env!("CARGO_BIN_EXE_allowlist"),
        dir.path(),
        &[
            ("ALLOWLIST_DRY_RUN", "1".into()),
            ("ALLOWLIST_SECRET", SECRET.into()),
            ("ALLOWLIST_AUDIT_LOG", audit.to_string_lossy().into_owned()),
        ],
    )
    .arg("--close-ports")
    .output()
    .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let events: Vec<String> = audit_events(&audit)
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(events, ["dry_run", "closed_ports"]);
    assert_eq!(audit_events(&audit)[0]["enabled"], false);
}

#[test]
fn the_gateway_refuses_bad_configuration_and_a_key_that_is_not_the_certificates() {
    let dir = tempfile::tempdir().unwrap();
    let missing = command(env!("CARGO_BIN_EXE_gateway"), dir.path(), &[])
        .arg("serve")
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Configuration: GATEWAY_CERT and GATEWAY_KEY"));

    let (cert, other) = (make_cert(), make_cert());
    let mismatch = command(
        env!("CARGO_BIN_EXE_gateway"),
        dir.path(),
        &[
            ("GATEWAY_CERT", cert.cert_file.to_string_lossy().into_owned()),
            ("GATEWAY_KEY", other.key_file.to_string_lossy().into_owned()),
            ("GATEWAY_SECRET", GATEWAY_SECRET.into()),
            ("ALLOWLIST_SECRET", SECRET.into()),
            ("GATEWAY_BIND", "127.0.0.1".into()),
            ("GATEWAY_PORT", "1".into()),
        ],
    )
    .arg("serve")
    .output()
    .unwrap();
    assert_eq!(mismatch.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(stderr.contains("Certificate ") && stderr.contains(" key "), "{stderr}");
}

/// Starts a binary and collects its JSON log lines until one says `ready`.
struct Child {
    child: tokio::process::Child,
    lines: Arc<std::sync::Mutex<Vec<Value>>>,
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn launch(mut command: std::process::Command, ready: &str) -> Child {
    use tokio::io::AsyncBufReadExt;
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let lines: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    for stream in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let lines = lines.clone();
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                lines
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&line).unwrap_or(json!({ "raw": line })));
            }
        });
    }
    let until = tokio::time::Instant::now() + Duration::from_secs(20);
    while !lines.lock().unwrap().iter().any(|l| l["msg"] == ready) {
        assert!(
            tokio::time::Instant::now() < until,
            "{ready}: {:?}",
            lines.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Child { child, lines }
}

#[tokio::test(flavor = "multi_thread")]
async fn helper_and_gateway_as_processes_open_the_ports_for_a_logged_in_player() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.log");
    let helper_port = dead_port().await;
    let helper = launch(
        command(
            env!("CARGO_BIN_EXE_allowlist"),
            dir.path(),
            &[
                ("ALLOWLIST_DRY_RUN", "1".into()),
                ("ALLOWLIST_ALLOW_PRIVATE", "1".into()),
                ("ALLOWLIST_SECRET", SECRET.into()),
                ("ALLOWLIST_PORT", helper_port.to_string()),
                ("ALLOWLIST_MIN_INTERVAL_MS", "200".into()),
                ("ALLOWLIST_AUDIT_LOG", audit.to_string_lossy().into_owned()),
                (
                    "ALLOWLIST_STATE_FILE",
                    dir.path().join("state.json").to_string_lossy().into_owned(),
                ),
            ],
        ),
        "allowlist helper ready",
    )
    .await;
    assert!(
        helper
            .lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l["msg"].as_str().unwrap_or("").starts_with("DRY RUN"))
    );

    let handler: Handler = Arc::new(|seen: Seen| {
        Box::pin(async move {
            let ok = seen.url == "/heartbeat"
                && seen.headers.get("authorization").map(String::as_str) == Some("Bearer good-token");
            ok.then(|| (200, Vec::new(), b"20000".to_vec()))
        })
    });
    let meta = start_upstream("metagame", Some(handler)).await;
    let cert = make_cert();
    let gateway_port = dead_port().await;
    let mut gateway_command = command(
        env!("CARGO_BIN_EXE_gateway"),
        dir.path(),
        &[
            ("GATEWAY_CERT", cert.cert_file.to_string_lossy().into_owned()),
            ("GATEWAY_KEY", cert.key_file.to_string_lossy().into_owned()),
            ("GATEWAY_SECRET", GATEWAY_SECRET.into()),
            ("GATEWAY_BIND", "127.0.0.1".into()),
            ("GATEWAY_PORT", gateway_port.to_string()),
            ("GATEWAY_METAGAME_URL", format!("http://127.0.0.1:{}", meta.port)),
            ("GATEWAY_CONTENT_URL", format!("http://127.0.0.1:{}", dead_port().await)),
            ("GATEWAY_WS_URL", format!("http://127.0.0.1:{}", dead_port().await)),
            ("ALLOWLIST_URL", format!("http://127.0.0.1:{helper_port}")),
            ("ALLOWLIST_SECRET", SECRET.into()),
        ],
    );
    gateway_command.arg("serve");
    let gateway = launch(gateway_command, "gateway ready").await;
    {
        let lines = gateway.lines.lock().unwrap();
        let ready = lines.iter().find(|l| l["msg"] == "gateway ready").unwrap();
        assert_eq!(ready["fingerprint"], cert.info.fingerprint.as_str());
        assert_eq!(ready["listen"], format!("127.0.0.1:{gateway_port}"));
        let text = serde_json::to_string(&*lines).unwrap();
        assert!(!text.contains(GATEWAY_SECRET) && !text.contains(SECRET));
    }

    let heartbeat = Options {
        method: Some("POST"),
        body: Some(b"{}".to_vec()),
        headers: vec![("authorization", "Bearer good-token".into())],
        pin: Some(&cert.info.fingerprint),
        ..Options::default()
    };
    let before = meta.count();
    assert_eq!(
        https_request(gateway_port, "/heartbeat", heartbeat)
            .await
            .unwrap()
            .status,
        200
    );
    let seen = &meta.seen()[before];
    assert_eq!(seen.headers["x-dauntless-gateway"], GATEWAY_SECRET);
    assert_eq!(seen.headers["x-forwarded-for"], "127.0.0.1");

    let until = tokio::time::Instant::now() + Duration::from_secs(5);
    while !dry_runs(&audit).iter().any(|ips| ips == &strings(&["127.0.0.1"])) {
        assert!(
            tokio::time::Instant::now() < until,
            "no dry-run rule change for the player"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (status, body) = call(helper_port, "GET", "/status", None, Some(SECRET)).await;
    assert_eq!((status, body["dryRun"].as_bool()), (200, Some(true)));
    assert_eq!(body["entries"][0]["ip"], "127.0.0.1");

    // An admin route through the real process: refused
    let admin = Options {
        method: Some("POST"),
        body: Some(b"{}".to_vec()),
        pin: Some(&cert.info.fingerprint),
        ..Options::default()
    };
    assert_eq!(
        https_request(gateway_port, "/undaunted/api/CreateInvite", admin)
            .await
            .unwrap()
            .status,
        403
    );
    drop(gateway);
    drop(helper);
}
