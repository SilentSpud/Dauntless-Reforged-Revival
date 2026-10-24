//! The admin API end to end: the bot's HTTP requests against a fake backend and Director that answer
//! as the Node services do (`routes/reforgedApi.ts`, `controllers/admin.ts`, the Director's
//! `/api/instances`) and check the manager-operator signature and the admin key as they do.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use admin::config::{Config, ServerMode};
use admin::schema::Admin;
use admin::upstream::{Operator, operator_signature};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const API_KEY: &str = "api-key-0123456789abcdef0123456789";
const ADMIN_KEY: &str = "admin-key-0123456789abcdef01234567";
const SECRET: &str = "operator-secret-0123456789abcdef01";
const START: i64 = 1_800_000_000_000;

// ------------------------------------------------------------------ the fake Node services

#[derive(Default)]
struct Backend {
    accounts: Mutex<Vec<Value>>,
    online: Mutex<Vec<String>>,
    kicked: Mutex<Vec<String>>,
    secret: String,
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, [("content-type", "application/json")], body.to_string()).into_response()
}

fn account(user_id: &str, name: &str, roles: &[&str]) -> Value {
    json!({
        "userId": user_id, "displayName": name, "email": "", "approvalStatus": "approved", "status": "active",
        "roles": roles, "usernameSet": true, "createdAt": "2026-10-01T10:00:00.000Z", "lastLoginAt": "2026-10-24T09:00:00.000Z"
    })
}

async fn backend(State(state): State<Arc<Backend>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = body.collect().await.map(|b| b.to_bytes()).unwrap_or_default();
    let target = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let path = parts.uri.path().to_owned();
    let query: Vec<(String, String)> = url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
        .into_owned()
        .collect();
    let param = |name: &str| query.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());

    if path == "/reforged/api/ServerStatus" {
        return reply(
            StatusCode::OK,
            json!({
                "name": "Friends' server", "online": true, "version": "0.1.0", "commit": "8f2c97f", "sourceUrl": null,
                "registration": "INVITECODE", "uptimeSeconds": 3600, "contentPort": null,
                "game": {"build": "rel-1.14.7-Archon", "changelist": 647472, "executableSha256": null},
                "gameData": "real", "chat": true, "playersOnline": 0, "players": [], "instances": [], "limited": true
            }),
        );
    }
    if path == "/reforged/api/Kick" {
        // HasAdminKey: the key, from this machine, not through the gateway
        if header("x-reforged-admin-key").as_deref() != Some(ADMIN_KEY) || header("x-reforged-gateway").is_some() {
            return reply(StatusCode::FORBIDDEN, json!({"error": "forbidden"}));
        }
        let given: Value = serde_json::from_slice(&body).unwrap_or_default();
        let name = given["Username"].as_str().unwrap_or("").to_lowercase();
        let accounts = state.accounts.lock().unwrap();
        let Some(found) = accounts
            .iter()
            .find(|a| a["displayName"].as_str().unwrap().to_lowercase() == name)
        else {
            return reply(StatusCode::NOT_FOUND, json!({"error": "not_found", "message": "x"}));
        };
        state
            .kicked
            .lock()
            .unwrap()
            .push(found["displayName"].as_str().unwrap().to_owned());
        return reply(
            StatusCode::OK,
            json!({"UserId": found["userId"], "Username": found["displayName"], "Closed": 2}),
        );
    }
    if !path.starts_with("/admin/v1/") {
        return reply(StatusCode::NOT_FOUND, json!({}));
    }

    // HasAdminAuth with the manager's credential
    let (Some(id), Some(ts), Some(sig)) = (
        header("x-paradox-operator-id"),
        header("x-paradox-operator-timestamp"),
        header("x-paradox-operator-signature"),
    ) else {
        return reply(
            StatusCode::UNAUTHORIZED,
            json!({"error": {"code": "ADMIN_UNAUTHORIZED"}}),
        );
    };
    let operator = Operator::parse(&id).unwrap();
    if operator_signature(&state.secret, parts.method.as_str(), &target, &ts, &operator, &body) != sig {
        return reply(
            StatusCode::UNAUTHORIZED,
            json!({"error": {"code": "ADMIN_OPERATOR_SIGNATURE"}}),
        );
    }
    if !body.is_empty() && header("content-type").as_deref() != Some("application/json") {
        return reply(
            StatusCode::BAD_REQUEST,
            json!({"error": {"code": "ADMIN_OPERATOR_BODY"}}),
        );
    }

    let mut accounts = state.accounts.lock().unwrap();
    if path == "/admin/v1/online-players" {
        let online = state.online.lock().unwrap();
        let players: Vec<Value> = accounts
            .iter()
            .filter(|a| online.iter().any(|id| a["userId"] == *id))
            .map(|a| json!({"userId": a["userId"], "displayName": a["displayName"]}))
            .collect();
        return reply(
            StatusCode::OK,
            json!({"count": players.len(), "players": players, "observedAt": "x", "source": "xmpp+presence"}),
        );
    }
    if path == "/admin/v1/players" {
        if let Some(ids) = param("ids") {
            let found: Vec<Value> = accounts
                .iter()
                .filter(|a| ids.split(',').any(|id| a["userId"] == id))
                .cloned()
                .collect();
            return reply(
                StatusCode::OK,
                json!({"accounts": found, "total": found.len(), "page": 1, "limit": 100}),
            );
        }
        let search = param("search").unwrap_or_default().to_lowercase();
        let page: usize = param("page").and_then(|p| p.parse().ok()).unwrap_or(1);
        let limit: usize = param("limit").and_then(|p| p.parse().ok()).unwrap_or(25);
        let matching: Vec<Value> = accounts
            .iter()
            .filter(|a| {
                a["displayName"].as_str().unwrap().to_lowercase().contains(&search)
                    || a["userId"].as_str().unwrap().contains(&search)
            })
            .cloned()
            .collect();
        let shown: Vec<Value> = matching.iter().skip((page - 1) * limit).take(limit).cloned().collect();
        return reply(
            StatusCode::OK,
            json!({"accounts": shown, "total": matching.len(), "page": page, "limit": limit}),
        );
    }
    if let Some(user_id) = path
        .strip_prefix("/admin/v1/players/")
        .and_then(|p| p.strip_suffix("/access"))
    {
        let given: Value = serde_json::from_slice(&body).unwrap_or_default();
        let Some(found) = accounts.iter_mut().find(|a| a["userId"] == user_id) else {
            return reply(StatusCode::NOT_FOUND, json!({"error": {"code": "NOT_FOUND"}}));
        };
        found["status"] = given["status"].clone();
        found["approvalReason"] = given["reason"].clone();
        found["approvalUpdatedBy"] = json!(format!("manager:{id}"));
        found["approvalUpdatedAt"] = json!("2026-10-24T12:00:00.000Z");
        return reply(StatusCode::OK, json!({"account": found.clone(), "requestId": "r"}));
    }
    reply(StatusCode::NOT_FOUND, json!({}))
}

async fn director(State(instances): State<Arc<Mutex<Value>>>) -> Response {
    reply(StatusCode::OK, instances.lock().unwrap().clone())
}

async fn listen(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(service_kit::serve::serve(
        listener,
        app,
        std::future::pending(),
        Duration::from_secs(1),
    ));
    address
}

struct World {
    url: String,
    backend: Arc<Backend>,
    instances: Arc<Mutex<Value>>,
    admin: Arc<Admin>,
    clock: Arc<AtomicI64>,
    timeouts_file: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

struct Options {
    director_up: bool,
    secret: &'static str,
}

async fn world_with(options: Options) -> World {
    let state = Arc::new(Backend {
        secret: SECRET.into(),
        ..Backend::default()
    });
    *state.accounts.lock().unwrap() = vec![
        account("u-ada", "Ada", &["player"]),
        account("u-bo", "Bo_Slayer", &["player"]),
        account("u-cy", "Cyra", &["player", "admin"]),
    ];
    *state.online.lock().unwrap() = vec!["u-ada".into(), "u-cy".into()];
    let backend_address = listen(Router::new().fallback(backend).with_state(state.clone())).await;
    let instances = Arc::new(Mutex::new(json!([
        {"id": "city-8777", "kind": "city", "map": "/Game/Maps/ramsgate/ramsgate_01_persistent", "behemoth": null,
         "players": 1, "maxPlayers": 0, "playerIds": ["u-cy"], "startedAt": "2026-10-24T08:00:00.000Z"},
        {"id": "hunt-8770", "kind": "hunt", "map": "/Game/Maps/islands/a/IslandA_persistent",
         "behemoth": "/Game/Behemoths/BP_Skarn.BP_Skarn_C", "players": 2, "maxPlayers": 4,
         "playerIds": ["u-ada", "u-gone"], "startedAt": "2026-10-24T09:30:00.000Z"}
    ])));
    let director_url = if options.director_up {
        let address = listen(
            Router::new()
                .route("/api/instances", axum::routing::get(director))
                .with_state(instances.clone()),
        )
        .await;
        format!("http://{address}")
    } else {
        // Nothing listens there
        let unused = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        format!("http://{unused}")
    };
    let dir = tempfile::tempdir().unwrap();
    let timeouts_file = dir.path().join("admin").join("timeouts.json");
    let config = Config {
        port: 0,
        bind_hosts: vec!["127.0.0.1".into()],
        backend_url: format!("http://{backend_address}"),
        director_url,
        api_key: API_KEY.into(),
        admin_key: ADMIN_KEY.into(),
        operator_secret: options.secret.into(),
        timeouts_file: timeouts_file.clone(),
        max_hunt_servers: 3,
        server_mode: ServerMode::Public,
    };
    let clock = Arc::new(AtomicI64::new(START));
    let reading = clock.clone();
    let (app, admin) = admin::server::prepare(&config, Arc::new(move || reading.load(Ordering::SeqCst))).unwrap();
    let address = listen(admin::app::router(app)).await;
    World {
        url: format!("http://{address}"),
        backend: state,
        instances,
        admin,
        clock,
        timeouts_file,
        _dir: dir,
    }
}

async fn world() -> World {
    world_with(Options {
        director_up: true,
        secret: SECRET,
    })
    .await
}

// ------------------------------------------------------------------ the bot's side

async fn send(url: &str, method: &str, headers: &[(&str, &str)], body: &str) -> (u16, Value) {
    let client: Client<HttpConnector, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let mut request = hyper::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = client
        .request(request.body(Full::new(Bytes::from(body.to_owned()))).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

impl World {
    /// A GraphQL request as the bot sends it, for Discord user 42.
    async fn gql(&self, query: &str, variables: Value) -> Value {
        let bearer = format!("Bearer {API_KEY}");
        let (status, body) = send(
            &format!("{}/graphql", self.url),
            "POST",
            &[
                ("authorization", &bearer),
                ("content-type", "application/json"),
                ("x-reforged-operator-id", "42"),
            ],
            &json!({"query": query, "variables": variables}).to_string(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        body
    }

    fn status_of(&self, user_id: &str) -> String {
        let accounts = self.backend.accounts.lock().unwrap();
        let found = accounts.iter().find(|a| a["userId"] == user_id).unwrap();
        found["status"].as_str().unwrap().to_owned()
    }
}

fn code(body: &Value) -> &str {
    body["errors"][0]["extensions"]["code"].as_str().unwrap_or("")
}

const PLAYER: &str = "query($name: String!) { player(username: $name) { userId username status online admin
  restriction { kind reason by until } location { kind instanceId title } } }";

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn refuses_requests_without_the_key_or_through_a_proxy() {
    let w = world().await;
    let url = format!("{}/graphql", w.url);
    let query = r#"{"query":"{ serverStatus { name } }"}"#;
    let bearer = format!("Bearer {API_KEY}");
    let json_type = ("content-type", "application/json");
    for (headers, status, code_text) in [
        (vec![json_type], 401, "UNAUTHENTICATED"),
        (
            vec![json_type, ("authorization", "Bearer wrong")],
            401,
            "UNAUTHENTICATED",
        ),
        (
            vec![
                json_type,
                ("authorization", bearer.as_str()),
                ("x-forwarded-for", "203.0.113.9"),
            ],
            403,
            "FORWARDED",
        ),
        (
            vec![
                json_type,
                ("authorization", bearer.as_str()),
                ("x-reforged-gateway", "s"),
            ],
            403,
            "FORWARDED",
        ),
        (
            vec![("authorization", bearer.as_str()), ("content-type", "text/plain")],
            415,
            "NOT_JSON",
        ),
        (
            vec![
                json_type,
                ("authorization", bearer.as_str()),
                ("x-reforged-operator-id", "me"),
            ],
            400,
            "BAD_OPERATOR",
        ),
    ] {
        let (got, body) = send(&url, "POST", &headers, query).await;
        assert_eq!((got, code(&body)), (status, code_text), "{headers:?}");
    }
    let (got, body) = send(&url, "POST", &[json_type, ("authorization", bearer.as_str())], "[1,2]").await;
    assert_eq!((got, code(&body)), (400, "BAD_REQUEST"));
    // No query by GET, and nothing else is served
    assert_eq!(
        send(&url, "GET", &[("authorization", bearer.as_str())], "").await.0,
        405
    );
    assert_eq!(send(&format!("{}/", w.url), "GET", &[], "").await.0, 404);
    let (got, body) = send(&format!("{}/health", w.url), "GET", &[], "").await;
    assert_eq!((got, body), (200, json!({"ok": true})));
}

#[tokio::test]
async fn looks_up_a_player_with_where_they_are() {
    let w = world().await;
    let body = w.gql(PLAYER, json!({"name": "ada"})).await;
    assert_eq!(
        body["data"]["player"],
        json!({"userId": "u-ada", "username": "Ada", "status": "ACTIVE", "online": true, "admin": false,
               "restriction": null, "location": {"kind": "HUNT", "instanceId": "hunt-8770", "title": "Skarn"}})
    );
    let body = w.gql(PLAYER, json!({"name": "Bo_Slayer"})).await;
    assert_eq!(body["data"]["player"]["online"], false);
    assert_eq!(body["data"]["player"]["location"], Value::Null);
    let body = w.gql(PLAYER, json!({"name": "Cyra"})).await;
    assert_eq!(body["data"]["player"]["location"]["kind"], "HUB");
    assert_eq!(body["data"]["player"]["admin"], true);
    // A part of a name is not a match
    assert_eq!(
        w.gql(PLAYER, json!({"name": "Bo_"})).await["data"]["player"],
        Value::Null
    );
    assert_eq!(code(&w.gql(PLAYER, json!({"name": "no spaces"})).await), "BAD_INPUT");
}

#[tokio::test]
async fn reports_the_server_and_its_hunts() {
    let w = world().await;
    let body = w
        .gql(
            "{ serverStatus { name mode registration version commit uptimeSeconds onlineCount chat gameData } }",
            json!({}),
        )
        .await;
    assert_eq!(
        body["data"]["serverStatus"],
        json!({"name": "Friends' server", "mode": "PUBLIC", "registration": "INVITECODE", "version": "0.1.0",
               "commit": "8f2c97f", "uptimeSeconds": 3600, "onlineCount": 2, "chat": true, "gameData": "real"})
    );
    let body = w
        .gql(
            "{ huntStatus { maxHuntServers running free hunts { id title map behemoth playerCount maxPlayers startedAt players { userId username } } } }",
            json!({}),
        )
        .await;
    assert_eq!(
        body["data"]["huntStatus"],
        json!({"maxHuntServers": 3, "running": 1, "free": 2, "hunts": [{
            "id": "hunt-8770", "title": "Skarn", "map": "IslandA_persistent", "behemoth": "BP_Skarn",
            "playerCount": 2, "maxPlayers": 4, "startedAt": "2026-10-24T09:30:00Z",
            "players": [{"userId": "u-ada", "username": "Ada"}, {"userId": "u-gone", "username": null}]
        }]})
    );
    *w.instances.lock().unwrap() = json!([]);
    let body = w.gql("{ huntStatus { running free hunts { id } } }", json!({})).await;
    assert_eq!(
        body["data"]["huntStatus"],
        json!({"running": 0, "free": 3, "hunts": []})
    );
}

#[tokio::test]
async fn kicks_through_the_backend_but_never_an_admin() {
    let w = world().await;
    let kick = "mutation($name: String!) { kick(username: $name) { userId username connectionsClosed } }";
    let body = w.gql(kick, json!({"name": "ADA"})).await;
    assert_eq!(
        body["data"]["kick"],
        json!({"userId": "u-ada", "username": "Ada", "connectionsClosed": 2})
    );
    assert_eq!(code(&w.gql(kick, json!({"name": "Cyra"})).await), "PROTECTED_ACCOUNT");
    assert_eq!(code(&w.gql(kick, json!({"name": "Nobody"})).await), "NOT_FOUND");
    assert_eq!(*w.backend.kicked.lock().unwrap(), ["Ada"]);
    assert_eq!(w.status_of("u-ada"), "active");
}

#[tokio::test]
async fn bans_and_unbans_in_the_operators_name() {
    let w = world().await;
    let ban = "mutation($name: String!, $reason: String!) { ban(username: $name, reason: $reason) { status restriction { kind reason by until } } }";
    let body = w
        .gql(ban, json!({"name": "Bo_Slayer", "reason": "griefing\nhunts"}))
        .await;
    assert_eq!(
        body["data"]["ban"],
        json!({"status": "BANNED", "restriction": {"kind": "BAN", "reason": "griefing hunts", "by": "manager:42", "until": null}})
    );
    assert_eq!(w.status_of("u-bo"), "banned");
    assert_eq!(
        code(&w.gql(ban, json!({"name": "Bo_Slayer", "reason": "x"})).await),
        "BAD_INPUT"
    );
    assert_eq!(
        code(&w.gql(ban, json!({"name": "Cyra", "reason": "testing"})).await),
        "PROTECTED_ACCOUNT"
    );

    let unban = "mutation($name: String!) { unban(username: $name) { status restriction { kind } } }";
    let body = w.gql(unban, json!({"name": "bo_slayer"})).await;
    assert_eq!(body["data"]["unban"], json!({"status": "ACTIVE", "restriction": null}));
    assert_eq!(code(&w.gql(unban, json!({"name": "bo_slayer"})).await), "NOT_BANNED");
}

const TIMEOUT: &str = "mutation($name: String!, $minutes: Int, $until: String, $reason: String!) {
  timeout(username: $name, minutes: $minutes, until: $until, reason: $reason) { status restriction { kind reason until } } }";

#[tokio::test]
async fn a_timeout_is_lifted_when_it_runs_out_even_after_a_restart() {
    let w = world().await;
    let body = w
        .gql(
            TIMEOUT,
            json!({"name": "Ada", "minutes": 30, "reason": "spamming chat"}),
        )
        .await;
    assert_eq!(
        body["data"]["timeout"],
        json!({"status": "BANNED", "restriction": {"kind": "TIMEOUT",
               "reason": "Timed out until 2027-01-15T08:30:00Z: spamming chat", "until": "2027-01-15T08:30:00Z"}})
    );
    assert!(std::fs::read_to_string(&w.timeouts_file).unwrap().contains("u-ada"));

    // A restart reads the file back
    let again = admin::timeouts::Timeouts::load(&w.timeouts_file).unwrap();
    assert_eq!(again.due(START + 30 * 60_000).await.len(), 1);

    w.admin.sweep().await;
    assert_eq!(w.status_of("u-ada"), "banned", "not due yet");
    w.clock.store(START + 30 * 60_000, Ordering::SeqCst);
    w.admin.sweep().await;
    assert_eq!(w.status_of("u-ada"), "active");
    assert!(!std::fs::read_to_string(&w.timeouts_file).unwrap().contains("u-ada"));
    let body = w.gql(PLAYER, json!({"name": "Ada"})).await;
    assert_eq!(body["data"]["player"]["restriction"], Value::Null);
}

#[tokio::test]
async fn a_timeout_never_lifts_a_ban_made_since_or_one_it_did_not_make() {
    let w = world().await;
    // Already banned for good: a timeout would shorten it
    let ban = "mutation { ban(username: \"Bo_Slayer\", reason: \"cheating\") { status } }";
    w.gql(ban, json!({})).await;
    assert_eq!(
        code(
            &w.gql(TIMEOUT, json!({"name": "Bo_Slayer", "minutes": 5, "reason": "spam"}))
                .await
        ),
        "ALREADY_BANNED"
    );

    // Timed out, then banned for good from the backend's own dashboard
    w.gql(
        TIMEOUT,
        json!({"name": "Ada", "until": "2027-01-15T09:00:00Z", "reason": "spam"}),
    )
    .await;
    for a in w.backend.accounts.lock().unwrap().iter_mut() {
        if a["userId"] == "u-ada" {
            a["approvalReason"] = json!("cheating, for good");
        }
    }
    assert_eq!(
        w.gql(PLAYER, json!({"name": "Ada"})).await["data"]["player"]["restriction"]["kind"],
        "BAN"
    );
    w.clock.store(START + 2 * 60 * 60_000, Ordering::SeqCst);
    w.admin.sweep().await;
    assert_eq!(w.status_of("u-ada"), "banned");
    assert!(w.admin.timeouts.due(i64::MAX).await.is_empty());

    // A bad end, and lifting what is not a timeout
    for (minutes, until) in [
        (json!(0), Value::Null),
        (Value::Null, json!("tomorrow")),
        (json!(5), json!("2027-01-16T00:00:00Z")),
    ] {
        let body = w
            .gql(
                TIMEOUT,
                json!({"name": "Cyra", "minutes": minutes, "until": until, "reason": "spam"}),
            )
            .await;
        assert_eq!(code(&body), "BAD_INPUT", "{minutes} {until}");
    }
    let lift = "mutation { liftTimeout(username: \"Bo_Slayer\") { status } }";
    assert_eq!(code(&w.gql(lift, json!({})).await), "NOT_TIMED_OUT");
}

#[tokio::test]
async fn lifting_a_timeout_early_or_banning_over_it() {
    let w = world().await;
    w.gql(TIMEOUT, json!({"name": "Ada", "minutes": 60, "reason": "spam"}))
        .await;
    let body = w
        .gql(
            "mutation { liftTimeout(username: \"Ada\") { status restriction { kind } } }",
            json!({}),
        )
        .await;
    assert_eq!(
        body["data"]["liftTimeout"],
        json!({"status": "ACTIVE", "restriction": null})
    );
    assert!(w.admin.timeouts.get("u-ada").await.is_none());

    w.gql(TIMEOUT, json!({"name": "Ada", "minutes": 60, "reason": "spam"}))
        .await;
    let body = w
        .gql(
            "mutation { ban(username: \"Ada\", reason: \"for good\") { restriction { kind until } } }",
            json!({}),
        )
        .await;
    assert_eq!(
        body["data"]["ban"]["restriction"],
        json!({"kind": "BAN", "until": null})
    );
    assert!(w.admin.timeouts.get("u-ada").await.is_none());
}

#[tokio::test]
async fn says_what_is_wrong_when_a_service_is_down_or_the_secret_differs() {
    let w = world_with(Options {
        director_up: false,
        secret: SECRET,
    })
    .await;
    // The lookup still answers, without a location
    let body = w.gql(PLAYER, json!({"name": "Ada"})).await;
    assert_eq!(body["data"]["player"]["online"], true);
    assert_eq!(body["data"]["player"]["location"], Value::Null);
    assert_eq!(
        code(&w.gql("{ huntStatus { running } }", json!({})).await),
        "BACKEND_UNAVAILABLE"
    );

    let w = world_with(Options {
        director_up: true,
        secret: "another-secret-0123456789abcdef0123",
    })
    .await;
    let body = w.gql(PLAYER, json!({"name": "Ada"})).await;
    assert_eq!(code(&body), "BACKEND_REFUSED");
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("reforged-host setup")
    );
}

#[tokio::test]
async fn refuses_queries_too_deep_or_too_big() {
    let w = world().await;
    let body = w
        .gql(
            "{ __schema { types { fields { type { ofType { ofType { name } } } } } } }",
            json!({}),
        )
        .await;
    assert!(!body["errors"].as_array().unwrap().is_empty());
}

#[test]
fn schema_graphql_is_current() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema.graphql");
    let current = admin::schema::sdl();
    if std::env::var("RR_UPDATE_GENERATED").as_deref() == Ok("1") {
        std::fs::write(&file, &current).unwrap();
    }
    let written = std::fs::read_to_string(&file).unwrap_or_default().replace("\r\n", "\n");
    assert_eq!(
        written, current,
        "schema.graphql is stale: RR_UPDATE_GENERATED=1 cargo test -p admin"
    );
}
