//! The controller's flows (`test/controller.test.ts`) against a fake host: a TLS gateway (public mode)
//! or a plain loopback server (private mode) that answers the metagame routes and `/content/*`.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;
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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::platform::{Boxed, Platform, Prepared};
use super::*;
use crate::bridge::types::GameLanguage;
use crate::core::manifest::validate_manifest;
use crate::core::pins::EXPECTED_BUILD;
use crate::net::pinned::certificate_fingerprint;
use crate::net::pinned::tests::{acceptor, certificate};

const EXE: &str = "Archon/Binaries/Win64/Fake-Game.exe";
const KEY_HEADER: &str = "x-undaunted-user-api-key";

fn pattern(n: usize, seed: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + seed * 7) & 0xff) as u8).collect()
}

fn files() -> Vec<(String, Vec<u8>)> {
    vec![
        (EXE.into(), pattern(300_000, 1)),
        (
            "Archon/Content/Paks/pakchunk0-WindowsNoEditor.pak".into(),
            pattern(900_000, 2),
        ),
        (
            "Engine/Config/BaseEngine.ini".into(),
            b"[Core.System]\r\nPaths=../../../Engine/Content\r\n".to_vec(),
        ),
        ("Version.txt".into(), EXPECTED_BUILD.as_bytes().to_vec()),
        ("Archon/Content/Movies/intro (1).bk2".into(), pattern(120_000, 3)),
        ("Engine/Binaries/Win64/empty.dat".into(), Vec::new()),
    ]
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn manifest_json() -> Value {
    let mut list: Vec<Value> = files()
        .iter()
        .map(|(path, data)| json!({ "path": path, "size": data.len(), "sha256": sha(data) }))
        .collect();
    list.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let total: u64 = files().iter().map(|(_, data)| data.len() as u64).sum();
    json!({ "build": EXPECTED_BUILD, "totalBytes": total, "files": list })
}

// ---------------------------------------------------------------- the fake host

#[derive(Default)]
struct Host {
    name: String,
    content_port: Option<u16>,
    users: HashMap<String, String>,
    valid_codes: HashSet<String>,
    registrations: Vec<(String, String)>,
    /// Every ServerStatus request: (had a key header, was a known account's key)
    status_calls: Vec<(bool, bool)>,
    requests: Vec<String>,
    device_blocked: bool,
    /// Registration without an invite code, as the offline metagame runs
    open_registration: bool,
    counter: u32,
    /// The next ServerStatus is answered (as of its arrival) only once this is notified
    hold: Option<(tokio::sync::oneshot::Sender<()>, Arc<tokio::sync::Notify>)>,
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
    let now = chrono::Utc::now();
    json!({
        "name": host.name,
        "online": true,
        "version": "1.0.0",
        "commit": "abc1234",
        "sourceUrl": "https://github.com/mixutin/dauntless-revived",
        "registration": "INVITECODE",
        "playersOnline": if full { 2 } else { 0 },
        "players": if full { json!([
            { "name": "Aurora", "where": "city", "instance": "ramsgate" },
            { "name": "Borealis", "where": "hunt", "instance": "hunt-1" },
        ]) } else { json!([]) },
        "instances": if full { json!([
            { "id": "ramsgate", "kind": "city", "title": "Ramsgate", "map": "Hub", "behemoth": null, "players": 1,
              "maxPlayers": 60, "startedAt": (now - chrono::Duration::hours(1)).to_rfc3339() },
        ]) } else { json!([]) },
        "contentPort": host.content_port,
        "uptimeSeconds": 7200,
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
    let range = request
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    host.lock().unwrap().requests.push(format!("{method} {path}"));
    let body = request
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes())
        .unwrap_or_default();
    let known = |host: &Host| key.as_ref().is_some_and(|key| host.users.contains_key(key));

    if path == "/undaunted/api/ServerStatus" {
        let (answer, hold) = {
            let mut h = host.lock().unwrap();
            let registered = known(&h);
            h.status_calls.push((key.is_some(), registered));
            (status_body(&h, registered), h.hold.take())
        };
        if let Some((arrived, gate)) = hold {
            let _ = arrived.send(());
            gate.notified().await;
        }
        return reply(200, answer);
    }
    if path == "/undaunted/api/GetUserInfo" {
        let h = host.lock().unwrap();
        return match key.as_ref().and_then(|key| h.users.get(key)) {
            Some(name) => reply(
                200,
                json!({ "UserId": format!("UID-{name}"), "Username": name, "IsAdmin": false }),
            ),
            None => reply(401, json!({})),
        };
    }
    if path == "/undaunted/api/DeviceSignals" && host.lock().unwrap().device_blocked {
        return reply(403, json!({ "error": "device_banned", "reason": "Test restriction" }));
    }
    if path == "/undaunted/api/Register" && method == hyper::Method::POST {
        let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let username = parsed["Username"].as_str().unwrap_or("").to_owned();
        let code = parsed["InviteCode"].as_str().unwrap_or("").to_owned();
        let mut h = host.lock().unwrap();
        if h.users.values().any(|u| u.eq_ignore_ascii_case(&username)) {
            return reply(409, json!({ "error": "username_taken" }));
        }
        if !h.open_registration && !h.valid_codes.remove(&code) {
            return reply(401, json!({ "error": "invite_invalid" }));
        }
        h.registrations.push((username.clone(), code));
        h.counter += 1;
        let key = format!("UUK_{:048x}", h.counter);
        h.users.insert(key.clone(), username);
        return reply(200, json!({ "UUK": key }));
    }
    if path == "/content/v1/manifest" {
        return reply(200, manifest_json());
    }
    if path == "/content/v1/news" {
        return reply(
            200,
            json!({ "items": [{ "title": "Hello", "body": "First hunt tonight" }] }),
        );
    }
    if path == "/content/v1/branding" {
        return reply(200, json!({ "backgrounds": [], "accent": null }));
    }
    if let Some(encoded) = path.strip_prefix("/content/v1/files/") {
        if !known(&host.lock().unwrap()) {
            return reply(401, Value::Null);
        }
        let relative: String = encoded
            .split('/')
            .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let Some((_, data)) = files().into_iter().find(|(p, _)| *p == relative) else {
            return reply(404, Value::Null);
        };
        let from = range
            .as_deref()
            .and_then(|r| r.strip_prefix("bytes="))
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|from| *from < data.len());
        let mut response = Response::builder()
            .header("etag", format!("\"{}\"", sha(&data)))
            .header("accept-ranges", "bytes");
        let start = from.unwrap_or(0);
        if let Some(from) = from {
            response = response.status(206).header(
                "content-range",
                format!("bytes {from}-{}/{}", data.len() - 1, data.len()),
            );
        }
        return response
            .header("content-length", data.len() - start)
            .body(Full::new(Bytes::from(data[start..].to_vec())))
            .unwrap();
    }
    if path == "/dauntless-status" {
        return reply(200, json!({ "show-status": true }));
    }
    reply(404, Value::Null)
}

/// Serves the fake host on a free loopback port, over TLS when an acceptor is given.
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

fn host(codes: &[&str]) -> Shared {
    Arc::new(Mutex::new(Host {
        name: "Friday Hunts".into(),
        valid_codes: codes.iter().map(|c| (*c).to_owned()).collect(),
        ..Host::default()
    }))
}

/// A public gateway with a fresh certificate: (host, port, fingerprint).
async fn gateway(codes: &[&str]) -> (Shared, u16, String) {
    let (cert, key) = certificate();
    let fingerprint = certificate_fingerprint(&cert);
    let shared = host(codes);
    let port = serve(shared.clone(), Some(acceptor(cert, key))).await;
    (shared, port, fingerprint)
}

fn public_invite(port: u16, fp: &str, code: &str) -> String {
    format_invite(&Invite {
        mode: ServerMode::Public,
        host: "127.0.0.1".into(),
        port,
        code: code.into(),
        name: "Friday Hunts".into(),
        share: None,
        fp: Some(fp.into()),
    })
    .unwrap()
}

// ---------------------------------------------------------------- the harness

struct Fake {
    last: Mutex<Option<Snapshot>>,
    folder: Mutex<Option<PathBuf>>,
}

impl Platform for Fake {
    fn choose_folder(&self, _: Language, _: FolderPurpose, _: PathBuf) -> Boxed<Option<PathBuf>> {
        let picked = self.folder.lock().unwrap().clone();
        Box::pin(async move { picked })
    }
    fn choose_save_file(&self, _: String) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
    fn choose_open_file(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
    fn open_url(&self, _: &str, _: Option<&str>) {}
    fn open_folder(&self, _: &Path) {}
    fn emit_snapshot(&self, snapshot: &Snapshot) {
        *self.last.lock().unwrap() = Some(snapshot.clone());
    }
    fn emit_progress(&self, _: &TaskProgress) {}
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
    fn collect_device_signals(&self) -> Boxed<Value> {
        Box::pin(async { json!({}) })
    }
    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>) {
        (Vec::new(), Vec::new())
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

/// One started stand-in game: (Win64 folder, arguments, extra environment).
type Spawned = (PathBuf, Vec<String>, Vec<(String, String)>);

struct Harness {
    c: ControllerHandle,
    fake: Arc<Fake>,
    spawns: Arc<Mutex<Vec<Spawned>>>,
    user: tempfile::TempDir,
    install: tempfile::TempDir,
    config: tempfile::TempDir,
}

impl Harness {
    fn new(relay_port: u16) -> Self {
        Self::with_user(relay_port, tempfile::tempdir().unwrap())
    }

    fn with_user(relay_port: u16, user: tempfile::TempDir) -> Self {
        Self::build(
            relay_port,
            user,
            #[cfg(feature = "offline")]
            None,
        )
    }

    fn build(
        relay_port: u16,
        user: tempfile::TempDir,
        #[cfg(feature = "offline")] offline: Option<super::offline::OfflineSetup>,
    ) -> Self {
        let install = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake {
            last: Mutex::new(None),
            folder: Mutex::new(None),
        });
        let spawns: Arc<Mutex<Vec<Spawned>>> = Arc::default();
        let log = spawns.clone();
        let manifest = validate_manifest(&manifest_json(), EXPECTED_BUILD, false).unwrap();
        let exe = files().into_iter().find(|(p, _)| p == EXE).unwrap().1;
        let config_dir = config.path().to_path_buf();
        let c = Controller::new(
            fake.clone(),
            Config {
                user_data: user.path().to_path_buf(),
                documents: None,
                resources_dir: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets")),
                default_install_dir: install.path().to_path_buf(),
                host: HostPlatform::current(),
                app_version: "0.0.0-test".into(),
                packaged: false,
                language: Language::En,
                system_locale: "en-US".into(),
                manifest: Some(manifest),
                game_config_dir: config_dir,
                relay_port,
                exe_pin: (EXE.into(), sha(&exe)),
                status_poll: Duration::from_secs(60),
                encryptor: Arc::new(Plain),
                legacy_encryptor: None,
                #[cfg(feature = "offline")]
                offline,
                // Requests the test sends stand in for the game, so the game is this process
                spawn: Some(Arc::new(move |dir, args, _, env| {
                    log.lock().unwrap().push((
                        dir.to_path_buf(),
                        args.to_vec(),
                        env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
                    ));
                    Ok(std::process::id())
                })),
            },
        );
        Self {
            c,
            fake,
            spawns,
            user,
            install,
            config,
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

    /// The stand-in game exits.
    async fn exit(&self, code: i32) {
        self.c.game.ended(Some(code));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn ok() -> ActionResult {
    ActionResult::Ok
}

fn failed(code: ErrorCode) -> ActionResult {
    ActionResult::err(code)
}

fn code_of(outcome: &ActionResult) -> Option<ErrorCode> {
    match outcome {
        ActionResult::Ok => None,
        ActionResult::Err(error) => Some(error.code),
    }
}

fn registered(name: &str) -> RegisterOutcome {
    RegisterOutcome::Ok { username: name.into() }
}

/// A plain GET through the relay, as the game would send it: (status, body).
async fn get(port: u16, path: &str) -> std::io::Result<(u16, String)> {
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    tcp.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
        .await?;
    let mut text = String::new();
    tcp.read_to_string(&mut text).await?;
    let status = text.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    Ok((status, body))
}

async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn players(snapshot: &Snapshot) -> Vec<String> {
    snapshot
        .status
        .as_ref()
        .map(|status| status.players.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- the flows

#[tokio::test(flavor = "multi_thread")]
async fn public_mode_end_to_end_join_register_install_play_through_the_relay_game_exit() {
    let (meta, port, fp) = gateway(&["ABCD-EFGH-JKLM"]).await;
    let relay_port = free_port().await;
    let hx = Harness::new(relay_port);
    hx.c.init().await;
    assert_eq!(hx.last().phase, Phase::Join);

    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "ABCD-EFGH-JKLM"), false)
            .await,
        ok()
    );
    let s = hx.last();
    assert_eq!(s.server.as_ref().map(|sv| sv.mode), Some(ServerMode::Public));
    assert_eq!(
        s.server.as_ref().and_then(|sv| sv.fingerprint.clone()),
        Some(fp.clone())
    );
    assert_eq!(s.connect.problem, None);
    assert_eq!(s.phase, Phase::Register);
    assert!(s.install.content_available, "content found behind the gateway");
    let status = s.status.clone().unwrap();
    assert_eq!(status.name, "Friday Hunts");
    assert!(status.limited, "not registered yet: the server hides who is online");
    assert_eq!((status.players_online, status.players.len()), (0, 0));

    assert_eq!(hx.c.register("Slayer_42").await, registered("Slayer_42"));
    let s = hx.last();
    assert_eq!(s.phase, Phase::Install);
    assert_eq!(s.account.username.as_deref(), Some("Slayer_42"));
    assert!(
        !s.status.as_ref().unwrap().limited,
        "registered: the status is asked again with the key"
    );
    assert_eq!(players(&s), ["Aurora", "Borealis"]);
    assert_eq!(meta.lock().unwrap().registrations.last().unwrap().1, "ABCD-EFGH-JKLM");

    assert_eq!(hx.c.start_install().await, ok());
    let s = hx.last();
    assert_eq!(s.phase, Phase::Ready);
    assert_eq!(s.notice, Some(NoticeCode::InstallDone));
    for (path, data) in files() {
        let on_disk = std::fs::read(path.split('/').fold(hx.install.path().to_path_buf(), |p, s| p.join(s))).unwrap();
        assert!(on_disk == data, "{path}");
    }
    let win64 = hx.install.path().join("Archon").join("Binaries").join("Win64");
    for dll in ["dxgi.dll", "UndauntedInternalServer.dll"] {
        assert!(win64.join(dll).exists(), "{dll}");
    }
    assert!(
        meta.lock()
            .unwrap()
            .requests
            .iter()
            .any(|r| r.starts_with("GET /content/v1/files/")),
        "files came through the gateway"
    );

    meta.lock().unwrap().device_blocked = true;
    let denied = hx.c.play().await;
    assert_eq!(code_of(&denied), Some(ErrorCode::AccountBanned));
    assert!(hx.spawns.lock().unwrap().is_empty());
    assert_eq!(hx.c.keep_open(), None);
    meta.lock().unwrap().device_blocked = false;

    assert_eq!(hx.c.play().await, ok());
    let s = hx.last();
    assert_eq!(s.phase, Phase::Running);
    assert_eq!(s.game.relay_port, Some(relay_port));
    assert_eq!(hx.c.keep_open(), Some(KeepOpen::Relay));

    // Launch line: the relay first, the key second
    let (dir, args, _) = hx.spawns.lock().unwrap()[0].clone();
    assert_eq!(dir, win64);
    assert_eq!(args[0], format!("127.0.0.1:{relay_port}"));
    let key = args[1].strip_prefix("-AUTH_PASSWORD=").unwrap().to_owned();
    assert!(key.starts_with("UUK_") && key.len() == 52, "{key}");
    assert_eq!(
        crate::platform::log::redact(&key),
        "<hidden>",
        "the key is masked in the log"
    );

    // Chat goes to the relay too
    let ini = std::fs::read(hx.config.path().join("Engine.ini")).unwrap();
    let ini = String::from_utf8_lossy(&ini);
    assert!(
        ini.contains(&format!(
            "ServerAddr=\"ws://127.0.0.1\"\r\nServerPort={relay_port}\r\nbUseSSL=false"
        )),
        "{ini}"
    );

    // The game's own traffic crosses the relay to the gateway
    let (status, body) = get(relay_port, "/undaunted/api/ServerStatus").await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Friday Hunts"), "{body}");

    // The game exits: the relay goes away with it
    hx.exit(0).await;
    assert_eq!(hx.c.keep_open(), None);
    assert_eq!(hx.last().phase, Phase::Ready);
    assert!(get(relay_port, "/undaunted/api/ServerStatus").await.is_err());
    assert_eq!(hx.last().last_error, None);

    assert_eq!(hx.c.play().await, ok());
    hx.exit(-1073741515).await;
    let error = hx.last().last_error.unwrap();
    assert_eq!(error.code, ErrorCode::LaunchFailed);
    assert!(error.detail.unwrap().contains("0xC0000135"));
    assert_eq!(hx.c.keep_open(), None);

    let user = meta.lock().unwrap().users.remove(&key).unwrap();
    assert_eq!(hx.c.play().await, failed(ErrorCode::KeyRejected));
    assert_eq!(
        hx.spawns.lock().unwrap().len(),
        2,
        "a rejected key never starts a game process"
    );
    meta.lock().unwrap().users.insert(key, user);
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn game_language_log_window_and_exposure_reach_the_game_at_play() {
    let (_meta, port, fp) = gateway(&["SETTINGS-CODE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "SETTINGS-CODE"), false)
            .await,
        ok()
    );
    assert_eq!(hx.c.register("Settings_1").await, registered("Settings_1"));
    assert_eq!(hx.c.start_install().await, ok());
    let s = hx.last().settings;
    assert_eq!(
        (s.game_language, s.show_console, s.exposure),
        (GameLanguage::Auto, false, ExposureMode::Game)
    );

    let patch = |value: Value| serde_json::from_value::<SettingsPatch>(value).unwrap_or_default();
    let s =
        hx.c.set_settings(patch(
            json!({ "gameLanguage": "es-ES", "showConsole": true, "exposure": "basic" }),
        ))
        .await;
    assert_eq!(
        (s.game_language, s.show_console, s.exposure),
        (GameLanguage::EsEs, true, ExposureMode::Basic)
    );
    let s = hx.c.set_settings(patch(json!({ "gameLanguage": "xx-XX" }))).await;
    assert_eq!(s.game_language, GameLanguage::EsEs, "an unknown value changes nothing");
    let file: Value = serde_json::from_slice(&std::fs::read(hx.user.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        (file["gameLanguage"].as_str(), file["showConsole"].as_bool()),
        (Some("es-ES"), Some(true))
    );

    assert_eq!(hx.c.play().await, ok());
    let (_, args, env) = hx.spawns.lock().unwrap().last().unwrap().clone();
    assert!(args.iter().any(|a| a == "-epiclocale=es-ES"), "{args:?}");
    assert_eq!(env, [("DR_SHOW_CONSOLE".to_owned(), "1".to_owned())]);
    hx.exit(0).await;
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_servers_come_back_with_one_click_keeping_their_account() {
    let (_meta, port, fp) = gateway(&["SAVED-CODE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    assert!(hx.c.snapshot().saved_servers.is_empty());
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "SAVED-CODE"), false).await,
        ok()
    );
    assert_eq!(hx.c.register("Saved_1").await, registered("Saved_1"));
    let entry = hx.c.snapshot().saved_servers[0].clone();
    assert_eq!(
        (entry.name.as_str(), entry.current, entry.username.as_deref()),
        ("Friday Hunts", true, Some("Saved_1"))
    );
    assert_eq!(
        hx.c.remove_saved_server(&entry.id).await,
        failed(ErrorCode::Busy),
        "the current server stays"
    );

    assert_eq!(hx.c.forget_server().await, ok());
    let s = hx.c.snapshot();
    assert!(s.server.is_none());
    assert_eq!(
        s.saved_servers
            .iter()
            .map(|sv| (sv.id.clone(), sv.current))
            .collect::<Vec<_>>(),
        [(entry.id.clone(), false)]
    );

    assert_eq!(
        code_of(&hx.c.switch_server(&"0".repeat(24)).await),
        Some(ErrorCode::InviteInvalidFormat)
    );
    assert_eq!(hx.c.switch_server(&entry.id).await, ok());
    let back = hx.c.snapshot();
    let server = back.server.unwrap();
    assert_eq!(
        (
            server.host.as_str(),
            server.fingerprint,
            back.account.has_key,
            server.has_pending_invite
        ),
        ("127.0.0.1", Some(fp), true, false)
    );

    assert_eq!(hx.c.forget_server().await, ok());
    assert_eq!(hx.c.remove_saved_server(&entry.id).await, ok());
    assert!(hx.c.snapshot().saved_servers.is_empty());
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_that_does_not_match_the_invite_stops_everything_before_any_request() {
    let (meta, port, _) = gateway(&["CODE-ONE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    let other = certificate_fingerprint(&certificate().0);
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &other, "CODE-ONE"), false)
            .await,
        ok()
    );
    let s = hx.last();
    assert_eq!(s.phase, Phase::Connect);
    assert_eq!(s.connect.problem, Some(ConnectProblem::CertMismatch));
    assert!(
        meta.lock().unwrap().requests.is_empty(),
        "nothing was sent to the wrong certificate"
    );
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_relay_port_is_reported_and_the_game_is_not_started() {
    let (_meta, port, fp) = gateway(&["BUSY-CODE"]).await;
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let hx = Harness::new(busy_port);
    hx.c.init().await;
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "BUSY-CODE"), false).await,
        ok()
    );
    assert_eq!(hx.c.register("Busy_1").await, registered("Busy_1"));
    assert_eq!(hx.c.start_install().await, ok());
    let outcome = hx.c.play().await;
    assert_eq!(code_of(&outcome), Some(ErrorCode::RelayPortBusy));
    assert!(hx.spawns.lock().unwrap().is_empty());
    assert_eq!(hx.c.keep_open(), None);
    drop(busy);
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_certificate_for_the_same_server_never_gets_the_stored_key_before_the_user_confirms() {
    let (first, port, fp) = gateway(&["CERT-ONE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "CERT-ONE"), false).await,
        ok()
    );
    assert_eq!(hx.c.register("Cert_1").await, registered("Cert_1"));
    let key = first.lock().unwrap().users.keys().next().unwrap().clone();

    // The same host:port, now behind another certificate (a reinstalled gateway, or someone else)
    let (cert, private) = certificate();
    let new_fp = certificate_fingerprint(&cert);
    let invite = public_invite(port, &new_fp, "CERT-TWO");
    match hx.c.check_invite(&invite) {
        InviteCheck::Ok {
            certificate_changed,
            previous_fingerprint,
        } => assert_eq!((certificate_changed, previous_fingerprint), (true, Some(fp.clone()))),
        InviteCheck::Err(error) => panic!("{error:?}"),
    }
    let before = first.lock().unwrap().requests.len();
    let refused = hx.c.submit_invite(&invite, false).await;
    assert_eq!(
        refused,
        ActionResult::Err(LauncherError {
            code: ErrorCode::CertChanged,
            detail: Some(fp.clone())
        })
    );
    // Only the background news and art loads that registering started may still arrive
    let after: Vec<String> = first.lock().unwrap().requests[before..].to_vec();
    assert!(
        after.iter().all(|r| r.starts_with("GET /content/v1/")),
        "nothing was sent for the refused invite: {after:?}"
    );
    assert_eq!(
        hx.c.snapshot().server.unwrap().fingerprint,
        Some(fp.clone()),
        "nothing was changed"
    );

    // Confirmed: the key moves to the new certificate and goes to the new gateway
    let second = host(&[]);
    second.lock().unwrap().users.insert(key.clone(), "Cert_1".into());
    let _ = (second, cert, private);
    assert_eq!(hx.c.submit_invite(&invite, true).await, ok());
    let s = hx.c.snapshot();
    assert_eq!(s.server.unwrap().fingerprint, Some(new_fp.clone()));
    // The old gateway still has the old certificate, so the launcher only reports the mismatch
    assert_eq!(s.connect.problem, Some(ConnectProblem::CertMismatch));
    let slot = StoredServer::from_invite(&parse_invite(&invite).unwrap(), None).slot();
    assert_eq!(
        hx.c.keys.load(&slot).unwrap().as_deref(),
        Some(key.as_str()),
        "the key moved to the new certificate"
    );
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_status_carries_the_key_only_for_a_registered_player_and_never_a_refused_one() {
    let (meta, port, fp) = gateway(&["STATUS-CODE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "STATUS-CODE"), false)
            .await,
        ok()
    );
    assert_eq!(
        meta.lock().unwrap().status_calls.last(),
        Some(&(false, false)),
        "no key before registering"
    );
    assert_eq!(hx.c.register("Status_1").await, registered("Status_1"));
    hx.c.poll_status().await;
    assert_eq!(meta.lock().unwrap().status_calls.last(), Some(&(true, true)));
    assert_eq!(players(&hx.last()), ["Aurora", "Borealis"]);

    // The server stops accepting the key: it is not sent with ServerStatus again
    meta.lock().unwrap().users.clear();
    assert_eq!(hx.c.connect().await, ok());
    assert_eq!(hx.last().phase, Phase::Register);
    let calls = meta.lock().unwrap().status_calls.len();
    hx.c.poll_status().await;
    let last = {
        let meta = meta.lock().unwrap();
        assert_eq!(meta.status_calls.len(), calls + 1);
        meta.status_calls.last().copied()
    };
    assert_eq!(last, Some((false, false)));
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_status_request_on_its_way_when_the_player_logs_out_never_brings_the_list_back() {
    let (meta, port, fp) = gateway(&["LOGOUT-CODE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "LOGOUT-CODE"), false)
            .await,
        ok()
    );
    assert_eq!(hx.c.register("Logout_1").await, registered("Logout_1"));
    assert_eq!(players(&hx.last()), ["Aurora", "Borealis"]);

    let (arrived, arrival) = tokio::sync::oneshot::channel();
    let gate = Arc::new(tokio::sync::Notify::new());
    meta.lock().unwrap().hold = Some((arrived, gate.clone()));
    let poll = {
        let c = hx.c.clone();
        tokio::spawn(async move { c.poll_status().await })
    };
    arrival.await.unwrap();
    assert_eq!(hx.c.logout().await, ok());
    assert!(players(&hx.last()).is_empty(), "the list goes at once");
    gate.notify_one();
    poll.await.unwrap();
    let s = hx.last();
    assert!(
        players(&s).is_empty(),
        "the answer asked for with the old key is dropped"
    );
    assert!(!s.account.has_key);
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn private_mode_sends_the_key_with_server_status_over_plain_http_to_the_invite_host_only() {
    let meta = host(&["PRIV-CODE"]);
    let port = serve(meta.clone(), None).await;
    meta.lock().unwrap().content_port = Some(port);
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    let invite = format_invite(&Invite {
        mode: ServerMode::Private,
        host: "127.0.0.1".into(),
        port,
        code: "PRIV-CODE".into(),
        name: "Friday Hunts".into(),
        share: None,
        fp: None,
    })
    .unwrap();
    assert_eq!(hx.c.submit_invite(&invite, false).await, ok());
    assert_eq!(hx.last().phase, Phase::Register);
    assert!(
        hx.last().install.content_available,
        "the content port comes from ServerStatus"
    );
    assert_eq!(hx.c.register("Private_1").await, registered("Private_1"));
    assert_eq!(meta.lock().unwrap().status_calls.last(), Some(&(true, true)));
    assert_eq!(players(&hx.last()), ["Aurora", "Borealis"]);

    // News load in the background once the content server is known
    for _ in 0..50 {
        if !hx.c.news().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(hx.c.news().first().map(|n| n.title.as_str()), Some("Hello"));
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn change_folder_on_an_existing_game_uses_that_game_and_anything_else_gets_a_subfolder() {
    let (_meta, port, fp) = gateway(&["FOLDER-CODE"]).await;
    let hx = Harness::new(free_port().await);
    hx.c.init().await;
    assert_eq!(
        hx.c.submit_invite(&public_invite(port, &fp, "FOLDER-CODE"), false)
            .await,
        ok()
    );

    assert_eq!(hx.c.choose_install_dir().await, failed(ErrorCode::Cancelled));

    let busy = tempfile::tempdir().unwrap();
    std::fs::write(busy.path().join("notes.txt"), "x").unwrap();
    *hx.fake.folder.lock().unwrap() = Some(busy.path().to_path_buf());
    assert_eq!(hx.c.choose_install_dir().await, ok());
    let dir = hx.c.snapshot().install.dir;
    assert!(dir.ends_with("DauntlessRevived"), "{dir}");
    assert!(dir.starts_with(&busy.path().to_string_lossy().into_owned()), "{dir}");

    let empty = tempfile::tempdir().unwrap();
    *hx.fake.folder.lock().unwrap() = Some(empty.path().to_path_buf());
    assert_eq!(hx.c.choose_install_dir().await, ok());
    assert_eq!(hx.c.snapshot().install.dir, empty.path().to_string_lossy());
    hx.c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_polling_starts_and_stops_from_a_thread_outside_the_runtime() {
    // Tauri runs sync commands such as set_status_polling on the main thread, outside any runtime
    let meta = host(&[]);
    let port = serve(meta.clone(), None).await;
    let hx = Harness::new(free_port().await);
    let invite = format_invite(&Invite {
        mode: ServerMode::Private,
        host: "127.0.0.1".into(),
        port,
        code: "POLL-CODE".into(),
        name: "Friday Hunts".into(),
        share: None,
        fp: None,
    })
    .unwrap();
    assert_eq!(hx.c.submit_invite(&invite, false).await, ok());
    let calls = meta.lock().unwrap().status_calls.len();
    let c = hx.c.clone();
    std::thread::spawn(move || c.set_status_polling(true)).join().unwrap();
    for _ in 0..50 {
        if meta.lock().unwrap().status_calls.len() > calls {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        meta.lock().unwrap().status_calls.len() > calls,
        "the first poll is at once"
    );
    let c = hx.c.clone();
    std::thread::spawn(move || c.set_status_polling(false)).join().unwrap();
    hx.c.shutdown().await;
}

#[cfg(feature = "offline")]
mod offline_play;
