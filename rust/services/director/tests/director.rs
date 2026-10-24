#![allow(clippy::unwrap_used, clippy::expect_used)] // tests

//! The Director over HTTP with fake game servers: this test binary runs itself as the game server
//! (harness = false), prints the runtime's readiness marker and waits until told to exit. What a
//! fake does depends on its map: `/Game/Maps/silent` never reports ready, `/Game/Maps/crash` exits
//! at once, anything else is ready and runs until `stop-<port>` appears in the control folder.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use director::config::Config;
use director::gameservers::Director;
use serde_json::{Value, json};
use service_kit::env::Env;

const FAKE: &str = "DIRECTOR_TEST_FAKE_GAMESERVER";
const CONTROL: &str = "DIRECTOR_TEST_CONTROL";
const BEGIN: u16 = 47000;
const END: u16 = 47004;

type Test = fn() -> futures_util::future::BoxFuture<'static, ()>;

fn main() -> ExitCode {
    if std::env::var_os(FAKE).is_some() {
        return fake_game_server();
    }
    director::log::set_level(Some("error"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let tests: Vec<(&str, Test)> = vec![
        ("serves_the_hubs_and_hunts_as_the_node_director", || {
            Box::pin(serves_the_hubs_and_hunts())
        }),
        ("answers_express_s_404_and_takes_a_safe_request_id", || {
            Box::pin(answers_404_and_request_ids())
        }),
        ("a_hunt_that_fails_before_ready_is_a_failed_matchmaking", || {
            Box::pin(failed_hunts())
        }),
        ("a_crashed_ramsgate_is_refused_then_restarted", || {
            Box::pin(ramsgate_restart())
        }),
    ];
    let filter: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    let mut failed = 0;
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(test())));
        match result {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed += 1;
            }
        }
    }
    runtime.shutdown_timeout(Duration::from_secs(1));
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ---- the fake game server

fn fake_game_server() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let control = PathBuf::from(std::env::var(CONTROL).unwrap());
    let port = args[1].clone();
    let launch = std::env::var("MYSTICPARADOX_GAMESERVER_LAUNCH_ID").unwrap_or_default();
    let record = json!({
        "args": args,
        "launchId": launch,
        "trialsWeek": std::env::var("MYSTICPARADOX_TRIALS_WEEK").ok(),
        "inherited": std::env::var("REFORGED_SERVER_MAX_FPS").ok(),
    });
    std::fs::write(control.join(format!("args-{port}.json")), record.to_string()).unwrap();
    match args[2].as_str() {
        "/Game/Maps/crash" => return ExitCode::from(3),
        "/Game/Maps/silent" => {}
        _ => {
            println!("LogNet: listening");
            println!("MYSTICPARADOX_GAMESERVER_READY launchId=00000000-0000-4000-8000-000000000000 port={port}");
            println!("MYSTICPARADOX_GAMESERVER_READY launchId={launch} port={port}");
        }
    }
    let stop = control.join(format!("stop-{port}"));
    loop {
        if stop.exists() {
            let _ = std::fs::remove_file(&stop);
            return ExitCode::from(0);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---- the Director under test

struct Running {
    url: String,
    director: Arc<Director>,
    control: PathBuf,
    _dir: tempfile::TempDir,
}

fn write_tables(dir: &Path) {
    let table = |rows: Value| json!([{ "Type": "DataTable", "Rows": rows }]).to_string();
    std::fs::write(
        dir.join("player_hunts_table.json"),
        table(json!({
            "Plain_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "MM_Plain" }] },
            "Boss_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "MM_Boss" }] }
        })),
    )
    .unwrap();
    std::fs::write(
        dir.join("matchmaker_hunts_table.json"),
        table(json!({
            "MM_Plain": { "MapList": [{ "MapAssetName": "/Game/Maps/test/plain.plain" }], "SpecificBehemoth": { "BehemothAsset": { "AssetPathName": "None" } } },
            "MM_Boss": { "MapList": [{ "MapAssetName": "/Game/Maps/test/boss.boss" }], "SpecificBehemoth": { "BehemothAsset": { "AssetPathName": "/Game/Monsters/test/boss_bp.boss_bp_C" } } }
        })),
    )
    .unwrap();
    for (_, file) in &director::hunt::MATCHMAKER_TABLES[1..] {
        std::fs::write(dir.join(file), table(json!({}))).unwrap();
    }
}

async fn start(extra: &[(&str, &str)]) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let control = dir.path().join("control");
    let data = dir.path().join("game-data");
    let logs = dir.path().join("logs");
    for d in [&control, &data, &logs] {
        std::fs::create_dir_all(d).unwrap();
    }
    write_tables(&data);
    let exe = std::env::current_exe().unwrap().display().to_string();
    let (control_s, data_s, logs_s) = (
        control.display().to_string(),
        data.display().to_string(),
        logs.display().to_string(),
    );
    let (begin, end) = (BEGIN.to_string(), END.to_string());
    let system_root = std::env::var("SystemRoot").unwrap_or_default();
    let path = std::env::var("PATH").unwrap_or_default();
    let mut pairs: Vec<(&str, &str)> = vec![
        ("PORT", "1"),
        ("MY_IP", "100.101.102.103"),
        ("PORT_RANGE_BEGIN", &begin),
        ("PORT_RANGE_END", &end),
        ("GAMESERVER_BINARY_PATH", &exe),
        ("METAGAME_API_KEY", "gameserver-key"),
        ("SECONDS_TO_WAIT_BETWEEN_GAMESERVER_STARTUP", "0"),
        ("GAMESERVER_READY_TIMEOUT_MS", "3000"),
        ("MAX_CONCURRENT_HUNT_SERVERS", "2"),
        ("GAMESERVER_LOG_DIR", &logs_s),
        ("PARADOX_GAME_DATA_DIR", &data_s),
        ("REFORGED_SERVER_MAX_FPS", "30"),
        (FAKE, "1"),
        (CONTROL, &control_s),
        ("SystemRoot", &system_root),
        ("PATH", &path),
    ];
    for (k, v) in extra {
        pairs.retain(|(name, _)| name != k);
        pairs.push((k, v));
    }
    let config = Config::from_env(&Env::from_pairs(pairs)).unwrap();
    let tables = director::hunt::load_tables(&config.game_data_dir).unwrap();
    let (app, director) = director::server::prepare(config, tables);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    Running {
        url,
        director,
        control,
        _dir: dir,
    }
}

struct Reply {
    status: u16,
    headers: reqwest_like::Headers,
    text: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

/// A minimal HTTP/1.1 client over a TCP socket (no client crate in the tree for tests).
mod reqwest_like {
    pub type Headers = Vec<(String, String)>;

    pub async fn send(url: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> super::Reply {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let address = url.trim_start_matches("http://");
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
        for (k, v) in headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let mut lines = head.lines();
        let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
        let headers: Headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked");
        let body = if chunked { dechunk(body) } else { body.to_owned() };
        super::Reply {
            status,
            headers,
            text: body,
        }
    }

    fn dechunk(mut body: &str) -> String {
        let mut out = String::new();
        while let Some((size, rest)) = body.split_once("\r\n") {
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            out.push_str(&rest[..n]);
            body = &rest[n + 2..];
        }
        out
    }
}

impl Running {
    async fn get(&self, path: &str) -> Reply {
        reqwest_like::send(&self.url, "GET", path, &[], "").await
    }

    async fn post(&self, path: &str, body: Value) -> Reply {
        reqwest_like::send(
            &self.url,
            "POST",
            path,
            &[("Content-Type", "application/json")],
            &body.to_string(),
        )
        .await
    }

    async fn matchmake(&self, body: Value) -> Reply {
        self.post("/api/matchmaker/handle-matchmaking-for-player", body).await
    }

    fn args(&self, port: u16) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.control.join(format!("args-{port}.json"))).unwrap()).unwrap()
    }

    fn stop(&self, port: u16) {
        std::fs::write(self.control.join(format!("stop-{port}")), "").unwrap();
    }

    async fn until(&self, what: &str, check: impl Fn(&Director) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !check(&self.director) {
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn shutdown(&self) {
        self.director.stop_all();
    }
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

// ---- the tests

async fn serves_the_hubs_and_hunts() {
    let d = start(&[]).await;
    // Before the boot: not ready, nothing to list, no hub.
    let health = d.get("/health/ready").await;
    assert_eq!(
        (health.status, health.text.as_str()),
        (503, r#"{"ok":false,"service":"deployserver"}"#)
    );
    assert_eq!(header(&health, "content-type"), Some("application/json; charset=utf-8"));
    assert_eq!(
        header(&health, "x-mysticparadox-request-id"),
        None,
        "registered before the request id"
    );
    assert_eq!(
        d.get("/api/hub-status").await.text,
        r#"{"ramsgate":null,"trainingDojo":null}"#
    );
    assert_eq!(d.get("/api/instances").await.text, "[]");
    let city = d.matchmake(json!({"GameMode": "CITY"})).await;
    assert_eq!(
        (city.status, city.text.as_str()),
        (500, r#"{"error":"matchmaking_failed"}"#)
    );

    d.director.startup().await;
    d.director.set_persistent_hubs_ready(true);
    assert_eq!(
        d.get("/HEALTH/Ready/").await.text,
        r#"{"ok":true,"service":"deployserver"}"#,
        "Express matching"
    );
    let status = d.get("/api/hub-status").await.json();
    assert_eq!(status["ramsgate"]["port"], END);
    assert_eq!(status["ramsgate"]["host"], "100.101.102.103");
    assert_eq!(status["trainingDojo"]["port"], END - 1);
    assert_eq!(status["ramsgate"]["ready"], true);

    // The hubs' command lines: the map and the NO_ placeholders, then the standard switches.
    let ramsgate = d.args(END);
    let args: Vec<&str> = ramsgate["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        args[..7],
        [
            "gameserver-key",
            "47004",
            "/Game/Maps/ramsgate/ramsgate_01_persistent",
            "NO_BEHEMOTH",
            "NO_MM_HUNTID",
            "NO_EXPECTED_PLAYERS",
            "100.101.102.103:47004"
        ]
    );
    assert_eq!(
        args[7..13],
        [
            "-EpicPortal",
            "-server",
            "-nullrhi",
            "-warp",
            "-nosound",
            "-RepDriverEnable"
        ]
    );
    assert_eq!(
        args[13],
        "-LogCmds=\"LogPhoenixCharacter VeryVerbose, LogOnline Verbose, LogLoad Off\""
    );
    assert_eq!(ramsgate["inherited"], "30", "the game servers inherit the env file");

    let city = d.matchmake(json!({"GameMode": "CITY"})).await;
    assert_eq!(
        (city.status, city.text.as_str()),
        (200, r#"{"host":"100.101.102.103","port":47004}"#)
    );
    assert!(header(&city, "x-mysticparadox-request-id").is_some_and(|id| id.len() == 36));
    let dojo = d
        .matchmake(json!({"GameMode": "SHARED", "HuntId": "ShatteredIsles_TrainingDojo"}))
        .await;
    assert_eq!(dojo.json()["port"], END - 1);

    // ISLAND from travel arguments: the oldest free hunt port.
    let island = d
        .matchmake(json!({
            "GameMode": "ISLAND",
            "GameArgs": "/Game/Maps/test/x?MonsterClass=/Game/Monsters/a/fulgar_bp.fulgar_bp_C?HuntID=Arena_MatchmakerHunt_Hard_042",
            "HuntId": "Plain_Hunt",
            "ExpectedPlayers": ["acc-1"]
        }))
        .await;
    assert_eq!(island.text, r#"{"host":"100.101.102.103","port":47000}"#);
    let hunt = d.args(BEGIN);
    let args = hunt["args"].as_array().unwrap();
    assert_eq!(args[3], "/Game/Monsters/a/fulgar_bp.fulgar_bp_C");
    assert_eq!(args[4], "Arena_MatchmakerHunt_Hard_042");
    assert_eq!(args[5], "acc-1:Plain_Hunt");
    assert_eq!(hunt["trialsWeek"], "042", "an arena hunt keeps its week");
    assert!(
        d.control
            .parent()
            .unwrap()
            .join("logs")
            .join("port47000_fulgar_bp.log")
            .is_file()
    );

    // SHARED through the manifest.
    let shared = d
        .matchmake(json!({"GameMode": "SHARED", "HuntId": "Boss_Hunt", "ExpectedPlayers": ["acc-2", "acc-3"]}))
        .await;
    assert_eq!(shared.json()["port"], BEGIN + 1);
    let args = d.args(BEGIN + 1)["args"].clone();
    assert_eq!(args[2], "/Game/Maps/test/boss");
    assert_eq!(args[3], "/Game/Monsters/test/boss_bp.boss_bp_C");
    assert_eq!(args[4], "MM_Boss");
    assert_eq!(args[5], "acc-2:Boss_Hunt,acc-3:Boss_Hunt");

    // The cap (2): a third hunt fails, nothing is started.
    let full = d
        .matchmake(json!({"GameMode": "SHARED", "HuntId": "Plain_Hunt", "ExpectedPlayers": ["acc-4"]}))
        .await;
    assert_eq!(full.status, 500);
    assert!(!d.control.join(format!("args-{}.json", BEGIN + 2)).exists());

    let instances = d.get("/api/instances").await.json();
    let kinds: Vec<&str> = instances
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["city-47004", "dojo-47003", "hunt-47000", "hunt-47001"]);
    let boss = &instances[3];
    assert_eq!(boss["players"], 2);
    assert_eq!(boss["maxPlayers"], 4);
    assert_eq!(boss["playerIds"], json!(["acc-2", "acc-3"]));
    assert_eq!(boss["behemoth"], "/Game/Monsters/test/boss_bp.boss_bp_C");
    assert_eq!(instances[0]["maxPlayers"], 0);

    // A hunt that ends gives its port back at once, and the next hunt can start.
    d.stop(BEGIN);
    d.until("the hunt's exit", |dir| {
        dir.gameservers().iter().all(|s| s.port != BEGIN)
    })
    .await;
    let next = d
        .matchmake(json!({"GameMode": "SHARED", "HuntId": "Plain_Hunt", "ExpectedPlayers": ["acc-5"]}))
        .await;
    assert_eq!(next.json()["port"], BEGIN + 2, "FIFO: the freed port is reused last");
    let log =
        std::fs::read_to_string(d.control.parent().unwrap().join("logs").join("port47000_fulgar_bp.log")).unwrap();
    assert!(log.starts_with("=== Gameserver launch "));
    assert!(
        log.contains("MYSTICPARADOX_GAMESERVER_READY"),
        "the output is copied to the log"
    );
    assert!(
        log.contains("=== Gameserver process exited port=47000 label=fulgar_bp pid="),
        "{log}"
    );

    // The player-hosted resolver.
    let manifest = d
        .post(
            "/api/matchmaker/resolve-hunt-launch-manifest",
            json!({"HuntId": "Boss_Hunt", "ExpectedPlayers": ["p"]}),
        )
        .await;
    assert_eq!(
        manifest.text,
        r#"{"huntId":"Boss_Hunt","matchmakerHuntId":"MM_Boss","mapPath":"/Game/Maps/test/boss","behemothPath":"/Game/Monsters/test/boss_bp.boss_bp_C","expectedPlayerString":"p:Boss_Hunt","guardCompletionMode":"FIRST_DEFEAT"}"#
    );
    let bad = d
        .post(
            "/api/matchmaker/resolve-hunt-launch-manifest",
            json!({"HuntId": "Nope"}),
        )
        .await;
    assert_eq!(
        (bad.status, bad.text.as_str()),
        (400, r#"{"error":"hunt_manifest_invalid"}"#)
    );
    let missing = d.director.missing_hunt_requests();
    assert!(
        missing.is_empty(),
        "a bad party is refused before the row is looked up: {missing:?}"
    );
    let unknown = d
        .post(
            "/api/matchmaker/resolve-hunt-launch-manifest",
            json!({"HuntId": "Nope", "ExpectedPlayers": ["p"]}),
        )
        .await;
    assert_eq!(unknown.status, 400);
    assert_eq!(d.director.missing_hunt_requests()[0]["huntId"], "Nope");

    let unsupported = d.matchmake(json!({"GameMode": "RAID"})).await;
    assert_eq!(unsupported.status, 500);
    let not_json = reqwest_like::send(
        &d.url,
        "POST",
        "/api/matchmaker/handle-matchmaking-for-player",
        &[],
        "GameMode=CITY",
    )
    .await;
    assert_eq!(not_json.status, 500, "no JSON content type: req.body is undefined");
    d.shutdown();
}

async fn answers_404_and_request_ids() {
    let d = start(&[]).await;
    let missing = d.get("/nope?x=1").await;
    assert_eq!(missing.status, 404);
    assert!(missing.text.contains("<pre>Not Found</pre>"));
    assert_eq!(header(&missing, "content-type"), Some("text/html; charset=utf-8"));
    assert!(header(&missing, "x-mysticparadox-request-id").is_some());
    let chosen = reqwest_like::send(&d.url, "GET", "/nope", &[("x-request-id", "client-id-0001")], "").await;
    assert_eq!(header(&chosen, "x-mysticparadox-request-id"), Some("client-id-0001"));
    let unsafe_id = reqwest_like::send(&d.url, "GET", "/nope", &[("x-mysticparadox-request-id", "short")], "").await;
    assert_ne!(header(&unsafe_id, "x-mysticparadox-request-id"), Some("short"));
    let wrong_method = d.post("/api/instances", json!({})).await;
    assert_eq!(wrong_method.status, 404);
    let head = reqwest_like::send(&d.url, "HEAD", "/api/instances", &[], "").await;
    assert_eq!((head.status, head.text.as_str()), (200, ""));
    let broken = reqwest_like::send(
        &d.url,
        "POST",
        "/api/matchmaker/handle-matchmaking-for-player",
        &[("Content-Type", "application/json")],
        "{oops",
    )
    .await;
    assert_eq!(broken.status, 400);
    let p2p_off = start(&[("MYSTICPARADOX_P2P_MODULE", "off")]).await;
    let refused = p2p_off
        .post(
            "/api/matchmaker/resolve-hunt-launch-manifest",
            json!({"HuntId": "Boss_Hunt", "ExpectedPlayers": ["p"]}),
        )
        .await;
    assert_eq!(refused.status, 404, "MYSTICPARADOX_P2P_MODULE=off mounts no resolver");
    d.shutdown();
    p2p_off.shutdown();
}

async fn failed_hunts() {
    let d = start(&[("RAMSGATE_HUB", "off"), ("TRAINING_HUB", "off")]).await;
    d.director.startup().await;
    let crash = d
        .matchmake(json!({"GameMode": "ISLAND", "GameArgs": "/Game/Maps/crash"}))
        .await;
    assert_eq!(crash.status, 500);
    let silent = d
        .matchmake(json!({"GameMode": "ISLAND", "GameArgs": "/Game/Maps/silent"}))
        .await;
    assert_eq!(silent.status, 500, "no readiness within GAMESERVER_READY_TIMEOUT_MS");
    // Both ports come back: the crash's at once, the silent one's once its stopped process exits.
    d.until("the ports back", |dir| dir.gameservers().is_empty()).await;
    let ok = d
        .matchmake(json!({"GameMode": "ISLAND", "GameArgs": "/Game/Maps/test/ok"}))
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text);
    let off = d.matchmake(json!({"GameMode": "CITY"})).await;
    assert_eq!(off.status, 500, "RAMSGATE_HUB=off");
    // TRAINING_HUB=off: the first request starts the Training Grounds and waits for them.
    let dojo = d
        .matchmake(json!({"GameMode": "SHARED", "HuntId": "ShatteredIsles_TrainingDojo"}))
        .await;
    assert_eq!(dojo.json()["port"], END - 1, "{}", dojo.text);
    d.shutdown();
}

async fn ramsgate_restart() {
    let d = start(&[("TRAINING_HUB", "off")]).await;
    d.director.startup().await;
    let first = d.director.ramsgate.current().unwrap();
    d.stop(END);
    d.until("Ramsgate's exit", |dir| dir.ramsgate.current().is_none()).await;
    assert_eq!(d.get("/api/hub-status").await.json()["ramsgate"], Value::Null);
    assert_eq!(
        d.matchmake(json!({"GameMode": "CITY"})).await.status,
        500,
        "never a dead hub's address"
    );
    // An early exit backs off 5 s, then the supervisor starts it again by itself.
    d.until("Ramsgate's restart", |dir| {
        dir.ramsgate.current().is_some_and(|c| !Arc::ptr_eq(&c, &first))
    })
    .await;
    assert_eq!(d.matchmake(json!({"GameMode": "CITY"})).await.json()["port"], END);
    d.shutdown();
}
