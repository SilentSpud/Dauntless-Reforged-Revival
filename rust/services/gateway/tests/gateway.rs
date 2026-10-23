#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! The gateway over TLS (`test/gateway.test.ts`): routing, header rules, the admin-route block, the
//! allowlist feed, the access log and WebSocket upgrades, against fake upstreams.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use gateway::config::Allowlist;
use gateway::feed::AllowlistFeed;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HELPER_SECRET: &str = "test-helper-secret-abcdefabcdefabcdefabcdefabcdef";
const FAKE_KEY: &str = "RRK_fedcba9876543210fedcba9876543210fedcba9876543210";

struct World {
    cert: TestCert,
    meta: Upstream,
    content: Upstream,
    helper: Upstream,
    ws: WsUpstream,
    feed: AllowlistFeed,
    gateway: Running,
}

fn answer(status: u16, content_type: &'static str, body: &str) -> Answer {
    Some((
        status,
        vec![("content-type", content_type.to_owned())],
        body.as_bytes().to_vec(),
    ))
}

async fn world() -> World {
    let cert = make_cert();
    let meta_handler: Handler = Arc::new(|seen: Seen| {
        Box::pin(async move {
            let path = seen.url.split('?').next().unwrap().to_lowercase();
            let header = |name: &str| seen.headers.get(name).map(String::as_str);
            match path.as_str() {
                "/heartbeat" => {
                    let ok =
                        header("authorization") == Some("Bearer good-token") || header("x-test-force-ok") == Some("1");
                    answer(if ok { 200 } else { 401 }, "text/plain", if ok { "20000" } else { "" })
                }
                "/account/api/oauth/token" => {
                    let ok = String::from_utf8_lossy(&seen.body).contains("exchange_code=good");
                    answer(
                        if ok { 200 } else { 400 },
                        "application/json",
                        if ok { "{\"access_token\":\"x\"}" } else { "" },
                    )
                }
                _ => None,
            }
        })
    });
    let meta = start_upstream("backend", Some(meta_handler)).await;
    let content = start_upstream("content", None).await;
    let helper = start_upstream(
        "helper",
        Some(Arc::new(|_| Box::pin(async { answer(200, "application/json", "{}") }))),
    )
    .await;
    let ws = start_ws_upstream().await;
    let feed = AllowlistFeed::new(&Allowlist {
        url: format!("http://127.0.0.1:{}", helper.port),
        secret: HELPER_SECRET.into(),
        refresh: Duration::from_secs(60),
        timeout: Duration::from_secs(2),
        additional_urls: Vec::new(),
    });
    let ports = Ports {
        backend: meta.port,
        content: content.port,
        ws: ws.port,
    };
    let config = test_config(&cert, &ports, &[]);
    let gateway = start_gateway(&cert, config, Some(feed.clone())).await;
    World {
        cert,
        meta,
        content,
        helper,
        ws,
        feed,
        gateway,
    }
}

fn get() -> Options<'static> {
    Options::default()
}

fn post(body: &str) -> Options<'static> {
    Options {
        method: Some("POST"),
        body: Some(body.as_bytes().to_vec()),
        ..Options::default()
    }
}

fn with(mut options: Options<'static>, name: &'static str, value: &str) -> Options<'static> {
    options.headers.push((name, value.to_owned()));
    options
}

fn from(mut options: Options<'static>, local: &'static str) -> Options<'static> {
    options.local = Some(local);
    options
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_its_certificate_and_a_pinning_client_connects() {
    let w = world().await;
    let port = w.gateway.port;
    let fingerprint = w.cert.info.fingerprint.clone();
    assert_eq!(fingerprint.len(), 64);
    assert_eq!(w.gateway.gateway.certificate().unwrap().fingerprint, fingerprint);
    let pinned = Options {
        pin: Some(&fingerprint),
        ..get()
    };
    let reply = https_request(port, "/party/invites", pinned).await.unwrap();
    assert_eq!((reply.status, reply.fingerprint.as_str()), (200, fingerprint.as_str()));
    let before = w.meta.count();
    let wrong = "0".repeat(64);
    let error = https_request(
        port,
        "/party/invites",
        Options {
            pin: Some(&wrong),
            ..get()
        },
    )
    .await
    .unwrap_err();
    assert!(error.contains("not the pinned"), "{error}");
    assert_eq!(w.meta.count(), before, "a request went out before the pin was checked");

    // 127.0.0.1 is an IP SAN, so ordinary validation against the certificate as its own root passes
    let tls = tls_connect_ca(port, &w.cert.cert_pem).await.unwrap();
    assert_eq!(request_on(tls, "/party/invites", &get()).await.unwrap().status, 200);
    assert!(w.cert.info.subject_alt_name.contains("IP Address:127.0.0.1"));
    let days = (w.cert.info.not_after - w.cert.info.not_before).num_days();
    assert!((3650..=3654).contains(&days), "valid {days} days");
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_plain_http() {
    let w = world().await;
    let mut tcp = tcp_connect(w.gateway.port, None).await.unwrap();
    tcp.write_all(b"GET /heartbeat HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    let (_, received) = wait_close(&mut tcp, Duration::from_secs(5)).await;
    assert!(
        !received.starts_with(b"HTTP/"),
        "answered plain HTTP: {}",
        String::from_utf8_lossy(&received)
    );

    // A TLS 1.1 ClientHello (no supported_versions extension) gets an alert, never a ServerHello
    let mut hello = vec![0x03, 0x02];
    hello.extend_from_slice(&[7u8; 32]);
    hello.extend_from_slice(&[0x00, 0x00, 0x02, 0x00, 0x2f, 0x01, 0x00]);
    let mut handshake = vec![0x01, 0x00, 0x00, hello.len() as u8];
    handshake.extend(hello);
    let mut record = vec![0x16, 0x03, 0x01, 0x00, handshake.len() as u8];
    record.extend(handshake);
    let mut tcp = tcp_connect(w.gateway.port, None).await.unwrap();
    tcp.write_all(&record).await.unwrap();
    let (_, received) = wait_close(&mut tcp, Duration::from_secs(5)).await;
    assert_eq!(received.first(), Some(&0x15), "an alert record: {received:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_content_and_the_rest_with_path_and_query_untouched() {
    let w = world().await;
    let port = w.gateway.port;
    let before = w.content.count();
    let manifest = https_request(port, "/content/v1/manifest", get()).await.unwrap().json();
    assert_eq!(
        (manifest["upstream"].as_str(), manifest["url"].as_str()),
        (Some("content"), Some("/content/v1/manifest"))
    );
    assert_eq!(w.content.count(), before + 1);

    let file = https_request(
        port,
        "/content/v1/files/Archon/Content/Paks/x.pak",
        with(with(get(), "x-reforged-user-api-key", FAKE_KEY), "range", "bytes=0-9"),
    )
    .await
    .unwrap()
    .json();
    assert_eq!(file["upstream"], "content");
    assert_eq!(file["headers"]["x-reforged-user-api-key"], FAKE_KEY);
    assert_eq!(file["headers"]["range"], "bytes=0-9");

    for path in [
        "/party/invites",
        "/entitlementsv2?accountId=UID-1&x=%20y",
        "/account127.0.0.1:61000",
        "/reforged/api/ServerStatus",
        "/QoS",
    ] {
        let reply = https_request(port, path, get()).await.unwrap();
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(
            (reply.json()["upstream"].as_str(), reply.json()["url"].as_str()),
            (Some("backend"), Some(path))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_bodies_and_drops_hop_by_hop_headers() {
    let w = world().await;
    let body = json!({ "characterId": "c", "updateVersion": 3, "data": "x".repeat(20_000) }).to_string();
    let mut options = post(&body);
    for (name, value) in [
        ("content-type", "application/json"),
        ("authorization", "Bearer good-token"),
        ("keep-alive", "timeout=5"),
        ("x-custom", "kept"),
        ("connection", "x-custom-hop, keep-alive"),
        ("x-custom-hop", "dropped"),
    ] {
        options = with(options, name, value);
    }
    let reply = https_request(w.gateway.port, "/character", options).await.unwrap();
    let echo = reply.json();
    assert_eq!(echo["method"], "POST");
    assert_eq!(echo["bodyLength"], body.len());
    assert_eq!(echo["bodySha"], sha256_hex(body.as_bytes()));
    assert_eq!(echo["headers"]["authorization"], "Bearer good-token");
    assert_eq!(echo["headers"]["x-custom"], "kept");
    assert!(echo["headers"].get("x-custom-hop").is_none());
    assert!(echo["headers"].get("keep-alive").is_none());
    assert!(!reply.headers.contains_key("x-powered-by"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sets_the_forwarded_address_and_secret_whatever_the_client_claims() {
    let w = world().await;
    for (local, path) in [
        ("127.0.0.1", "/party"),
        ("127.0.0.2", "/content/v1/news"),
        ("127.0.0.3", "/reforged/api/GetUserInfo"),
    ] {
        let mut options = from(get(), local);
        for (name, value) in [
            ("x-forwarded-for", "6.6.6.6, 7.7.7.7"),
            ("X-Real-IP", "6.6.6.6"),
            ("forwarded", "for=6.6.6.6"),
            ("x-forwarded-proto", "http"),
            ("x-forwarded-host", "evil.example"),
            ("x-reforged-gateway", "forged"),
        ] {
            options = with(options, name, value);
        }
        let reply = https_request(w.gateway.port, path, options).await.unwrap();
        let headers = reply.json()["headers"].clone();
        assert_eq!(headers["x-forwarded-for"], local, "{path}");
        assert_eq!(headers["x-reforged-gateway"], GATEWAY_SECRET, "{path}");
        assert_eq!(headers["x-forwarded-proto"], "https");
        for gone in ["x-real-ip", "forwarded", "x-forwarded-host"] {
            assert!(headers.get(gone).is_none(), "{gone}");
        }
        // The secret never goes back to the client
        assert!(!reply.headers.contains_key("x-reforged-gateway"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_game_server_key_header_is_refused_and_never_forwarded() {
    let w = world().await;
    let (meta_before, content_before) = (w.meta.count(), w.content.count());
    for (method, path) in [
        ("POST", "/heartbeat"),
        ("POST", "/progression/UID-1"),
        ("PUT", "/cooldown/batch/UID-1"),
        ("GET", "/content/v1/manifest"),
        ("POST", "/reforged/api/Register"),
        ("GET", "/party/invites"),
    ] {
        let mut options = Options {
            method: Some(method),
            body: (method != "GET").then(|| b"{}".to_vec()),
            ..get()
        };
        options
            .headers
            .push(("x-mysticparadox-gameserver-apikey", "0123456789abcdef".repeat(3)));
        options.headers.push(("authorization", "Bearer good-token".into()));
        let reply = https_request(w.gateway.port, path, options).await.unwrap();
        assert_eq!(
            (reply.status, reply.json()),
            (403, json!({ "error": "forbidden" })),
            "{method} {path}"
        );
    }
    let upper = https_request(
        w.gateway.port,
        "/party",
        with(get(), "X-MysticParadox-GameServer-ApiKey", "k"),
    )
    .await
    .unwrap();
    assert_eq!(upper.status, 403);
    assert_eq!((w.meta.count(), w.content.count()), (meta_before, content_before));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_public_reforged_routes_reach_the_backend() {
    let w = world().await;
    let port = w.gateway.port;
    let public = |method: &str, path: &str| {
        let name = path["/reforged/api/".len()..].to_lowercase();
        gateway::policy::PUBLIC_REFORGED_API
            .iter()
            .any(|(route, methods)| *route == name && methods.contains(&method))
    };
    // Every route the backend registers, read from its source, plus two it never had
    let mut routes = reforged_api_routes();
    routes.push(("POST".into(), "/reforged/api/RegistrationStatus".into()));
    routes.push(("GET".into(), "/reforged/api/GetAllUsers".into()));
    let mut blocked = 0;
    for (method, path) in &routes {
        let (method, path) = (method.as_str(), path.as_str());
        let public = public(method, path);
        blocked += usize::from(!public);
        let before = w.meta.count();
        let options = Options {
            method: Some(method),
            body: (method != "GET").then(|| b"{}".to_vec()),
            headers: vec![
                ("x-reforged-user-api-key", FAKE_KEY.into()),
                ("content-type", "application/json".into()),
            ],
            ..get()
        };
        let reply = https_request(port, path, options).await.unwrap();
        if public {
            assert_eq!(reply.status, 200, "{method} {path}");
            assert_eq!((w.meta.count(), w.meta.last().url), (before + 1, path.to_owned()));
        } else {
            assert_eq!(
                (reply.status, reply.json()),
                (403, json!({ "error": "forbidden" })),
                "{method} {path}"
            );
            assert_eq!(w.meta.count(), before, "{method} {path} reached the backend");
        }
    }
    assert!(blocked >= 4, "blocked {blocked}");
    for path in [
        "/REFORGED/API/CREATEINVITE",
        "/reforged/api/createinvite/",
        "/reforged//api/CreateInvite",
        "/reforged/api/%43reateInvite",
        "/reforged/api/GrantEntitlement?UserId=x",
    ] {
        let before = w.meta.count();
        let reply = https_request(port, path, with(post("{}"), "content-type", "application/json"))
            .await
            .unwrap();
        assert_eq!(reply.status, 403, "{path}");
        assert_eq!(w.meta.count(), before, "{path}");
    }
    for path in ["/reforged/api/Register/../CreateInvite", "/reforged%2Fapi/CreateInvite"] {
        assert_eq!(
            https_request(port, path, post("{}")).await.unwrap().status,
            400,
            "{path}"
        );
    }
    // A backslash never gets past the HTTP parser
    let backslash = https_request(port, "/reforged/api\\CreateInvite", post("{}")).await;
    assert!(backslash.is_err() || backslash.unwrap().status == 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_logged_in_peers_to_the_allowlist_helper_and_only_them() {
    let w = world().await;
    let port = w.gateway.port;
    let allowed = || -> Vec<String> {
        w.helper
            .seen()
            .iter()
            .filter(|s| s.url == "/allow")
            .map(|s| {
                serde_json::from_slice::<serde_json::Value>(&s.body).unwrap()["ip"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    };
    let token_form = |code: &str| {
        with(
            post(&format!("grant_type=exchange_code&exchange_code={code}")),
            "content-type",
            "application/x-www-form-urlencoded",
        )
    };

    // Not logged in: 401 upstream, nothing reported
    https_request(
        port,
        "/heartbeat",
        from(with(post("{}"), "authorization", "Bearer wrong"), "127.0.0.4"),
    )
    .await
    .unwrap();
    https_request(port, "/heartbeat", from(post("{}"), "127.0.0.4"))
        .await
        .unwrap();
    // 200 without a bearer token (an upstream that stopped checking): not reported either
    https_request(
        port,
        "/heartbeat",
        from(with(post("{}"), "x-test-force-ok", "1"), "127.0.0.4"),
    )
    .await
    .unwrap();
    // Other routes and other methods never report
    https_request(
        port,
        "/heartbeat",
        from(with(get(), "authorization", "Bearer good-token"), "127.0.0.4"),
    )
    .await
    .unwrap();
    https_request(
        port,
        "/party",
        from(with(post("{}"), "authorization", "Bearer good-token"), "127.0.0.4"),
    )
    .await
    .unwrap();
    https_request(port, "/account/api/oauth/token", from(token_form("bad"), "127.0.0.4"))
        .await
        .unwrap();
    w.feed.idle().await;
    assert!(allowed().is_empty());

    let heartbeat = https_request(
        port,
        "/heartbeat",
        from(
            with(
                with(post("{\"map\":\"x\"}"), "authorization", "Bearer good-token"),
                "content-type",
                "application/json",
            ),
            "127.0.0.5",
        ),
    )
    .await
    .unwrap();
    assert_eq!((heartbeat.status, heartbeat.text().as_str()), (200, "20000"));
    let token = https_request(port, "/account/api/oauth/token", from(token_form("good"), "127.0.0.6"))
        .await
        .unwrap();
    assert_eq!(token.status, 200);
    w.feed.idle().await;
    let mut ips = allowed();
    ips.sort();
    assert_eq!(ips, ["127.0.0.5", "127.0.0.6"]);
    let first = w.helper.seen().into_iter().find(|s| s.url == "/allow").unwrap();
    assert_eq!(first.method, "POST");
    assert_eq!(first.headers["x-allowlist-secret"], HELPER_SECRET);
    assert_eq!(first.headers["content-type"], "application/json");

    // Heartbeats every 20 s reach the helper once per refresh period
    for _ in 0..3 {
        https_request(
            port,
            "/heartbeat",
            from(with(post("{}"), "authorization", "Bearer good-token"), "127.0.0.5"),
        )
        .await
        .unwrap();
    }
    w.feed.idle().await;
    assert_eq!(allowed().iter().filter(|ip| *ip == "127.0.0.5").count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_access_log_line_per_request_with_no_secret_in_it() {
    let _serial = serial().await;
    let w = world().await;
    let port = w.gateway.port;
    let start = log_len();
    let bearer = format!("Bearer {FAKE_JWT}");
    // Its own client address: other tests log at the same time
    let kill = Options {
        local: Some("127.0.0.40"),
        method: Some("DELETE"),
        headers: vec![
            ("authorization", bearer.clone()),
            ("x-reforged-user-api-key", FAKE_KEY.into()),
        ],
        ..get()
    };
    https_request(port, &format!("/account/api/oauth/sessions/kill/{FAKE_JWT}"), kill)
        .await
        .unwrap();
    https_request(
        port,
        &format!("/entitlementsv2?accountId=UID-1&token={FAKE_JWT}"),
        from(with(get(), "authorization", &bearer), "127.0.0.40"),
    )
    .await
    .unwrap();
    https_request(
        port,
        "/reforged/api/CreateInvite",
        from(
            with(
                post("{\"InviteCode\":\"SECRET-CODE-1234\"}"),
                "x-reforged-user-api-key",
                FAKE_KEY,
            ),
            "127.0.0.40",
        ),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let lines = log_since(start);
    let requests: Vec<&serde_json::Value> = lines
        .iter()
        .map(|(_, v)| v)
        .filter(|v| v["msg"] == "request" && v["ip"] == "127.0.0.40")
        .collect();
    assert_eq!(requests.len(), 3, "{requests:?}");
    let text: String = lines.iter().map(|(raw, _)| raw.as_str()).collect::<Vec<_>>().join("\n");
    for secret in [
        FAKE_JWT,
        FAKE_KEY,
        GATEWAY_SECRET,
        HELPER_SECRET,
        "SECRET-CODE-1234",
        "good-token",
    ] {
        assert!(!text.contains(secret), "the log contains {}...", &secret[..12]);
    }
    let kill = requests.iter().find(|v| v["method"] == "DELETE").unwrap();
    assert_eq!(kill["target"], "/account/api/oauth/sessions/kill/<token>");
    assert_eq!(
        (kill["route"].as_str(), kill["status"].as_u64(), kill["ip"].as_str()),
        (Some("backend"), Some(200), Some("127.0.0.40"))
    );
    let entitlements = requests
        .iter()
        .find(|v| v["target"].as_str().unwrap().starts_with("/entitlementsv2"))
        .unwrap();
    assert_eq!(
        entitlements["target"],
        "/entitlementsv2?accountId=UID-1&token=<redacted>"
    );
    let admin = requests.iter().find(|v| v["method"] == "POST").unwrap();
    assert_eq!(
        (
            admin["status"].as_u64(),
            admin["route"].as_str(),
            admin["reason"].as_str()
        ),
        (Some(403), Some("gateway"), Some("admin_route"))
    );
    for field in [
        "t", "ip", "method", "target", "route", "status", "bytesIn", "bytesOut", "ms",
    ] {
        assert!(kill.get(field).is_some(), "{field}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn server_status_keys_are_forwarded_byte_for_byte_and_never_logged() {
    let _serial = serial().await;
    let w = world().await;
    let port = w.gateway.port;
    let start = log_len();
    // A key that does not look like one: it stays out of the log only because headers never go there
    let odd_key = format!("k3y-{}", &sha256_hex(b"odd")[..16]);
    let cases = [
        ("GET", "/reforged/api/ServerStatus", FAKE_KEY.to_owned()),
        ("GET", "/reforged/api/serverstatus", odd_key.clone()),
        ("HEAD", "/reforged/api/ServerStatus", FAKE_KEY.to_owned()),
    ];
    for (method, path, key) in &cases {
        let before = w.meta.count();
        let reply = https_request(
            port,
            path,
            Options {
                method: Some(method),
                local: Some("127.0.0.41"),
                headers: vec![("X-Reforged-User-Api-Key", key.clone())],
                ..get()
            },
        )
        .await
        .unwrap();
        assert_eq!(reply.status, 200, "{method} {path}");
        assert_eq!(w.meta.count(), before + 1, "{method} {path} reached the backend");
        let seen = w.meta.last();
        assert_eq!((seen.method.as_str(), seen.url.as_str()), (*method, *path));
        assert_eq!(seen.headers["x-reforged-user-api-key"], *key, "forwarded byte for byte");
        assert_eq!(seen.headers["x-reforged-gateway"], GATEWAY_SECRET);
        assert_eq!(seen.headers["x-forwarded-for"], "127.0.0.41");
    }
    let anonymous = https_request(port, "/reforged/api/ServerStatus", from(get(), "127.0.0.41"))
        .await
        .unwrap();
    assert_eq!(anonymous.status, 200);
    assert!(!w.meta.last().headers.contains_key("x-reforged-user-api-key"));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let lines = log_since(start);
    let requests: Vec<&serde_json::Value> = lines
        .iter()
        .map(|(_, v)| v)
        .filter(|v| v["msg"] == "request" && v["ip"] == "127.0.0.41")
        .collect();
    assert_eq!(requests.len(), cases.len() + 1);
    let text: String = lines.iter().map(|(raw, _)| raw.as_str()).collect::<Vec<_>>().join("\n");
    for secret in [FAKE_KEY, odd_key.as_str(), GATEWAY_SECRET] {
        assert!(!text.contains(secret), "the log contains {}...", &secret[..6]);
    }
    assert!(
        !text.to_lowercase().contains("x-reforged-user-api-key"),
        "no header names or values in the access log"
    );
    let allowed = [
        "t", "level", "msg", "ip", "method", "target", "route", "status", "bytesIn", "bytesOut", "ms", "ua",
    ];
    for line in requests {
        let extra: Vec<&String> = object(line).keys().filter(|k| !allowed.contains(&k.as_str())).collect();
        assert!(extra.is_empty(), "an access-log line grew a field: {extra:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_websocket_upgrades_byte_for_byte() {
    let w = world().await;
    let mut result = upgrade(
        w.gateway.port,
        "/xmpp?room=ramsgate",
        &[("Sec-WebSocket-Protocol", "xmpp")],
        None,
    )
    .await;
    assert_eq!(result.status, 101);
    assert_eq!(
        result.header("sec-websocket-accept"),
        Some(ws_accept(&result.key).as_str())
    );
    result.stream.write_all(b"hello over tls").await.unwrap();
    let mut got = vec![0u8; "echo:hello over tls".len()];
    tokio::time::timeout(Duration::from_secs(3), result.stream.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, b"echo:hello over tls");
    let (_, seen) = w.ws.last();
    assert_eq!(seen["x-forwarded-for"], "127.0.0.1");
    assert_eq!(seen["x-reforged-gateway"], GATEWAY_SECRET);
    assert_eq!(seen["sec-websocket-protocol"], "xmpp");
    assert_eq!(w.gateway.gateway.stats().websockets, 1);
    drop(result);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(w.gateway.gateway.stats().websockets, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn carries_the_clients_chat_connection_as_libwebsockets_sends_it() {
    let w = world().await;
    let mut socket = tls_connect(w.gateway.port, None).await.unwrap();
    let request = [
        "GET // HTTP/1.1",
        "Pragma: no-cache",
        "Cache-Control: no-cache",
        "Host: 127.0.0.1",
        "Origin: http://127.0.0.1",
        "Upgrade: websocket",
        "Connection: Upgrade",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
        "Sec-WebSocket-Protocol: xmpp",
        "Sec-WebSocket-Version: 13",
        "",
        "",
    ]
    .join("\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let (status, headers, _) = read_head(&mut socket).await;
    assert_eq!(status, 101);
    // libwebsockets checks these: one Connection: Upgrade, and the protocol it asked for
    let count = |name: &str, value: &str| {
        headers
            .iter()
            .filter(|(n, v)| n.eq_ignore_ascii_case(name) && v.eq_ignore_ascii_case(value))
            .count()
    };
    assert_eq!(count("connection", "upgrade"), 1, "{headers:?}");
    assert_eq!(count("sec-websocket-protocol", "xmpp"), 1, "{headers:?}");
    let (url, seen) = w.ws.last();
    assert_eq!(url, "//", "the upstream sees the request target as sent");
    assert_eq!(seen["x-reforged-gateway"], GATEWAY_SECRET);
    assert_eq!(seen["x-forwarded-for"], "127.0.0.1");
    assert_eq!(seen["sec-websocket-protocol"], "xmpp");
}

#[tokio::test(flavor = "multi_thread")]
async fn upgrades_follow_the_same_rules_and_pass_on_a_refusal() {
    let w = world().await;
    let port = w.gateway.port;
    let before = w.ws.count();
    assert_eq!(
        upgrade(port, "/xmpp", &[("x-mysticparadox-gameserver-apikey", "k")], None)
            .await
            .status,
        403
    );
    assert_eq!(upgrade(port, "/reforged/api/ServerStatus", &[], None).await.status, 403);
    assert_eq!(upgrade(port, "/a/../xmpp", &[], None).await.status, 400);
    assert_eq!(w.ws.count(), before);
    let refused = upgrade(port, "/refuse", &[], None).await;
    assert_eq!(refused.status, 404);
    assert_eq!(refused.body().await, "no room");
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_502_when_an_upstream_is_down() {
    let cert = make_cert();
    let dead = dead_port().await;
    let ports = Ports {
        backend: dead,
        content: dead,
        ws: dead,
    };
    let running = start_gateway(&cert, test_config(&cert, &ports, &[]), None).await;
    let ws = upgrade(running.port, "/xmpp", &[], None).await;
    assert_eq!(ws.status, 502);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ws.body().await).unwrap(),
        json!({ "error": "bad_gateway" })
    );
    let http = https_request(running.port, "/heartbeat", post("{}")).await.unwrap();
    assert_eq!((http.status, http.json()), (502, json!({ "error": "bad_gateway" })));
    running.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn private_mode_serves_plain_http_with_the_same_rules() {
    let backend = start_upstream("backend", None).await;
    let content = start_upstream("content", None).await;
    let (backend_url, content_url) = (
        format!("http://127.0.0.1:{}", backend.port),
        format!("http://127.0.0.1:{}", content.port),
    );
    let env = service_kit::env::Env::from_pairs([
        ("GATEWAY_TLS", "0"),
        ("GATEWAY_BIND", "127.0.0.1"),
        ("GATEWAY_SECRET", GATEWAY_SECRET),
        ("GATEWAY_BACKEND_URL", backend_url.as_str()),
        ("GATEWAY_CONTENT_URL", content_url.as_str()),
        ("GATEWAY_WS_URL", backend_url.as_str()),
        ("GATEWAY_ALLOWLIST", "0"),
        ("GATEWAY_SHUTDOWN_GRACE_MS", "500"),
    ]);
    let config = gateway::config::Config::from_env(&env).unwrap();
    // A public gateway without its certificate is a programming error
    let cert = make_cert();
    let public = test_config(
        &cert,
        &Ports {
            backend: 1,
            content: 1,
            ws: 1,
        },
        &[],
    );
    assert!(gateway::gateway::Gateway::new(public, None, None, None).is_err());

    let running = start_gateway_with(None, config, None, None).await;
    assert!(running.gateway.certificate().is_none());
    let port = running.port;
    let target = "/__origin/gamedev-prod.steelyard.ca/party/invites";
    assert_eq!(plain_request(port, target, get()).await.unwrap().status, 200);
    let seen = backend.last();
    assert_eq!(seen.url, target, "the target goes upstream unchanged");
    assert_eq!(seen.headers["x-forwarded-proto"], "http");
    assert_eq!(seen.headers["x-forwarded-for"], "127.0.0.1");
    assert_eq!(seen.headers["x-reforged-gateway"], GATEWAY_SECRET);
    assert_eq!(
        plain_request(port, "/content/v1/runtime/manifest", get())
            .await
            .unwrap()
            .status,
        200
    );
    assert_eq!(content.count(), 1);
    let before = backend.count();
    for path in ["/reforged/api/CreateInvite", "/launcher/v1/auth/register"] {
        assert_eq!(
            plain_request(port, path, post("{}")).await.unwrap().status,
            403,
            "{path}"
        );
    }
    assert_eq!(backend.count(), before, "nothing blocked reached the backend");
    // TLS is not spoken here
    assert!(tls_connect(port, None).await.is_err());
    running.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_origin_prefix_and_the_host_only_routes_end_to_end() {
    let _serial = serial().await;
    let w = world().await;
    let port = w.gateway.port;
    let host = "/__origin/gamedev-prod.steelyard.ca";
    let reply = https_request(port, &format!("{host}/party/invites"), get())
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(w.meta.last().url, format!("{host}/party/invites"));
    let before = (w.meta.count(), w.content.count());
    for (path, status, reason) in [
        (format!("{host}/reforged/api/CreateInvite"), 403, "admin_route"),
        (format!("{host}/admin/v1/players"), 403, "host_only_route"),
        ("/internal/v1/metrics".to_owned(), 403, "host_only_route"),
        ("/health/ready".to_owned(), 403, "host_only_route"),
        (format!("{host}/__origin/other/x"), 400, "bad_origin"),
        ("/__origin/a..b/x".to_owned(), 400, "bad_origin"),
    ] {
        let start = log_len();
        let reply = https_request(port, &path, get()).await.unwrap();
        assert_eq!(reply.status, status, "{path}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            log_since(start).iter().any(|(_, line)| line["reason"] == reason),
            "{path} logs {reason}"
        );
    }
    for name in ["x-reforged-admin-key", "x-mysticparadox-metrics-token"] {
        let reply = https_request(port, "/reforged/api/ServerStatus", with(get(), name, "x"))
            .await
            .unwrap();
        assert_eq!(reply.status, 403, "{name}");
    }
    // The runtime's content path goes to the backend, never to the content server
    let reply = https_request(port, &format!("{host}/content/v1/runtime/manifest"), get())
        .await
        .unwrap();
    assert_eq!(reply.json()["upstream"], "backend");
    assert_eq!((w.meta.count(), w.content.count()), (before.0 + 1, before.1));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_capture_records_requests_answers_and_chat_redacted() {
    use gateway::capture::{CaptureOptions, CaptureWriter};

    const KEY: &str = "RRK_0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f";
    const GS_KEY: &str = "ffffffffffffffffffffffffffffffffffffffffffffffff";
    let key_hash = sha256_hex(KEY.as_bytes());
    let cert = make_cert();
    let handler: Handler = Arc::new(|seen: Seen| {
        Box::pin(async move {
            let path = seen.url.split('?').next().unwrap().to_owned();
            path.ends_with("/account/api/oauth/token").then(|| {
                let text = json!({ "access_token": FAKE_JWT, "account_id": "UID-test", "expires_in": 86400 });
                (
                    200,
                    vec![("content-type", "application/json".to_owned())],
                    text.to_string().into_bytes(),
                )
            })
        })
    });
    let backend = start_upstream("backend", Some(handler)).await;
    let ws = start_ws_upstream().await;
    let ports = Ports {
        backend: backend.port,
        content: dead_port().await,
        ws: ws.port,
    };
    let dir = tempfile::tempdir().unwrap();
    let options = CaptureOptions {
        dir: dir.path().to_path_buf(),
        retention_days: 14,
        max_body_bytes: 4096,
        source: "gateway".into(),
    };
    let writer = CaptureWriter::new(options, Box::new(|_| {}), Arc::new(gateway::now_ms)).unwrap();
    let config = test_config(&cert, &ports, &[]);
    let running = start_gateway_with(Some(&cert), config, None, Some(writer)).await;
    let port = running.port;

    let login = Options {
        method: Some("POST"),
        headers: vec![("content-type", "application/x-www-form-urlencoded".into())],
        body: Some(format!("grant_type=password&username=Alex&password={KEY}").into_bytes()),
        ..get()
    };
    let target = "/__origin/account-public-service-prod.ol.epicgames.com/account/api/oauth/token";
    assert_eq!(https_request(port, target, login).await.unwrap().status, 200);
    let profile = Options {
        method: Some("POST"),
        headers: vec![
            ("authorization", format!("bearer {FAKE_JWT}")),
            ("content-type", "application/json".into()),
            ("x-reforged-user-api-key", KEY.into()),
        ],
        body: Some(br#"{"profileId":"athena"}"#.to_vec()),
        ..get()
    };
    assert_eq!(https_request(port, "/character", profile).await.unwrap().status, 200);
    let refused = https_request(
        port,
        "/reforged/admin/anything",
        with(get(), "x-mysticparadox-gameserver-apikey", GS_KEY),
    )
    .await
    .unwrap();
    assert_eq!(refused.status, 403);

    // Chat: a SASL PLAIN login from the game, masked as a client's frames are
    let mut chat = upgrade(port, "//", &[("sec-websocket-protocol", "xmpp")], None).await;
    assert_eq!(chat.status, 101);
    let plain = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("\0UID-chat-a\0{FAKE_JWT}"),
    );
    let stanza = format!("<auth xmlns=\"urn:ietf:params:xml:ns:xmpp-sasl\" mechanism=\"PLAIN\">{plain}</auth>");
    let mask = [7u8, 1, 9, 3];
    let mut frame = vec![0x81, 0x80 | 126];
    frame.extend_from_slice(&(stanza.len() as u16).to_be_bytes());
    frame.extend_from_slice(&mask);
    frame.extend(stanza.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    chat.stream.write_all(&frame).await.unwrap();
    let mut echo = vec![0u8; 5];
    chat.stream.read_exact(&mut echo).await.unwrap();
    assert_eq!(&echo, b"echo:");
    drop(chat);
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.close().await;

    let mut text = String::new();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        text.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let http: Vec<&serde_json::Value> = lines.iter().filter(|l| l["kind"] == "http").collect();
    assert_eq!(http.len(), 3, "{text}");
    let (first, second, third) = (http[0], http[1], http[2]);
    assert_eq!(
        (&first["src"], &first["route"], &first["status"], &first["method"]),
        (&json!("gateway"), &json!("backend"), &json!(200), &json!("POST"))
    );
    assert_eq!(first["target"], target);
    assert_eq!(
        first["reqBody"],
        json!({ "form": [["grant_type", "password"], ["username", "Alex"], ["password", format!("<key:{key_hash}>")]] })
    );
    assert_eq!(
        first["resBody"],
        json!({ "json": { "access_token": "<token:UID-test>", "account_id": "UID-test", "expires_in": 86400 } })
    );
    assert_eq!(second["reqHeaders"]["authorization"], "bearer <token:UID-test>");
    assert_eq!(
        second["reqHeaders"]["x-reforged-user-api-key"],
        format!("<key:{key_hash}>")
    );
    assert_eq!(second["reqBody"], json!({ "json": { "profileId": "athena" } }));
    assert_eq!(second["resBody"]["json"]["upstream"], "backend");
    assert_eq!(
        second["resBody"]["json"]["headers"]["x-reforged-gateway"], "<redacted>",
        "the gateway secret echoed by the backend is cut"
    );
    assert_eq!((&third["status"], &third["route"]), (&json!(403), &json!("gateway")));
    assert_eq!(third["reqHeaders"]["x-mysticparadox-gameserver-apikey"], "<gskey>");
    assert_eq!(third["resBody"], json!({ "json": { "error": "forbidden" } }));
    let seq: Vec<u64> = http.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert!(seq[0] < seq[1] && seq[1] < seq[2]);
    // Each request was on its own connection
    let conns: std::collections::HashSet<u64> = http.iter().map(|l| l["conn"].as_u64().unwrap()).collect();
    assert_eq!(conns.len(), 3);

    let chat: Vec<&serde_json::Value> = lines.iter().filter(|l| l["kind"] == "chat").collect();
    assert_eq!(chat.first().unwrap()["event"], "open", "{text}");
    assert_eq!(chat.first().unwrap()["target"], "//");
    assert_eq!(chat.last().unwrap()["event"], "close");
    let login = chat.iter().find(|l| l["dir"] == "in").unwrap();
    assert_eq!(
        login["text"],
        "<auth xmlns=\"urn:ietf:params:xml:ns:xmpp-sasl\" mechanism=\"PLAIN\"><plain:UID-chat-a></auth>"
    );
    assert!(!text.contains(KEY) && !text.contains(FAKE_JWT) && !text.contains(GS_KEY));
    assert!(!text.contains(&plain), "nor the chat login");
    assert!(!text.contains(GATEWAY_SECRET), "nor the gateway secret");
}
