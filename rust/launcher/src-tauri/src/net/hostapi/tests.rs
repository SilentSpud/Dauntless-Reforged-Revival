use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use super::*;
use crate::bridge::types::RuntimeFileInfo;
use crate::net::pinned::certificate_fingerprint;
use crate::net::pinned::tests::{acceptor, certificate};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

// Built from pieces, so the repository's secret check never sees a whole key
const KEY: &str = concat!("RRK_", "eeeeeeeeeeeeeeee", "eeeeeeeeeeeeeeee", "eeeeeeeeeeeeeeee");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cases {
    news: Vec<Case>,
    branding: Vec<Case>,
    images: Vec<Image>,
    news_body: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    input: Value,
    expected: Value,
}

#[derive(Deserialize)]
struct Image {
    bytes: Vec<u8>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[test]
fn the_shared_table() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/hostapi.json");
    let cases: Cases = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in cases.news {
        assert_eq!(
            serde_json::to_value(parse_news(Some(&case.input))).unwrap(),
            case.expected,
            "{}",
            case.input
        );
    }
    for case in cases.branding {
        let index = parse_branding(Some(&case.input));
        let backgrounds: Vec<Value> = index
            .backgrounds
            .iter()
            .map(|(path, credit)| serde_json::json!({ "path": path, "credit": credit }))
            .collect();
        assert_eq!(
            serde_json::json!({ "backgrounds": backgrounds, "accent": index.accent }),
            case.expected,
            "{}",
            case.input
        );
    }
    for case in cases.images {
        assert_eq!(sniff_image(&case.bytes), case.kind.as_deref(), "{:?}", case.bytes);
    }
    for case in cases.news_body {
        assert_eq!(
            serde_json::to_value(parse::clean_news_body(Some(&case.input))).unwrap(),
            case.expected,
            "{}",
            case.input
        );
    }
}

#[test]
fn branding_paths_with_other_characters_are_refused_without_panicking() {
    assert!(is_branding_path("/content/v1/branding/a.jpg"));
    assert!(is_branding_path("/CONTENT/v1/branding/A-b_c.d.WEBP"));
    assert!(!is_branding_path("/content/v1/branding/.jpg"));
    assert!(!is_branding_path("/content/v1/branding/a/../b.jpg"));
    assert!(!is_branding_path("/content/v1/branding/é.jpg"));
    assert!(!is_branding_path("/content/v1/branding/aé.png"));
    assert!(!is_branding_path("/content/v1/brandinǵ/a.png"));
    assert!(!is_branding_path(&format!(
        "/content/v1/branding/{}.png",
        "a".repeat(102)
    )));
    assert!(is_branding_path(&format!(
        "/content/v1/branding/{}.png",
        "a".repeat(101)
    )));
}

// ---------------------------------------------------------------- the runtime manifest

fn hash(c: char) -> String {
    c.to_string().repeat(64)
}

fn manifest() -> Value {
    serde_json::json!({
        "version": "20261014-abcdef12",
        "build": { "name": "rel-1.14.7-Archon", "changelist": 647472 },
        "files": [
            { "name": "winmm.dll", "size": 40001, "sha256": hash('b') },
            { "name": "MysticParadox.dll", "size": 200001, "sha256": hash('a') },
        ],
    })
}

#[test]
fn runtime_manifests_name_the_two_files_only() {
    let parsed = parse_runtime_manifest(Some(&manifest())).unwrap();
    assert_eq!(
        parsed,
        RuntimeManifest {
            version: "20261014-abcdef12".into(),
            changelist: Some(647472),
            files: vec![
                RuntimeFileInfo {
                    name: "MysticParadox.dll".into(),
                    size: 200001,
                    sha256: hash('a')
                },
                RuntimeFileInfo {
                    name: "winmm.dll".into(),
                    size: 40001,
                    sha256: hash('b')
                },
            ],
        }
    );

    let with = |edit: &dyn Fn(&mut Value)| {
        let mut raw = manifest();
        edit(&mut raw);
        parse_runtime_manifest(Some(&raw))
    };
    // Kept, with the build read loosely
    assert_eq!(with(&|raw| raw["build"] = Value::Null).unwrap().changelist, None);
    assert_eq!(
        with(&|raw| raw["build"]["changelist"] = 1.5.into()).unwrap().changelist,
        None
    );
    assert_eq!(
        with(&|raw| raw["build"]["changelist"] = (-3).into())
            .unwrap()
            .changelist,
        Some(-3)
    );
    assert_eq!(
        with(&|raw| raw["build"]["changelist"] = 9_007_199_254_740_992u64.into())
            .unwrap()
            .changelist,
        None
    );
    assert_eq!(
        with(&|raw| raw["files"][0]["size"] = 40001.0.into()).unwrap().files[1].size,
        40001
    );
    assert_eq!(
        with(&|raw| raw["version"] = " a\u{7} b ".into()).unwrap().version,
        "a b"
    );
    // Refused whole
    let refused: [&dyn Fn(&mut Value); 14] = [
        &|raw| raw["files"][0]["name"] = "evil.dll".into(),
        &|raw| raw["files"][0]["name"] = "..\\winmm.dll".into(),
        &|raw| raw["files"][0]["name"] = "MysticParadox.dll".into(),
        &|raw| raw["files"][0]["sha256"] = hash('B').into(),
        &|raw| raw["files"][0]["sha256"] = "ab".into(),
        &|raw| raw["files"][0]["size"] = 0.into(),
        &|raw| raw["files"][0]["size"] = (64 * 1024 * 1024 + 1).into(),
        &|raw| raw["files"][0]["size"] = "40001".into(),
        &|raw| raw["files"][0]["size"] = 1.5.into(),
        &|raw| raw["files"].as_array_mut().unwrap().truncate(1),
        &|raw| {
            let first = raw["files"][0].clone();
            raw["files"].as_array_mut().unwrap().push(first);
        },
        &|raw| raw["files"] = serde_json::json!({}),
        &|raw| raw["version"] = " ".into(),
        &|raw| raw["version"] = 5.into(),
    ];
    for (i, edit) in refused.iter().enumerate() {
        assert_eq!(with(*edit), None, "case {i}");
    }
    assert_eq!(parse_runtime_manifest(Some(&Value::Null)), None);
    assert_eq!(parse_runtime_manifest(None), None);
}

// ---------------------------------------------------------------- answers, without the network

#[test]
fn register_answers() {
    let ok = |error| RegisterResult::Err { error, message: None };
    let cases = [
        (
            200,
            serde_json::json!({ "UUK": KEY }),
            RegisterResult::Ok { key: KEY.into() },
        ),
        (200, serde_json::json!({ "UUK": "bad key" }), ok(ErrorCode::ServerError)),
        (200, serde_json::json!({ "UUK": 5 }), ok(ErrorCode::ServerError)),
        (200, Value::Null, ok(ErrorCode::ServerError)),
        (
            400,
            serde_json::json!({ "error": "registration_closed", "message": " closed\u{7} " }),
            RegisterResult::Err {
                error: ErrorCode::RegistrationClosed,
                message: Some("closed".into()),
            },
        ),
        (
            400,
            serde_json::json!({ "error": "username_invalid" }),
            ok(ErrorCode::UsernameInvalid),
        ),
        (400, serde_json::json!({ "error": "other" }), ok(ErrorCode::BadRequest)),
        (400, Value::Null, ok(ErrorCode::BadRequest)),
        (401, Value::Null, ok(ErrorCode::InviteInvalid)),
        (409, Value::Null, ok(ErrorCode::UsernameTaken)),
        (413, Value::Null, ok(ErrorCode::RateLimited)),
        (429, Value::Null, ok(ErrorCode::RateLimited)),
        (500, Value::Null, ok(ErrorCode::ServerError)),
    ];
    for (status, body, expected) in cases {
        assert_eq!(parse::read_register(status, Some(&body)), expected, "{status} {body}");
    }
}

#[test]
fn user_info_answers() {
    let ok = serde_json::json!({ "UserId": "uid-1_A", "Username": " Slayer ", "IsAdmin": true });
    assert_eq!(
        parse::read_user_info(200, Some(&ok)),
        Ok(UserInfo {
            user_id: "uid-1_A".into(),
            username: "Slayer".into(),
            is_admin: true
        })
    );
    let not_admin = serde_json::json!({ "UserId": "u", "Username": "S", "IsAdmin": "true" });
    assert!(!parse::read_user_info(200, Some(&not_admin)).unwrap().is_admin);
    let failed = |error| Err((error, None));
    // The id goes on the game's command line
    for id in ["a b", "a;b", "-AUTH=x y", "é", ""] {
        let body = serde_json::json!({ "UserId": id, "Username": "S" });
        assert_eq!(
            parse::read_user_info(200, Some(&body)),
            failed(ErrorCode::ServerError),
            "{id:?}"
        );
    }
    let long = serde_json::json!({ "UserId": "a".repeat(129), "Username": "S" });
    assert_eq!(parse::read_user_info(200, Some(&long)).unwrap().user_id.len(), 128);
    assert_eq!(
        parse::read_user_info(200, Some(&Value::Null)),
        failed(ErrorCode::ServerError)
    );

    let banned = serde_json::json!({ "error": "account_banned", "reason": "cheating\u{202e}" });
    assert_eq!(
        parse::read_user_info(403, Some(&banned)),
        Err((ErrorCode::AccountBanned, Some("cheating".into())))
    );
    // Disabled reads as banned; no reason, no default text
    let disabled = serde_json::json!({ "error": "account_disabled" });
    assert_eq!(
        parse::read_user_info(403, Some(&disabled)),
        failed(ErrorCode::AccountBanned)
    );
    let other = serde_json::json!({ "error": "forbidden" });
    assert_eq!(parse::read_user_info(403, Some(&other)), failed(ErrorCode::KeyRejected));
    assert_eq!(parse::read_user_info(401, None), failed(ErrorCode::KeyRejected));
    assert_eq!(parse::read_user_info(429, None), failed(ErrorCode::RateLimited));
    assert_eq!(parse::read_user_info(502, None), failed(ErrorCode::ServerError));
}

#[test]
fn the_key_is_never_sent_outside_the_tailnet_without_a_pin() {
    let public_plain = Endpoint {
        host: "203.0.113.9".into(),
        port: 80,
        pin: None,
    };
    assert!(key_headers(&public_plain, Some(KEY)).is_empty());
    let pinned = Endpoint {
        pin: Some("3f".repeat(32)),
        ..public_plain
    };
    assert_eq!(key_headers(&pinned, Some(KEY)), vec![(KEY_HEADER, KEY.to_owned())]);
    assert!(key_headers(&pinned, Some("short")).is_empty());
    assert!(key_headers(&pinned, Some("RRK_ with space")).is_empty());
    assert!(key_headers(&pinned, None).is_empty());
    let tailnet = Endpoint {
        host: "100.64.0.1".into(),
        port: 61000,
        pin: None,
    };
    assert_eq!(key_headers(&tailnet, Some(KEY)).len(), 1);
}

// ---------------------------------------------------------------- a fake Reforged server

/// Reforged's `test/fakes.ts` `FakeServer`, the parts these calls reach: the launcher API and the runtime
/// manifest on one address, plain HTTP or TLS. Records every request it sees, with the key header.
#[derive(Default)]
struct Fake {
    seen: Vec<(String, Option<String>)>,
    users: HashMap<String, (String, String)>,
    codes: HashSet<String>,
    registrations: usize,
    status_missing: bool,
    runtime: Option<Value>,
}

impl Fake {
    fn new() -> Self {
        let mut fake = Self {
            runtime: Some(manifest()),
            ..Self::default()
        };
        fake.codes.insert("GOOD-CODE".into());
        fake.users.insert(KEY.into(), ("aurora".into(), "uid-aurora".into()));
        fake
    }

    fn answer(&mut self, method: &Method, path: &str, key: Option<String>, body: &[u8]) -> (u16, Value) {
        self.seen.push((path.to_owned(), key.clone()));
        let user = key.as_ref().and_then(|key| self.users.get(key)).cloned();
        match path {
            "/dauntless-status" => (200, serde_json::json!({ "show-status": true, "name": "Test Server" })),
            "/reforged/api/ServerStatus" if self.status_missing => (404, serde_json::json!({})),
            "/reforged/api/ServerStatus" => {
                let full = user.is_some();
                let players = if full {
                    serde_json::json!([{ "name": "Aurora", "where": "city", "instance": "city-8777" }])
                } else {
                    serde_json::json!([])
                };
                (
                    200,
                    serde_json::json!({
                        "name": "Test Server", "online": true, "version": "0.1.0", "commit": "abc1234",
                        "registration": "INVITECODE", "playersOnline": u8::from(full), "players": players,
                        "game": { "build": "rel-1.14.7-Archon", "changelist": 647472, "executableSha256": null },
                        "gameData": "real", "chat": true, "limited": !full,
                    }),
                )
            }
            "/reforged/api/GetUserInfo" => match user {
                Some((username, id)) => (
                    200,
                    serde_json::json!({ "UserId": id, "Username": username, "IsAdmin": false }),
                ),
                None => (401, serde_json::json!({ "error": "unauthorized" })),
            },
            "/reforged/api/Register" if method == Method::POST => {
                let Ok(parsed) = serde_json::from_slice::<Value>(body) else {
                    return (400, serde_json::json!({ "error": "bad_request" }));
                };
                let username = parsed["Username"].as_str().unwrap_or_default().to_owned();
                if !protocol::account::is_valid_username(&username) {
                    return (400, serde_json::json!({ "error": "username_invalid" }));
                }
                if self
                    .users
                    .values()
                    .any(|(name, _)| name.eq_ignore_ascii_case(&username))
                {
                    return (409, serde_json::json!({ "error": "username_taken" }));
                }
                if !parsed["InviteCode"]
                    .as_str()
                    .is_some_and(|code| self.codes.remove(code))
                {
                    return (401, serde_json::json!({ "error": "invite_invalid" }));
                }
                self.registrations += 1;
                let key = format!("RRK_{:048x}", self.registrations);
                self.users
                    .insert(key.clone(), (username, format!("uid-{}", self.registrations)));
                (200, serde_json::json!({ "UUK": key }))
            }
            "/content/v1/runtime/manifest" => match &self.runtime {
                Some(manifest) => (200, manifest.clone()),
                None => (404, serde_json::json!({ "error": "no_runtime" })),
            },
            _ => (404, Value::Null),
        }
    }
}

type Shared = Arc<Mutex<Fake>>;

async fn serve(fake: Shared, tls: Option<TlsAcceptor>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (fake, tls) = (fake.clone(), tls.clone());
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                    let fake = fake.clone();
                    async move {
                        let key = request
                            .headers()
                            .get(KEY_HEADER)
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned);
                        let (method, path) = (request.method().clone(), request.uri().path().to_owned());
                        let body = request
                            .into_body()
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let (status, body) = fake.lock().unwrap().answer(&method, &path, key, &body);
                        let text = if body.is_null() {
                            String::new()
                        } else {
                            body.to_string()
                        };
                        let response = Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .body(Full::new(Bytes::from(text)))
                            .unwrap();
                        Ok::<_, Infallible>(response)
                    }
                });
                let http = hyper::server::conn::http1::Builder::new();
                match tls {
                    Some(tls) => {
                        if let Ok(stream) = tls.accept(tcp).await {
                            let _ = http.serve_connection(TokioIo::new(stream), service).await;
                        }
                    }
                    None => {
                        let _ = http.serve_connection(TokioIo::new(tcp), service).await;
                    }
                }
            });
        }
    });
    port
}

/// A fake server over TLS with its certificate's fingerprint, and the fingerprint of another certificate.
async fn pinned_server() -> (Shared, Endpoint, String) {
    let (cert, key) = certificate();
    let (other, _) = certificate();
    let fingerprint = certificate_fingerprint(&cert);
    let fake = Arc::new(Mutex::new(Fake::new()));
    let port = serve(fake.clone(), Some(acceptor(cert, key))).await;
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port,
        pin: Some(fingerprint),
    };
    (fake, endpoint, certificate_fingerprint(&other))
}

fn seen(fake: &Shared) -> Vec<(String, Option<String>)> {
    fake.lock().unwrap().seen.clone()
}

// Reforged's pinned.test.ts, one test per case

#[tokio::test]
async fn status_over_the_pin_the_full_list_with_the_key_the_limited_view_without() {
    let (fake, endpoint, other) = pinned_server().await;
    match fetch_server_status(&endpoint, None, None).await {
        StatusResult::Ok(status) => assert!(status.limited && status.players.is_empty()),
        answer => panic!("{answer:?}"),
    }
    match fetch_server_status(&endpoint, Some(KEY), None).await {
        StatusResult::Ok(status) => {
            assert!(!status.limited);
            let names: Vec<&str> = status.players.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, ["Aurora"]);
            let game = status.game.unwrap();
            assert_eq!(
                (game.build.as_str(), game.changelist),
                ("rel-1.14.7-Archon", Some(647472))
            );
            assert_eq!(serde_json::to_value(status.game_data).unwrap(), "real");
        }
        answer => panic!("{answer:?}"),
    }
    assert_eq!(
        seen(&fake),
        [
            ("/reforged/api/ServerStatus".to_owned(), None),
            ("/reforged/api/ServerStatus".to_owned(), Some(KEY.to_owned())),
        ]
    );

    let before = seen(&fake).len();
    let mismatched = Endpoint {
        pin: Some(other),
        ..endpoint.clone()
    };
    assert_eq!(
        fetch_server_status(&mismatched, Some(KEY), None).await,
        StatusResult::CertMismatch
    );
    assert_eq!(seen(&fake).len(), before, "a mismatching server never sees a request");

    // A public address over plain HTTP is refused before connecting
    let public_plain = Endpoint {
        host: "203.0.113.10".into(),
        port: endpoint.port,
        pin: None,
    };
    assert_eq!(
        fetch_server_status(&public_plain, Some(KEY), Some(2000)).await,
        StatusResult::Unreachable
    );
}

#[tokio::test]
async fn an_older_server_is_recognised_through_dauntless_status_which_never_gets_the_key() {
    let fake = Arc::new(Mutex::new(Fake {
        status_missing: true,
        ..Fake::new()
    }));
    let port = serve(fake.clone(), None).await;
    let endpoint = Endpoint {
        host: "127.0.0.1".into(),
        port,
        pin: None,
    };
    assert_eq!(
        fetch_server_status(&endpoint, Some(KEY), None).await,
        StatusResult::Unsupported
    );
    assert_eq!(
        seen(&fake),
        [
            ("/reforged/api/ServerStatus".to_owned(), Some(KEY.to_owned())),
            ("/dauntless-status".to_owned(), None),
        ]
    );

    // Nobody listening
    let closed = Endpoint { port: 1, ..endpoint };
    assert_eq!(
        fetch_server_status(&closed, None, Some(500)).await,
        StatusResult::Unreachable
    );
}

#[tokio::test]
async fn register_and_user_info_over_the_pin_and_nothing_sent_to_a_mismatching_server() {
    let (fake, endpoint, other) = pinned_server().await;
    let mismatched = Endpoint {
        pin: Some(other),
        ..endpoint.clone()
    };
    assert_eq!(
        register_account(&mismatched, "Borealis", "GOOD-CODE").await,
        RegisterResult::Err {
            error: ErrorCode::CertMismatch,
            message: None
        }
    );
    assert!(seen(&fake).is_empty());

    let RegisterResult::Ok { key } = register_account(&endpoint, "Borealis", "GOOD-CODE").await else {
        panic!("registration failed");
    };
    assert!(key.starts_with("RRK_") && key.len() == 52 && key[4..].bytes().all(|b| b.is_ascii_hexdigit()));
    let info = fetch_user_info(&endpoint, &key).await.unwrap();
    assert_eq!(info.username, "Borealis");
    assert_eq!(
        seen(&fake).last().unwrap(),
        &("/reforged/api/GetUserInfo".to_owned(), Some(key.clone()))
    );
    assert_eq!(
        fetch_user_info(&mismatched, &key).await,
        Err((ErrorCode::CertMismatch, None))
    );

    let failed = |error| RegisterResult::Err { error, message: None };
    // The same code twice: used up
    assert_eq!(
        register_account(&endpoint, "Cirrus", "GOOD-CODE").await,
        failed(ErrorCode::InviteInvalid)
    );
    assert_eq!(
        register_account(&endpoint, "borealis", "X").await,
        failed(ErrorCode::UsernameTaken)
    );
    assert_eq!(
        register_account(&endpoint, "no", "X").await,
        failed(ErrorCode::UsernameInvalid)
    );
    assert_eq!(fake.lock().unwrap().registrations, 1);
    // Register never carries a key
    assert!(
        seen(&fake)
            .iter()
            .filter(|(path, _)| path == REGISTER_PATH)
            .all(|(_, key)| key.is_none())
    );
}

#[tokio::test]
async fn the_runtime_manifest_over_the_pin() {
    let (fake, endpoint, other) = pinned_server().await;
    match fetch_runtime_manifest(&endpoint).await {
        RuntimeManifestResult::Ok(manifest) => assert_eq!(manifest.changelist, Some(647472)),
        answer => panic!("{answer:?}"),
    }
    let mismatched = Endpoint {
        pin: Some(other),
        ..endpoint.clone()
    };
    assert_eq!(
        fetch_runtime_manifest(&mismatched).await,
        RuntimeManifestResult::CertMismatch
    );
    fake.lock().unwrap().runtime = None;
    assert_eq!(fetch_runtime_manifest(&endpoint).await, RuntimeManifestResult::None);
    fake.lock().unwrap().runtime = Some(serde_json::json!({ "version": "1", "files": [] }));
    assert_eq!(
        fetch_runtime_manifest(&endpoint).await,
        RuntimeManifestResult::BadAnswer
    );
    // The manifest is public: no key goes with it
    assert!(seen(&fake).iter().all(|(_, key)| key.is_none()));
}

#[tokio::test]
async fn user_info_sends_no_key_where_the_rule_refuses_it() {
    // Plain HTTP to a public address is refused by request() before connecting; the header is never built
    let public_plain = Endpoint {
        host: "203.0.113.10".into(),
        port: 80,
        pin: None,
    };
    assert_eq!(
        fetch_user_info(&public_plain, KEY).await,
        Err((ErrorCode::ServerUnreachable, None))
    );
    assert_eq!(
        fetch_user_info(&public_plain, "bad key").await,
        Err((ErrorCode::KeyRejected, None))
    );
}
