//! The controller's flows (Reforged's `test/controller.test.ts`, on DR's Rust harness) against a fake
//! Reforged server (`test/fakes.ts`): invite -> ServerStatus -> Register -> the player's own Dauntless
//! 1.14.7 -> the server's runtime (asked, then installed and checked) -> Play (relay, Engine.ini, launch
//! arguments, environment) -> the game's exit stops the relay. Public mode runs over a fake TLS gateway,
//! private mode over plain HTTP on loopback. The Host page's cases are RA12's, at the end; the Dauntless Revived case
//! is replaced by the RD22 ones at the end (a saved 1.4.4 server is shown disabled).

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use protocol::invite::{Invite, ServerMode, format_invite};
use protocol::status::GameDataKind;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::platform::{Boxed, Platform, Prepared};
use super::{Config, Controller, ControllerHandle, Seams};
use crate::bridge::types::{
    ActionResult, ErrorCode, GameProblem, HostAction, HostFolder, HostKit, HostServerState, HostServerView,
    HostSetupRequest, InviteCheck, Language, LauncherError, NoticeCode, Phase, RegisterOutcome, RuntimeState, Snapshot,
    TaskProgress,
};
use crate::core::pins::EXE_RELATIVE_PATH;
use crate::net::pinned::certificate_fingerprint;
use crate::net::pinned::tests::{acceptor, certificate};
use crate::platform::dialogs::FolderPurpose;
use crate::platform::game_folder::{GAME_VERSION_TEXT, StoreEnv};
use crate::platform::keystore::Encryptor;
use crate::platform::launch::LaunchRuntime;

const KEY_HEADER: &str = "x-reforged-user-api-key";
const EXE_DATA: &[u8] = b"MZ this is not really Dauntless 1.14.7";

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn pattern(n: usize, seed: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + seed * 7) & 0xff) as u8).collect()
}

/// `makeRuntime`
fn make_runtime(seed: usize) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("MysticParadox.dll", pattern(200_000 + seed, seed)),
        ("winmm.dll", pattern(40_000 + seed, seed + 50)),
    ]
}

fn runtime_manifest(files: &[(&'static str, Vec<u8>)]) -> Value {
    json!({
        "version": "20261014-abcdef12",
        "build": { "name": "rel-1.14.7-Archon", "changelist": 647472 },
        "files": files
            .iter()
            .map(|(name, data)| json!({ "name": name, "size": data.len(), "sha256": sha(data) }))
            .collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------- the fake server

/// `FakeServer`: the backend's launcher API (`/reforged/api/*`) and the content server (`/content/v1/*`)
/// on one address, as the launcher sees them through the gateway.
struct Host {
    name: String,
    /// OPEN, INVITECODE or NONE
    registration: &'static str,
    valid_codes: HashSet<String>,
    /// key -> (username, user id)
    users: HashMap<String, (String, String)>,
    /// key -> reason
    banned: HashMap<String, String>,
    /// None: the host has not published one
    runtime: Option<Vec<(&'static str, Vec<u8>)>>,
    game_data: &'static str,
    registrations: Vec<(String, String)>,
    /// The path of every request
    seen: Vec<String>,
    counter: u32,
    /// The news answer waits for this (the shutdown test)
    news_gate: Option<Arc<tokio::sync::Notify>>,
}

impl Host {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            registration: "INVITECODE",
            valid_codes: HashSet::new(),
            users: HashMap::new(),
            banned: HashMap::new(),
            runtime: None,
            game_data: "real",
            registrations: Vec::new(),
            seen: Vec::new(),
            counter: 0,
            news_gate: None,
        }
    }
}

type Shared = Arc<Mutex<Host>>;

fn reply(status: u16, body: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

fn status_body(host: &Host, full: bool) -> Value {
    json!({
        "name": host.name,
        "online": true,
        "version": "0.1.0",
        "commit": "abc1234",
        "sourceUrl": "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival",
        "registration": host.registration,
        "playersOnline": if full { 1 } else { 0 },
        "players": if full { json!([{ "name": "Aurora", "where": "city", "instance": "city-8777" }]) } else { json!([]) },
        "instances": if full { json!([{ "id": "city-8777", "kind": "city", "title": "Ramsgate", "map": "Hub", "behemoth": null,
            "players": 1, "maxPlayers": 100, "startedAt": chrono::Utc::now().to_rfc3339() }]) } else { json!([]) },
        "uptimeSeconds": 600,
        "game": { "build": "rel-1.14.7-Archon", "changelist": 647472, "executableSha256": null },
        "gameData": host.game_data,
        "chat": true,
        "limited": !full,
    })
}

async fn handle(host: Shared, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let key = request
        .headers()
        .get(KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    host.lock().unwrap().seen.push(path.clone());
    let body = request
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    let user = |host: &Host| key.as_ref().and_then(|key| host.users.get(key)).cloned();
    match path.as_str() {
        "/dauntless-status" => return reply(200, json!({ "show-status": true })),
        "/reforged/api/ServerStatus" => {
            let h = host.lock().unwrap();
            return reply(200, status_body(&h, user(&h).is_some()));
        }
        "/reforged/api/GetUserInfo" => {
            let h = host.lock().unwrap();
            if let Some(reason) = key.as_ref().and_then(|key| h.banned.get(key)) {
                return reply(403, json!({ "error": "account_banned", "reason": reason }));
            }
            return match user(&h) {
                Some((name, id)) => reply(200, json!({ "UserId": id, "Username": name, "IsAdmin": false })),
                None => reply(401, json!({ "error": "unauthorized" })),
            };
        }
        "/reforged/api/Register" if method == hyper::Method::POST => {
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let username = parsed["Username"].as_str().unwrap_or("").to_owned();
            let code = parsed["InviteCode"].as_str().unwrap_or("").to_owned();
            let mut h = host.lock().unwrap();
            if h.registration == "NONE" {
                return reply(400, json!({ "error": "registration_closed" }));
            }
            if h.users.values().any(|(u, _)| u.eq_ignore_ascii_case(&username)) {
                return reply(409, json!({ "error": "username_taken" }));
            }
            if h.registration == "INVITECODE" && !h.valid_codes.remove(&code) {
                return reply(401, json!({ "error": "invite_invalid" }));
            }
            h.registrations.push((username.clone(), code));
            h.counter += 1;
            let key = format!("RRK_{:048x}", h.counter);
            let id = format!("uid-{}", h.counter);
            h.users.insert(key.clone(), (username, id));
            return reply(200, json!({ "UUK": key }));
        }
        "/content/v1/news" => {
            let gate = host.lock().unwrap().news_gate.clone();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            return reply(
                200,
                json!({ "items": [{ "title": "Hello", "body": "First hunt tonight" }] }),
            );
        }
        "/content/v1/branding" => return reply(200, json!({ "backgrounds": [], "accent": null })),
        "/content/v1/runtime/manifest" => {
            let h = host.lock().unwrap();
            return match &h.runtime {
                Some(files) => reply(200, runtime_manifest(files)),
                None => reply(404, json!({ "error": "no_runtime" })),
            };
        }
        _ => {}
    }
    if let Some(name) = path.strip_prefix("/content/v1/runtime/files/") {
        let h = host.lock().unwrap();
        if user(&h).is_none() {
            return reply(401, json!({ "error": "unauthorized" }));
        }
        let file = h
            .runtime
            .as_ref()
            .and_then(|files| files.iter().find(|(n, _)| *n == name));
        return match file {
            Some((_, data)) => Response::builder()
                .status(200)
                .header("content-type", "application/octet-stream")
                .header("content-length", data.len())
                .body(Full::new(Bytes::from(data.clone())))
                .unwrap(),
            None => reply(404, json!({})),
        };
    }
    reply(404, Value::Null)
}

/// Serves the fake on a free loopback port, over TLS when an acceptor is given.
async fn serve(host: Shared, tls: Option<TlsAcceptor>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let host = host.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let host = host.clone();
                    async move { Ok::<_, Infallible>(handle(host, request).await) }
                });
                let builder = hyper::server::conn::http1::Builder::new();
                match tls {
                    Some(tls) => {
                        if let Ok(stream) = tls.accept(tcp).await {
                            let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                        }
                    }
                    None => {
                        let _ = builder.serve_connection(TokioIo::new(tcp), service).await;
                    }
                }
            });
        }
    });
    port
}

/// A public gateway with a fresh certificate: (host, port, fingerprint).
async fn gateway(host: Host) -> (Shared, u16, String) {
    let (cert, key) = certificate();
    let fingerprint = certificate_fingerprint(&cert);
    let shared = Arc::new(Mutex::new(host));
    let port = serve(shared.clone(), Some(acceptor(cert, key))).await;
    (shared, port, fingerprint)
}

async fn lan(host: Host) -> (Shared, u16) {
    let shared = Arc::new(Mutex::new(host));
    let port = serve(shared.clone(), None).await;
    (shared, port)
}

fn invite(mode: ServerMode, host: &str, port: u16, fp: Option<&str>, code: &str, name: &str) -> String {
    format_invite(&Invite {
        mode,
        host: host.into(),
        port,
        code: code.into(),
        name: name.into(),
        share: None,
        fp: fp.map(str::to_owned),
    })
    .unwrap()
}

// ---------------------------------------------------------------- the harness

#[derive(Default)]
struct Fake {
    last: Mutex<Option<Snapshot>>,
    snapshots: Mutex<usize>,
    /// What the folder dialog answers, by purpose
    folders: Mutex<HashMap<&'static str, PathBuf>>,
    missing_vc: Mutex<Vec<String>>,
}

fn purpose_name(purpose: FolderPurpose) -> &'static str {
    match purpose {
        FolderPurpose::Game => "game",
        FolderPurpose::Runtime => "runtime",
        FolderPurpose::Sdk => "sdk",
        FolderPurpose::Workspace => "workspace",
    }
}

impl Platform for Fake {
    fn choose_folder(&self, _: Language, purpose: FolderPurpose, _: PathBuf) -> Boxed<Option<PathBuf>> {
        let picked = self.folders.lock().unwrap().get(purpose_name(purpose)).cloned();
        Box::pin(async move { picked })
    }
    fn choose_save_file(&self, _: String) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
    fn choose_open_file(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
    fn choose_dll(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
    fn choose_pack(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
    fn open_url(&self, _: &str, _: Option<&str>) {}
    fn open_folder(&self, _: &Path) {}
    fn emit_snapshot(&self, snapshot: &Snapshot) {
        *self.last.lock().unwrap() = Some(snapshot.clone());
        *self.snapshots.lock().unwrap() += 1;
    }
    fn emit_progress(&self, _: &TaskProgress) {}
    fn emit_host_kit(&self, _: &HostKit) {}
    fn emit_host_server(&self, _: &HostServerView) {}
    fn put_art(&self, _: &str, _: PathBuf, _: &'static str) {}
    fn find_tailscale(&self) -> bool {
        false
    }
    fn find_running_clients(&self) -> Boxed<Vec<u32>> {
        Box::pin(async { Vec::new() })
    }
    fn prepare_game_launch(&self) -> Boxed<Prepared> {
        Box::pin(async { Prepared::Native })
    }
    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>) {
        (self.missing_vc.lock().unwrap().clone(), Vec::new())
    }
}

struct Plain;

impl Encryptor for Plain {
    fn is_available(&self) -> bool {
        true
    }
    fn encrypt(&self, plain: &str) -> Option<Vec<u8>> {
        Some(format!("enc:{}", base64::engine::general_purpose::STANDARD.encode(plain)).into_bytes())
    }
    fn decrypt(&self, data: &[u8]) -> Option<String> {
        let text = std::str::from_utf8(data).ok()?.strip_prefix("enc:")?;
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(text).ok()?).ok()
    }
}

/// One started stand-in game: (Win64 folder, arguments, the whole environment).
type Spawned = (PathBuf, Vec<String>, Vec<(OsString, OsString)>);

struct Harness {
    c: ControllerHandle,
    fake: Arc<Fake>,
    spawns: Arc<Mutex<Vec<Spawned>>>,
    relay_port: u16,
    user: tempfile::TempDir,
    config: tempfile::TempDir,
    temps: Mutex<Vec<tempfile::TempDir>>,
}

/// A port nothing listens on now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Harness {
    fn new() -> Self {
        Self::with_user(tempfile::tempdir().unwrap())
    }

    fn with_user(user: tempfile::TempDir) -> Self {
        let config = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let spawns: Arc<Mutex<Vec<Spawned>>> = Arc::default();
        let log = spawns.clone();
        let relay_port = free_port();
        let c = Controller::with_seams(
            fake.clone(),
            Config {
                user_data: user.path().to_path_buf(),
                documents: None,
                resources_dir: PathBuf::from("resources"),
                host: crate::platform::settings::Platform::current(),
                app_version: "0.0.0-test".into(),
                packaged: false,
                language: Language::En,
                system_locale: "pt-BR".into(),
                game_config_dir: config.path().to_path_buf(),
                relay_port,
                exe_pin: (EXE_RELATIVE_PATH.into(), sha(EXE_DATA)),
                status_poll: Duration::from_secs(60),
                encryptor: Arc::new(Plain),
                legacy_encryptor: None,
            },
            Seams {
                // Requests the test sends through the relay stand in for the game, so the game is this
                // process (the relay's owner check, RD21)
                spawn: Some(Arc::new(
                    move |dir: &Path, args: &[String], _: Option<&LaunchRuntime>, env: &[(OsString, OsString)]| {
                        log.lock()
                            .unwrap()
                            .push((dir.to_path_buf(), args.to_vec(), env.to_vec()));
                        Ok(std::process::id())
                    },
                )),
                env: Some(vec![
                    ("PATH".into(), "/usr/bin".into()),
                    (
                        "REFORGED_BACKEND_URL".into(),
                        "https://should-be-replaced.invalid".into(),
                    ),
                ]),
                store_env: Some(StoreEnv::default()),
                qos_port: None,
                host_probe: None,
                dump_root: None,
            },
        );
        Self {
            c,
            fake,
            spawns,
            relay_port,
            user,
            config,
            temps: Mutex::new(Vec::new()),
        }
    }

    fn last(&self) -> Snapshot {
        self.fake
            .last
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.c.snapshot())
    }

    fn temp(&self) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        self.temps.lock().unwrap().push(dir);
        path
    }

    /// A Dauntless folder with a stand-in executable and `Version.txt`.
    fn fake_game(&self, version: &str, exe: &[u8]) -> PathBuf {
        let root = self.temp().join("Dauntless");
        let win64 = root.join("Archon").join("Binaries").join("Win64");
        std::fs::create_dir_all(&win64).unwrap();
        std::fs::write(win64.join("Dauntless-Win64-Shipping.exe"), exe).unwrap();
        std::fs::write(root.join("Version.txt"), version).unwrap();
        root
    }

    fn game(&self) -> PathBuf {
        self.fake_game(GAME_VERSION_TEXT, EXE_DATA)
    }

    fn pick(&self, purpose: &'static str, dir: &Path) {
        self.fake.folders.lock().unwrap().insert(purpose, dir.to_path_buf());
    }

    fn relay_active(&self) -> bool {
        self.c.keep_open().is_some()
    }

    /// The stand-in game exits; waits until the launcher is back to `phase`.
    async fn exit(&self, code: i32) {
        self.c.game.ended(Some(code));
        self.wait(|s| s.phase == Phase::Ready && s.running.relay_port.is_none())
            .await;
        assert!(!self.relay_active());
    }

    async fn wait(&self, done: impl Fn(&Snapshot) -> bool) {
        for _ in 0..150 {
            if done(&self.last()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out: {:?}", self.last());
    }
}

fn error(code: ErrorCode) -> ActionResult {
    ActionResult::err(code)
}

/// `GET` on the relay, as the game would: (status, body).
async fn get(port: u16, path: &str) -> (u16, String) {
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await.unwrap();
    tokio::spawn(connection);
    let request = Request::get(path)
        .header("host", format!("127.0.0.1:{port}"))
        .body(Full::new(Bytes::new()))
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn arg<'a>(args: &'a [String], prefix: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(prefix))
}

fn env_of<'a>(env: &'a [(OsString, OsString)], name: &str) -> Vec<&'a OsString> {
    env.iter().filter(|(k, _)| k == name).map(|(_, v)| v).collect()
}

// ---------------------------------------------------------------- Reforged's cases

#[tokio::test(flavor = "multi_thread")]
async fn public_mode_end_to_end_join_register_game_check_runtime_trust_and_install_play_exit() {
    let runtime = make_runtime(5);
    let mut host = Host::new("Friday Hunts");
    host.valid_codes = ["ABCD-EFGH-JKLM".to_owned()].into();
    host.runtime = Some(runtime.clone());
    let (gw, port, fp) = gateway(host).await;
    let hx = Harness::new();
    hx.c.init().await;
    assert_eq!(hx.last().phase, Phase::Join);

    let text = invite(
        ServerMode::Public,
        "127.0.0.1",
        port,
        Some(&fp),
        "ABCD-EFGH-JKLM",
        "Friday Hunts",
    );
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    let s = hx.last();
    assert_eq!(s.phase, Phase::Register);
    assert_eq!(s.server.as_ref().unwrap().fingerprint.as_deref(), Some(fp.as_str()));
    assert!(s.status.as_ref().unwrap().limited);

    assert_eq!(
        hx.c.register("Slayer_42").await,
        RegisterOutcome::Ok {
            username: "Slayer_42".into()
        }
    );
    let s = hx.last();
    assert_eq!(s.phase, Phase::Locate, "the launcher never downloads the game");
    assert_eq!(s.game.problem, Some(GameProblem::NotSet));
    assert!(!s.status.as_ref().unwrap().limited);
    assert_eq!(gw.lock().unwrap().registrations.last().unwrap().1, "ABCD-EFGH-JKLM");

    // A 1.14.7 folder whose executable is another build
    let wrong = hx.fake_game(GAME_VERSION_TEXT, b"another build");
    assert_eq!(
        hx.c.use_game_path(&wrong.to_string_lossy()).await,
        error(ErrorCode::GameWrongVersion)
    );
    assert_eq!(hx.last().game.problem, Some(GameProblem::WrongVersion));

    let root = hx.game();
    assert_eq!(
        hx.c.use_game_path(&root.join("Archon").to_string_lossy()).await,
        ActionResult::Ok
    );
    let s = hx.last();
    assert_eq!(s.game.root.as_deref(), Some(root.to_string_lossy().as_ref()));
    assert!(s.game.exe_verified);
    assert_eq!(s.notice, Some(NoticeCode::GameVerified));
    assert_eq!(s.phase, Phase::Runtime);
    assert_eq!(
        s.runtime.state,
        RuntimeState::Untrusted,
        "the player is asked before any native code is installed"
    );
    let names: Vec<&str> = s.runtime.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["MysticParadox.dll", "winmm.dll"]);
    let downloaded = |gw: &Shared| {
        gw.lock()
            .unwrap()
            .seen
            .iter()
            .any(|p| p.starts_with("/content/v1/runtime/files/"))
    };
    assert!(!downloaded(&gw), "nothing downloaded before the player agreed");

    assert_eq!(hx.c.play().await, error(ErrorCode::RuntimeUntrusted));

    assert_eq!(hx.c.trust_runtime().await, ActionResult::Ok);
    let s = hx.last();
    assert_eq!(s.phase, Phase::Ready);
    assert_eq!(s.runtime.state, RuntimeState::Ok);
    assert_eq!(s.notice, Some(NoticeCode::RuntimeInstalled));
    let win64 = root.join("Archon").join("Binaries").join("Win64");
    for (name, data) in &runtime {
        assert_eq!(&std::fs::read(win64.join(name)).unwrap(), data, "{name}");
    }

    // Left by Dauntless Revived's launcher (local-server mode): 1.14.7 would send its login checks to
    // 127.0.0.1
    let game_ini = hx.config.path().join("Game.ini");
    std::fs::write(
        &game_ini,
        "[OnlineSubsystemPhoenix]\r\nCheckCrossPlayProgressionEndpoint=\"http://127.0.0.1:61000/features/platform/{platform}\"\r\n[Mine]\r\nKeep=1\r\n",
    )
    .unwrap();
    assert_eq!(hx.c.play().await, ActionResult::Ok);
    let cleaned = String::from_utf8_lossy(&std::fs::read(&game_ini).unwrap()).into_owned();
    assert!(
        !cleaned.contains("OnlineSubsystemPhoenix") && cleaned.contains("Keep=1"),
        "Revived's endpoints removed, the rest kept: {cleaned}"
    );
    assert!(hx.config.path().join("Game.ini.before-reforged").exists());
    let s = hx.last();
    assert_eq!(s.phase, Phase::Running);
    assert_eq!(s.running.relay_port, Some(hx.relay_port));
    assert!(hx.relay_active());

    let (dir, args, env) = hx.spawns.lock().unwrap().last().cloned().unwrap();
    assert_eq!(dir, win64);
    assert!(args.iter().any(|a| a == "-AUTH_TYPE=exchangecode"));
    let password = arg(&args, "-AUTH_PASSWORD=").unwrap();
    assert!(password.starts_with("RRK_") && password.len() == 52, "{password}");
    assert_eq!(arg(&args, "-epicusername="), Some("Slayer_42"));
    assert_eq!(
        arg(&args, "-epiclocale="),
        Some("pt-BR"),
        "game language auto follows the system locale"
    );
    let relay_url = OsString::from(format!("http://127.0.0.1:{}", hx.relay_port));
    assert_eq!(env_of(&env, "REFORGED_BACKEND_URL"), [&relay_url]);
    assert_eq!(
        env_of(&env, "MYSTICPARADOX_RUNTIME_PROFILE"),
        [&OsString::from("production")]
    );
    assert_eq!(env_of(&env, "PATH"), [&OsString::from("/usr/bin")]);

    let engine_ini = String::from_utf8_lossy(&std::fs::read(hx.config.path().join("Engine.ini")).unwrap()).into_owned();
    assert!(
        engine_ini.contains(&format!("ServerPort={}", hx.relay_port))
            && engine_ini.contains("ServerAddr=\"ws://127.0.0.1\""),
        "{engine_ini}"
    );

    // The relay answers QoS itself and carries everything else over the pinned connection
    let (status, body) = get(hx.relay_port, "/QoS").await;
    assert_eq!(status, 200);
    assert!(body.contains("pong"), "{body}");
    let before = gw.lock().unwrap().seen.len();
    let (status, _) = get(hx.relay_port, "/reforged/api/RegistrationStatus").await;
    assert_eq!(status, 404, "the fake has no such route: the request reached it");
    assert_eq!(gw.lock().unwrap().seen.len(), before + 1);
    assert_eq!(hx.c.play().await, error(ErrorCode::AlreadyRunning));

    hx.exit(0).await;

    // Closed through its console (0xC000013A): not a failure. A crash code is reported
    assert_eq!(hx.c.play().await, ActionResult::Ok);
    hx.exit(0xC000_013Au32 as i32).await;
    assert_eq!(hx.last().last_error, None);
    assert_eq!(hx.c.play().await, ActionResult::Ok);
    hx.exit(0xC000_0005u32 as i32).await;
    assert_eq!(
        hx.last().last_error,
        Some(LauncherError::with_detail(ErrorCode::LaunchFailed, "0xC0000005"))
    );
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_invite_with_another_certificate_nothing_is_sent_and_new_invite_is_offered() {
    let (gw, port, _) = gateway(Host::new("Fake")).await;
    let (other, _) = certificate();
    let other = certificate_fingerprint(&other);
    let hx = Harness::new();
    hx.c.init().await;
    let before = gw.lock().unwrap().seen.len();
    let text = invite(
        ServerMode::Public,
        "127.0.0.1",
        port,
        Some(&other),
        "SECOND-CODE",
        "Fake",
    );
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    let s = hx.last();
    assert_eq!(s.phase, Phase::Connect);
    assert_eq!(
        s.connect.problem,
        Some(crate::bridge::types::ConnectProblem::CertMismatch)
    );
    assert_eq!(gw.lock().unwrap().seen.len(), before);
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn private_mode_open_registration_a_server_without_a_runtime_waits_for_its_host() {
    let mut host = Host::new("LAN Party");
    host.registration = "OPEN";
    host.game_data = "synthetic";
    let (_lan, port) = lan(host).await;
    let hx = Harness::new();
    let local = hx.temp();
    hx.pick("runtime", &local);
    hx.c.init().await;
    let text = invite(ServerMode::Private, "127.0.0.1", port, None, "OPEN-ANYONE", "LAN Party");
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    assert_eq!(
        hx.c.register("Borealis").await,
        RegisterOutcome::Ok {
            username: "Borealis".into()
        }
    );
    assert_eq!(hx.c.use_game_path(&hx.game().to_string_lossy()).await, ActionResult::Ok);
    let s = hx.last();
    assert_eq!(s.phase, Phase::Runtime);
    assert_eq!(s.runtime.state, RuntimeState::Unavailable);
    assert_eq!(s.status.as_ref().unwrap().game_data, GameDataKind::Synthetic);
    assert_eq!(hx.c.play().await, error(ErrorCode::RuntimeUnavailable));

    // A host testing their own build: a local runtime folder makes it playable
    assert_eq!(
        hx.c.choose_local_runtime().await,
        ActionResult::Err(LauncherError::with_detail(
            ErrorCode::LocalRuntimeInvalid,
            "MysticParadox.dll, winmm.dll"
        ))
    );
    for (name, data) in make_runtime(9) {
        std::fs::write(local.join(name), data).unwrap();
    }
    assert_eq!(hx.c.choose_local_runtime().await, ActionResult::Ok);
    assert_eq!(hx.last().runtime.state, RuntimeState::Local);
    assert_eq!(hx.last().phase, Phase::Ready);
    assert_eq!(hx.c.play().await, ActionResult::Ok);
    let (dir, _, _) = hx.spawns.lock().unwrap().last().cloned().unwrap();
    assert!(dir.join("MysticParadox.dll").exists());
    hx.exit(0).await;
    assert_eq!(hx.c.clear_local_runtime().await, ActionResult::Ok);
    assert_eq!(hx.last().runtime.local_dir, None);
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_server_at_a_new_address_the_account_goes_along_only_when_the_player_says_so() {
    let mut host = Host::new("LAN Party");
    host.registration = "OPEN";
    let (_lan, port) = lan(host).await;
    let hx = Harness::new();
    hx.c.init().await;
    // The launcher keeps the name the server itself reports, as the host kit's invites carry it
    let first = invite(ServerMode::Private, "127.0.0.1", port, None, "OPEN-ANYONE", "LAN Party");
    assert_eq!(hx.c.submit_invite(&first, false, false).await, ActionResult::Ok);
    assert_eq!(
        hx.c.register("Wanderer").await,
        RegisterOutcome::Ok {
            username: "Wanderer".into()
        }
    );
    assert!(hx.last().account.has_key);

    let moved = invite(ServerMode::Private, "127.0.0.2", port, None, "NEW-CODE", "LAN Party");
    assert_eq!(
        hx.c.check_invite(&moved),
        InviteCheck::Ok {
            certificate_changed: false,
            previous_fingerprint: None,
            moved_from: Some("127.0.0.1".into()),
        }
    );
    let other = invite(
        ServerMode::Private,
        "127.0.0.2",
        port,
        None,
        "NEW-CODE",
        "Another server",
    );
    assert!(
        matches!(hx.c.check_invite(&other), InviteCheck::Ok { moved_from: None, .. }),
        "another server's name: not the same server"
    );

    // Without the player's yes, nothing is taken along
    assert_eq!(hx.c.submit_invite(&moved, false, false).await, ActionResult::Ok);
    assert!(!hx.last().account.has_key);
    // With it, the key moves to the new address and the old entry goes
    assert_eq!(hx.c.submit_invite(&moved, false, true).await, ActionResult::Ok);
    let s = hx.last();
    assert!(s.account.has_key);
    assert_eq!(s.account.username.as_deref(), Some("Wanderer"));
    let hosts: Vec<&str> = s.saved_servers.iter().map(|sv| sv.host.as_str()).collect();
    assert_eq!(hosts, ["127.0.0.2"]);
    assert!(
        matches!(hx.c.check_invite(&moved), InviteCheck::Ok { moved_from: None, .. }),
        "nothing to take along any more"
    );
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_banned_account_is_told_why() {
    let key = format!("RRK_{}", "b".repeat(48));
    let mut host = Host::new("Strict");
    host.registration = "OPEN";
    host.users.insert(key.clone(), ("ghost".into(), "uid-ghost".into()));
    host.banned.insert(key.clone(), "cheating".into());
    let (_lan, port) = lan(host).await;
    let hx = Harness::new();
    hx.c.init().await;
    let text = invite(ServerMode::Private, "127.0.0.1", port, None, "STRICT-CODE", "Strict");
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    let banned = ActionResult::Err(LauncherError::with_detail(ErrorCode::AccountBanned, "cheating"));
    assert_eq!(hx.c.use_existing_key(&key).await, banned);
    let s = hx.last();
    assert_eq!(s.phase, Phase::Register);
    assert_eq!(
        s.last_error,
        Some(LauncherError::with_detail(ErrorCode::AccountBanned, "cheating"))
    );
    hx.c.shutdown().await;
}

// ---------------------------------------------------------------- the game folder

#[tokio::test(flavor = "multi_thread")]
async fn the_game_folder_is_chosen_detected_and_checked_once() {
    let hx = Harness::new();
    hx.c.init().await;
    assert_eq!(hx.c.choose_game_folder().await, error(ErrorCode::Cancelled));
    let root = hx.game();
    hx.pick("game", &root.join("Archon").join("Binaries").join("Win64"));
    assert_eq!(hx.c.choose_game_folder().await, ActionResult::Ok);
    let s = hx.last();
    assert_eq!(s.game.root.as_deref(), Some(root.to_string_lossy().as_ref()));
    assert_eq!(s.game.problem, None);
    assert!(s.game.version_ok);
    assert_eq!(s.game.version_text.as_deref(), Some(GAME_VERSION_TEXT));

    assert_eq!(
        hx.c.use_game_path("relative\\path").await,
        error(ErrorCode::FolderInvalid)
    );
    let empty = hx.temp();
    assert_eq!(
        hx.c.use_game_path(&empty.to_string_lossy()).await,
        error(ErrorCode::GameFolderNotFound)
    );
    assert_eq!(hx.c.use_detected_game(3).await, error(ErrorCode::FolderInvalid));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_game_found_unverified_is_checked_at_start_and_a_restart_trusts_the_cached_hash() {
    let first = Harness::new();
    let root = first.game();
    // Stored as chosen, never checked (an upgraded profile)
    let settings = json!({ "gameRoot": root.to_string_lossy() });
    std::fs::write(first.user.path().join("settings.json"), settings.to_string()).unwrap();
    let user = first.user;
    let hx = Harness::with_user(user);
    hx.c.init().await;
    hx.wait(|s| s.game.exe_verified).await;
    assert_eq!(hx.last().notice, Some(NoticeCode::GameVerified));
    hx.c.shutdown().await;
    let user = hx.user;
    let again = Harness::with_user(user);
    again.c.init().await;
    let s = again.last();
    assert!(s.game.exe_verified, "the cached hash is trusted");
    assert_eq!(s.game.problem, None);
    assert!(again.c.snapshot().task.is_none());
    drop(first.temps);
}

// ---------------------------------------------------------------- RD22

#[tokio::test(flavor = "multi_thread")]
async fn missing_visual_cpp_files_only_warn_before_play() {
    let mut host = Host::new("LAN Party");
    host.registration = "OPEN";
    let (_lan, port) = lan(host).await;
    let hx = Harness::new();
    let local = hx.temp();
    for (name, data) in make_runtime(2) {
        std::fs::write(local.join(name), data).unwrap();
    }
    hx.pick("runtime", &local);
    *hx.fake.missing_vc.lock().unwrap() = vec!["VCRUNTIME140_1.dll".into()];
    hx.c.init().await;
    let text = invite(ServerMode::Private, "127.0.0.1", port, None, "OPEN-ANYONE", "LAN Party");
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    assert!(matches!(hx.c.register("Vc_Less").await, RegisterOutcome::Ok { .. }));
    assert_eq!(hx.c.use_game_path(&hx.game().to_string_lossy()).await, ActionResult::Ok);
    assert_eq!(hx.c.choose_local_runtime().await, ActionResult::Ok);
    assert_eq!(hx.c.play().await, ActionResult::Ok, "the game starts anyway");
    hx.exit(0).await;
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_waits_for_the_news_and_refuses_play() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut host = Host::new("Slow News");
    host.registration = "OPEN";
    host.news_gate = Some(gate.clone());
    let (lan_host, port) = lan(host).await;
    let hx = Harness::new();
    hx.c.init().await;
    let text = invite(ServerMode::Private, "127.0.0.1", port, None, "OPEN-ANYONE", "Slow News");
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    // The news request is on its way and held
    for _ in 0..100 {
        if lan_host.lock().unwrap().seen.iter().any(|p| p == "/content/v1/news") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let c = hx.c.clone();
    let shutdown = tokio::spawn(async move { c.shutdown().await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!shutdown.is_finished(), "shutdown waits for the news");
    assert_eq!(hx.c.play().await, error(ErrorCode::Busy), "no PLAY while shutting down");
    assert!(!hx.c.may_install_update());
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hx.c.news().len(), 1, "the news that was loading arrived");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_1_4_4_server_is_shown_disabled_and_never_joined() {
    let hx = Harness::new();
    let revived = json!({ "game": "revived", "mode": "private", "host": "100.64.0.9", "port": 61000,
        "name": "Old Hunts", "share": null, "fp": null, "code": null });
    let settings = json!({ "server": revived, "savedServers": [revived] });
    std::fs::write(hx.user.path().join("settings.json"), settings.to_string()).unwrap();
    let hx = Harness::with_user(hx.user);
    hx.c.init().await;
    let s = hx.last();
    assert_eq!(s.phase, Phase::Join, "never the current server");
    assert_eq!(s.saved_servers.len(), 1);
    let saved = &s.saved_servers[0];
    assert!(saved.unsupported);
    assert!(!saved.current);
    assert_eq!(
        hx.c.switch_server(&saved.id).await,
        ActionResult::Err(LauncherError::with_detail(ErrorCode::InviteInvalidFormat, "revived"))
    );
    assert_eq!(hx.last().phase, Phase::Join);
    // A Dauntless Revived invite is refused the same way (RD5)
    let text = "dauntless-revived://join?v=1&h=100.64.0.9&p=61000&c=ABCD&n=Old";
    assert_eq!(
        hx.c.check_invite(text),
        InviteCheck::Err(LauncherError::with_detail(ErrorCode::InviteInvalidFormat, "revived"))
    );
    // It can still be taken off the list
    assert_eq!(hx.c.remove_saved_server(&saved.id).await, ActionResult::Ok);
    assert!(hx.last().saved_servers.is_empty());
    let page = serde_json::to_value(hx.c.snapshot()).unwrap();
    assert!(page.get("revived").is_none(), "1.4.4 is not ported (RD5)");
}

// ---------------------------------------------------------------- the rest of the page's commands

#[tokio::test(flavor = "multi_thread")]
async fn a_first_start_asks_for_an_invite() {
    let hx = Harness::new();
    hx.c.init().await;
    let s = hx.last();
    assert_eq!(s, hx.c.snapshot());
    assert_eq!(s.phase, Phase::Join);
    assert!(s.server.is_none());
    assert_eq!(s.game.problem, Some(GameProblem::NotSet));
    assert_eq!(s.runtime.state, RuntimeState::Unavailable);
    assert_eq!(s.app.version, "0.0.0-test");
    assert_eq!(s.app.platform, if cfg!(windows) { "win32" } else { "linux" });
    assert_eq!(s.settings.language, Language::En);
    // Nothing to do without a server
    assert_eq!(hx.c.connect().await, error(ErrorCode::ServerUnreachable));
    assert_eq!(hx.c.play().await, error(ErrorCode::ServerUnreachable));
    assert_eq!(hx.c.trust_runtime().await, error(ErrorCode::RuntimeUnavailable));
    assert_eq!(hx.c.install_runtime().await, error(ErrorCode::ServerUnreachable));
    assert_eq!(hx.c.verify_game().await, error(ErrorCode::GameFolderNotFound));
    assert_eq!(hx.c.save_key_backup().await, error(ErrorCode::ServerUnreachable));
    assert_eq!(hx.c.logout().await, ActionResult::Ok);
    assert_eq!(hx.c.import_key_file().await, error(ErrorCode::Cancelled));
    assert!(matches!(hx.c.check_invite("nonsense"), InviteCheck::Err(_)));
    assert!(matches!(
        hx.c.submit_invite("nonsense", false, false).await,
        ActionResult::Err(LauncherError {
            code: ErrorCode::InviteInvalidFormat,
            ..
        })
    ));
    assert!(hx.c.news().is_empty());
    assert!(hx.c.branding().backgrounds.is_empty());
    assert_eq!(
        hx.c.open_external(crate::bridge::types::ExternalTarget::ProjectSource),
        ActionResult::Ok
    );
    assert_eq!(
        hx.c.open_external(crate::bridge::types::ExternalTarget::TailscaleShare),
        error(ErrorCode::Unknown),
        "no private server, no share link"
    );
    hx.c.open_game_folder();
    hx.c.open_logs_folder();
    hx.c.cancel_task();
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_take_reforgeds_seven_keys() {
    use crate::bridge::types::{GameLanguage, GraphicsPreset, RuntimeProfile, SettingsPatch};
    let hx = Harness::new();
    hx.c.init().await;
    let patch = SettingsPatch {
        graphics: Some(GraphicsPreset::try_from(3).unwrap()),
        windowed: Some(true),
        streamer_mode: Some(true),
        host_tools: Some(true),
        language: Some(Language::PtBr),
        game_language: Some(GameLanguage::DeDe),
        runtime_profile: Some(RuntimeProfile::Development),
    };
    let settings = hx.c.set_settings(patch).await;
    assert_eq!(settings.graphics.value(), 3);
    assert!(settings.windowed && settings.streamer_mode && settings.host_tools);
    assert_eq!(settings.language, Language::PtBr);
    assert_eq!(settings.game_language, GameLanguage::DeDe);
    assert_eq!(settings.runtime_profile, RuntimeProfile::Development);
    assert_eq!(hx.c.language(), Language::PtBr);
    let stored: Value = serde_json::from_slice(&std::fs::read(hx.user.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(stored["runtimeProfile"], "development");
    assert_eq!(stored["language"], "pt-BR");
    // An empty patch changes nothing
    assert_eq!(hx.c.set_settings(SettingsPatch::default()).await, settings);
}

#[tokio::test(flavor = "multi_thread")]
async fn notices_errors_and_updates() {
    let hx = Harness::new();
    hx.c.set_notice(Some(NoticeCode::KeepOpen));
    assert_eq!(hx.c.snapshot().notice, Some(NoticeCode::KeepOpen));
    hx.c.set_notice(None);
    hx.c.dismiss_error();
    assert_eq!(*hx.fake.snapshots.lock().unwrap(), 3);
    assert!(!hx.c.may_install_update());
    hx.c.set_update_ready();
    assert!(hx.c.snapshot().app.update_ready);
    assert!(hx.c.may_install_update());
    assert!(hx.c.keep_open().is_none());
    hx.c.shutdown().await;
    assert!(!hx.c.may_install_update());
}

// ---------------------------------------------------------------- the Host page (RA12)

/// A workspace in the planned layout with the fake `reforged-host` (`fixtures/host`) in it.
fn host_workspace(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root.join("paradox")).unwrap();
    std::fs::copy(
        crate::platform::hostkit::fixtures::exe("fake-host-kit"),
        root.join(crate::platform::hostkit::HOST_BINARY),
    )
    .unwrap();
    root.to_path_buf()
}

/// `host.json` and both game data sets, so the server may start.
fn configure_workspace(ws: &Path) {
    use crate::platform::hostkit::{BACKEND_GAME_DATA, DIRECTOR_GAME_DATA, WorkspacePaths};
    let w = WorkspacePaths::new(ws);
    std::fs::create_dir_all(w.host_json.parent().unwrap()).unwrap();
    std::fs::write(&w.host_json, r#"{"mode":"private","name":"Friday Hunts"}"#).unwrap();
    for (dir, files) in [
        (&w.backend_game_data, &BACKEND_GAME_DATA[..]),
        (&w.director_game_data, &DIRECTOR_GAME_DATA[..]),
    ] {
        std::fs::create_dir_all(dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), b"{}").unwrap();
        }
    }
}

fn host_error(detail: &str) -> ActionResult {
    ActionResult::Err(LauncherError::with_detail(ErrorCode::HostFailed, detail))
}

/// A harness whose Host page sees no server on port 61000 unless it started one, and its own dump root.
fn host_harness() -> Harness {
    let hx = Harness::new();
    let probe: crate::platform::hostserver::Probe = Arc::new(|| Box::pin(async { false }));
    // Rebuilt with the seams: the Host state is made with the controller
    let dumps = hx.user.path().join("dumps");
    let c = Controller::with_seams(
        hx.fake.clone(),
        Config {
            user_data: hx.user.path().to_path_buf(),
            documents: None,
            resources_dir: PathBuf::from("resources"),
            host: crate::platform::settings::Platform::current(),
            app_version: "0.0.0-test".into(),
            packaged: false,
            language: Language::En,
            system_locale: "en-US".into(),
            game_config_dir: hx.config.path().to_path_buf(),
            relay_port: hx.relay_port,
            exe_pin: (EXE_RELATIVE_PATH.into(), sha(EXE_DATA)),
            status_poll: Duration::from_secs(60),
            encryptor: Arc::new(Plain),
            legacy_encryptor: None,
        },
        Seams {
            spawn: {
                let log = hx.spawns.clone();
                Some(Arc::new(
                    move |dir: &Path, args: &[String], _: Option<&LaunchRuntime>, env: &[(OsString, OsString)]| {
                        log.lock()
                            .unwrap()
                            .push((dir.to_path_buf(), args.to_vec(), env.to_vec()));
                        Ok(std::process::id())
                    },
                ))
            },
            env: Some(vec![
                ("PATH".into(), "/usr/bin".into()),
                ("REFORGED_BACKEND_URL".into(), "https://x.invalid".into()),
                ("MysticParadox_Profile".into(), "development".into()),
            ]),
            store_env: Some(StoreEnv::default()),
            qos_port: None,
            host_probe: Some(probe),
            dump_root: Some(dumps),
        },
    );
    Harness { c, ..hx }
}

async fn until(what: impl Fn() -> bool) {
    for _ in 0..250 {
        if what() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_page_drives_reforged_host_setup_server_invite_and_stop() {
    let hx = host_harness();
    hx.c.init().await;
    assert_eq!(hx.c.host_server_start().await, host_error("workspace"));

    // A folder without reforged-host is not a workspace
    let not_one = hx.temp();
    hx.pick("workspace", &not_one);
    assert_eq!(hx.c.host_choose_workspace().await, host_error("workspace"));
    let ws = host_workspace(&hx.temp().join("Reforged"));
    hx.pick("workspace", &ws);
    assert_eq!(hx.c.host_choose_workspace().await, ActionResult::Ok);
    let kit = hx.c.host_kit().await;
    assert!(kit.workspace_ok);
    assert!(!kit.server_config.configured);
    assert!(!kit.requirements.is_empty(), "the requirements are looked at");
    assert_eq!(hx.c.host_open(HostFolder::Workspace), ActionResult::Ok);
    assert_eq!(hx.c.host_open(HostFolder::HostLogs), host_error("folder"));

    // The server needs host.json and the game data
    assert_eq!(hx.c.host_server_start().await, host_error("not_configured"));
    // The fake host kit fails setup with an Error: line: that line is the page's detail, and the job failed
    let setup = HostSetupRequest {
        name: "Friday Hunts".into(),
        mode: crate::bridge::types::HostServerMode::Private,
        public_host: None,
    };
    assert_eq!(
        hx.c.host_setup(setup).await,
        host_error("game data is missing: reforged-host gamedata")
    );
    let job = hx.c.host_kit().await.job.unwrap();
    assert_eq!(job.state, crate::bridge::types::HostJobState::Failed);
    assert!(job.log_file.unwrap().ends_with("launcher-setup.log"));
    assert_eq!(hx.c.host_create_invite(None).await, host_error("server_not_running"));

    configure_workspace(&ws);
    assert_eq!(hx.c.host_server_start().await, ActionResult::Ok);
    until(|| hx.c.host_server_view().state == HostServerState::Running).await;
    assert!(hx.c.st().host_server_up);
    assert!(
        !hx.c
            .host_server_view()
            .lines
            .iter()
            .any(|l| l.contains("REFORGED_SERVER_READY")),
        "the ready line is not shown"
    );
    // Setup refuses a running server; the invite's link reaches the view only
    let setup = HostSetupRequest {
        name: "x".into(),
        mode: crate::bridge::types::HostServerMode::Private,
        public_host: None,
    };
    assert_eq!(hx.c.host_setup(setup).await, host_error("server_running"));
    assert_eq!(hx.c.host_create_invite(Some("Sam".into())).await, ActionResult::Ok);
    let view = hx.c.host_server_view();
    let invite = view.invite.unwrap();
    assert_eq!(invite.link, "reforged-revival://join/abc");
    assert_eq!(invite.for_name.as_deref(), Some("Sam"));
    assert!(!view.lines.iter().any(|l| l.contains("reforged-revival://")));
    let log = std::fs::read_to_string(ws.join(".workspace").join("logs").join("launcher-invite.log")).unwrap();
    assert!(!log.contains("reforged-revival://"), "the link is never logged");

    // The launcher's shutdown stops the server it started
    hx.c.shutdown().await;
    until(|| hx.c.host_server_view().state == HostServerState::Stopped).await;
    assert!(!hx.c.st().host_server_up);
}

#[tokio::test(flavor = "multi_thread")]
async fn host_tools_turned_on_take_the_project_folder_beside_the_game() {
    use crate::bridge::types::SettingsPatch;
    let hx = host_harness();
    let game = hx.game();
    hx.c.init().await;
    assert_eq!(hx.c.use_game_path(&game.to_string_lossy()).await, ActionResult::Ok);
    let beside = game.parent().unwrap().join("Reforged");
    assert_eq!(
        hx.c.host_kit().await.project_target.as_deref(),
        Some(beside.to_string_lossy().as_ref())
    );
    // Not there yet: the automatic look says nothing, the page's button says why
    hx.c.set_settings(SettingsPatch {
        host_tools: Some(true),
        ..SettingsPatch::default()
    })
    .await;
    assert_eq!(hx.c.s().host_workspace, None);
    assert_eq!(hx.c.snapshot().last_error, None);
    assert_eq!(hx.c.host_download_project().await, host_error("project_missing"));

    host_workspace(&beside);
    hx.c.set_settings(SettingsPatch {
        host_tools: Some(false),
        ..SettingsPatch::default()
    })
    .await;
    hx.c.set_settings(SettingsPatch {
        host_tools: Some(true),
        ..SettingsPatch::default()
    })
    .await;
    assert_eq!(
        hx.c.s().host_workspace.as_deref(),
        Some(beside.to_string_lossy().as_ref())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn import_sdk_takes_the_newest_dump_and_multipliers_are_private_only() {
    let hx = host_harness();
    hx.c.init().await;
    let ws = host_workspace(&hx.temp().join("Reforged"));
    hx.pick("workspace", &ws);
    assert_eq!(hx.c.host_choose_workspace().await, ActionResult::Ok);
    assert_eq!(
        hx.c.host_action(HostAction::ImportSdk).await,
        host_error("game data is missing: reforged-host gamedata")
    );

    // Mystic Paradox's source is in the checkout (RD1)
    let source = crate::platform::hostkit::WorkspacePaths::new(&ws).runtime_source;
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("dllmain.cpp"), b"//").unwrap();
    assert_eq!(hx.c.host_action(HostAction::ImportSdk).await, host_error("no_dump"));
    let dump = hx.user.path().join("dumps").join("Dauntless-1.14.7").join("CppSDK");
    std::fs::create_dir_all(dump.join("SDK")).unwrap();
    std::fs::write(dump.join("SDK.hpp"), b"#pragma once").unwrap();
    std::fs::write(dump.join("SDK").join("Engine_functions.cpp"), b"//").unwrap();
    assert_eq!(hx.c.host_action(HostAction::ImportSdk).await, ActionResult::Ok);
    assert!(source.join("SDK").join("Engine_functions.cpp").exists());
    assert_eq!(hx.c.snapshot().notice, Some(NoticeCode::SdkImported));

    assert_eq!(hx.c.host_set_multipliers(None).await, host_error("not_configured"));
    configure_workspace(&ws);
    std::fs::write(
        crate::platform::hostkit::WorkspacePaths::new(&ws).host_json,
        r#"{"mode":"public","name":"x"}"#,
    )
    .unwrap();
    assert_eq!(hx.c.host_set_multipliers(None).await, host_error("multipliers_public"));
    assert_eq!(
        hx.c.host_action(HostAction::CopyExports).await,
        host_error("no_exports")
    );
}

/// RD9: before every ordinary PLAY, a `mystic_loader.ini` or a Host tools DLL beside the loader is
/// refused, and a session a closed launcher left behind is undone at the next start.
#[tokio::test(flavor = "multi_thread")]
async fn play_refuses_a_leftover_ini_or_host_tools_dll_beside_the_loader() {
    use crate::platform::loader_session::{LOADER_INI, RECORD_FILE};
    let mut host = Host::new("LAN Party");
    host.registration = "OPEN";
    let (_lan, port) = lan(host).await;
    let hx = host_harness();
    let local = hx.temp();
    for (name, data) in make_runtime(3) {
        std::fs::write(local.join(name), data).unwrap();
    }
    hx.pick("runtime", &local);
    hx.c.init().await;
    let text = invite(ServerMode::Private, "127.0.0.1", port, None, "OPEN-ANYONE", "LAN Party");
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    assert!(matches!(hx.c.register("Leftover").await, RegisterOutcome::Ok { .. }));
    let game = hx.game();
    assert_eq!(hx.c.use_game_path(&game.to_string_lossy()).await, ActionResult::Ok);
    assert_eq!(hx.c.choose_local_runtime().await, ActionResult::Ok);
    let win64 = game.join("Archon").join("Binaries").join("Win64");

    std::fs::write(win64.join(LOADER_INI), "evil.dll\r\n").unwrap();
    std::fs::write(win64.join("evil.dll"), b"x").unwrap();
    let refused = ActionResult::Err(LauncherError::with_detail(
        ErrorCode::LoaderLeftovers,
        "evil.dll, mystic_loader.ini",
    ));
    assert_eq!(hx.c.play().await, refused);
    assert!(hx.spawns.lock().unwrap().is_empty(), "the game did not start");
    assert!(!hx.relay_active());
    std::fs::remove_file(win64.join(LOADER_INI)).unwrap();

    // Without an ini the loader would not load evil.dll; the host's own tools are refused all the same
    let dumper = hx.temp().join("Dumper-7.dll");
    std::fs::write(&dumper, b"dumper").unwrap();
    let text = dumper.to_string_lossy().into_owned();
    hx.c.update_settings(|map| super::stored::set_text(map, "dumperDll", Some(&text)))
        .await;
    std::fs::write(win64.join("Dumper-7.dll"), b"left over").unwrap();
    std::fs::write(win64.join("CatalogExporter.dll"), b"left over").unwrap();
    let refused = ActionResult::Err(LauncherError::with_detail(
        ErrorCode::LoaderLeftovers,
        "CatalogExporter.dll, Dumper-7.dll",
    ));
    assert_eq!(hx.c.play().await, refused);
    std::fs::remove_file(win64.join("Dumper-7.dll")).unwrap();
    std::fs::remove_file(win64.join("CatalogExporter.dll")).unwrap();

    assert_eq!(hx.c.play().await, ActionResult::Ok, "a clean folder plays");
    hx.exit(0).await;

    // A session the launcher did not finish (it was closed with the game running)
    let session = crate::platform::loader_session::begin(
        crate::platform::loader_session::SessionKind::Dump,
        &win64,
        &dumper,
        &hx.user.path().join(RECORD_FILE),
        &[],
    )
    .unwrap();
    drop(session);
    assert!(!win64.join("MysticParadox.dll").exists());
    assert_eq!(
        hx.c.play().await,
        ActionResult::Err(LauncherError::with_detail(
            ErrorCode::LoaderLeftovers,
            "Dumper-7.dll, mystic_loader.ini, MysticParadox.dll.rr-dump-off",
        ))
    );
    hx.c.init().await;
    assert!(!win64.join(LOADER_INI).exists() && !win64.join("Dumper-7.dll").exists());
    assert!(win64.join("MysticParadox.dll").exists(), "the runtime is back");
    assert_eq!(hx.c.play().await, ActionResult::Ok);
    hx.exit(0).await;
    hx.c.shutdown().await;
}

/// RD9: a Dumper-7 session places the DLL and the ini beside the loader, starts the game without the
/// runtime and without the runtime's variables, and both go when the game exits; its exit code is not a
/// failed launch.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_dumper_7_session_loads_through_the_loader_list_and_cleans_up() {
    use crate::platform::loader_session::{LOADER_INI, ini_entries};
    let hx = host_harness();
    let game = hx.game();
    let win64 = game.join("Archon").join("Binaries").join("Win64");
    for (name, data) in make_runtime(4) {
        std::fs::write(win64.join(name), data).unwrap();
    }
    hx.c.init().await;
    assert_eq!(hx.c.use_game_path(&game.to_string_lossy()).await, ActionResult::Ok);
    assert_eq!(hx.c.host_action(HostAction::InjectDumper).await, host_error("dumper"));
    let dumper = hx.temp().join("Dumper-7.dll");
    std::fs::write(&dumper, b"dumper").unwrap();
    let text = dumper.to_string_lossy().into_owned();
    hx.c.update_settings(|map| super::stored::set_text(map, "dumperDll", Some(&text)))
        .await;

    assert_eq!(hx.c.host_action(HostAction::InjectDumper).await, ActionResult::Ok);
    let (dir, args, env) = hx.spawns.lock().unwrap().last().cloned().unwrap();
    assert_eq!(dir, win64);
    assert_eq!(args, ["-NoEAC", "-EpicPortal", "-DisableEOS"]);
    assert!(env.iter().all(|(k, _)| {
        let k = k.to_string_lossy().to_ascii_uppercase();
        !k.starts_with("REFORGED_") && !k.starts_with("MYSTICPARADOX_")
    }));
    assert_eq!(env_of(&env, "PATH").len(), 1);
    let ini = std::fs::read_to_string(win64.join(LOADER_INI)).unwrap();
    assert_eq!(ini_entries(&ini), ["Dumper-7.dll"]);
    assert_eq!(std::fs::read(win64.join("Dumper-7.dll")).unwrap(), b"dumper");
    assert!(
        !win64.join("MysticParadox.dll").exists(),
        "no runtime in a dump session"
    );
    assert!(win64.join("winmm.dll").exists(), "the loader stays");
    assert_eq!(
        hx.c.host_action(HostAction::InjectDumper).await,
        host_error("game_running")
    );
    hx.c.dismiss_error();

    // Dumper-7's console ends the game with a code of its own
    hx.c.game.ended(Some(1));
    until(|| !win64.join(LOADER_INI).exists() && win64.join("MysticParadox.dll").exists()).await;
    assert!(!win64.join("Dumper-7.dll").exists());
    assert!(hx.c.loader_leftovers(&win64).is_empty());
    assert_eq!(hx.c.snapshot().last_error, None);
}

/// RD9: a CatalogExporter session starts the game as PLAY does, with the runtime, and CatalogExporter on
/// the loader's list after it.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_exporter_session_plays_with_the_exporter_on_the_loader_list() {
    use crate::platform::loader_session::{LOADER_INI, ini_entries};
    let mut host = Host::new("LAN Party");
    host.registration = "OPEN";
    let (_lan, port) = lan(host).await;
    let hx = host_harness();
    let local = hx.temp();
    for (name, data) in make_runtime(6) {
        std::fs::write(local.join(name), data).unwrap();
    }
    hx.pick("runtime", &local);
    hx.c.init().await;
    let text = invite(ServerMode::Private, "127.0.0.1", port, None, "OPEN-ANYONE", "LAN Party");
    assert_eq!(hx.c.submit_invite(&text, false, false).await, ActionResult::Ok);
    assert!(matches!(hx.c.register("Exporter").await, RegisterOutcome::Ok { .. }));
    let game = hx.game();
    assert_eq!(hx.c.use_game_path(&game.to_string_lossy()).await, ActionResult::Ok);
    assert_eq!(hx.c.choose_local_runtime().await, ActionResult::Ok);
    let win64 = game.join("Archon").join("Binaries").join("Win64");
    let ws = host_workspace(&hx.temp().join("Reforged"));
    hx.pick("workspace", &ws);
    assert_eq!(hx.c.host_choose_workspace().await, ActionResult::Ok);
    assert_eq!(
        hx.c.host_action(HostAction::InjectExporter).await,
        host_error("exporter")
    );
    let exporter = crate::platform::hostkit::WorkspacePaths::new(&ws).exporter_dll;
    std::fs::create_dir_all(exporter.parent().unwrap()).unwrap();
    std::fs::write(&exporter, b"exporter").unwrap();

    assert_eq!(hx.c.host_action(HostAction::InjectExporter).await, ActionResult::Ok);
    assert!(hx.relay_active(), "started as PLAY starts it");
    let (_, args, _) = hx.spawns.lock().unwrap().last().cloned().unwrap();
    assert_ne!(args, ["-NoEAC", "-EpicPortal", "-DisableEOS"]);
    let ini = std::fs::read_to_string(win64.join(LOADER_INI)).unwrap();
    assert_eq!(ini_entries(&ini), ["CatalogExporter.dll"]);
    assert!(win64.join("MysticParadox.dll").exists(), "the runtime loads first");
    assert!(win64.join("export_flags.txt").exists());

    hx.exit(0).await;
    until(|| !win64.join(LOADER_INI).exists()).await;
    assert!(!win64.join("CatalogExporter.dll").exists());
    assert_eq!(hx.c.play().await, ActionResult::Ok, "an ordinary PLAY afterwards");
    hx.exit(0).await;
    hx.c.shutdown().await;
}
