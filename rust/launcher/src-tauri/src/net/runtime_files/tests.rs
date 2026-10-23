//! Reforged's `test/runtime.test.ts` cases, and the whole download end to end against a local content
//! server: a tampered file, a changed set that needs accepting again, and a partial download.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::*;

const KEY_HEADER: &str = "x-reforged-user-api-key";

fn key() -> String {
    format!("RRK_{}", "a".repeat(48))
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `fakes.ts`'s `makeRuntime`: two files whose bytes depend on the seed.
fn make_runtime(seed: u32) -> Vec<(&'static str, Vec<u8>)> {
    let pattern = |length: u32, seed: u32| -> Vec<u8> {
        (0..length)
            .map(|i| (i.wrapping_mul(31).wrapping_add(seed.wrapping_mul(7)) % 251) as u8)
            .collect()
    };
    vec![
        ("MysticParadox.dll", pattern(200_000 + seed, seed)),
        ("winmm.dll", pattern(40_000 + seed, seed + 50)),
    ]
}

fn manifest_json(files: &[(&'static str, Vec<u8>)]) -> Value {
    let mut listed: Vec<Value> = files
        .iter()
        .map(|(name, data)| json!({ "name": name, "size": data.len(), "sha256": sha256_hex(data) }))
        .collect();
    listed.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({ "version": "20261014-abcdef12", "build": { "name": "rel-1.14.7-Archon", "changelist": 647472 }, "files": listed })
}

fn manifest_for(files: &[(&'static str, Vec<u8>)]) -> RuntimeManifest {
    parse_runtime_manifest(&manifest_json(files)).unwrap()
}

#[derive(Default)]
struct ServerState {
    runtime: Option<Vec<(&'static str, Vec<u8>)>>,
    /// A manifest that does not match the files
    manifest_override: Option<Value>,
    /// Serve flipped bytes
    corrupt: bool,
    /// Serve the first half, then cut the connection
    partial: bool,
    /// Serve one byte more than the file has
    oversize: bool,
    /// (path, key header) of every request
    seen: Vec<(String, Option<String>)>,
}

/// `fakes.ts`'s `FakeServer`, the runtime routes only.
#[derive(Clone)]
struct FakeServer {
    state: Arc<Mutex<ServerState>>,
    endpoint: Endpoint,
}

impl FakeServer {
    async fn start(runtime: Option<Vec<(&'static str, Vec<u8>)>>) -> Self {
        let state = Arc::new(Mutex::new(ServerState {
            runtime,
            ..ServerState::default()
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = state.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let state = shared.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let state = state.clone();
                        async move { Ok::<_, Infallible>(answer(&state, &request)) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tcp), service)
                        .await;
                });
            }
        });
        Self {
            state,
            endpoint: Endpoint {
                host: "127.0.0.1".into(),
                port,
                pin: None,
            },
        }
    }

    fn with<R>(&self, change: impl FnOnce(&mut ServerState) -> R) -> R {
        change(&mut self.state.lock().unwrap())
    }

    fn downloads(&self) -> Vec<(String, Option<String>)> {
        self.with(|state| {
            state
                .seen
                .iter()
                .filter(|(path, _)| path.starts_with("/content/v1/runtime/files/"))
                .cloned()
                .collect()
        })
    }
}

type Body = BoxBody<Bytes, std::io::Error>;

fn full(status: u16, body: impl Into<Bytes>) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Full::new(body.into()).map_err(|never| match never {}).boxed())
        .unwrap()
}

fn answer(state: &Mutex<ServerState>, request: &Request<hyper::body::Incoming>) -> Response<Body> {
    let mut state = state.lock().unwrap();
    let path = request.uri().path().to_owned();
    let key_header = request
        .headers()
        .get(KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    state.seen.push((path.clone(), key_header.clone()));
    if path == RUNTIME_MANIFEST_PATH {
        if let Some(manifest) = &state.manifest_override {
            return full(200, manifest.to_string());
        }
        return match &state.runtime {
            Some(files) => full(200, manifest_json(files).to_string()),
            None => full(404, r#"{"error":"no_runtime"}"#),
        };
    }
    let Some(name) = path.strip_prefix("/content/v1/runtime/files/") else {
        return full(404, "");
    };
    if key_header.as_deref() != Some(key().as_str()) {
        return full(401, r#"{"error":"unauthorized"}"#);
    }
    let name = percent_encoding::percent_decode_str(name)
        .decode_utf8_lossy()
        .into_owned();
    let Some((_, data)) = state.runtime.iter().flatten().find(|(file, _)| *file == name) else {
        return full(404, "{}");
    };
    let mut data = data.clone();
    if state.corrupt {
        data[0] ^= 0xff;
    }
    if state.oversize {
        data.push(0);
    }
    if state.partial {
        let length = data.len();
        let half = Bytes::from(data[..length / 2].to_vec());
        let frames = futures_util::stream::iter(vec![
            Ok(Frame::data(half)),
            Err(std::io::Error::other("connection cut")),
        ]);
        return Response::builder()
            .header("content-length", length.to_string())
            .body(StreamBody::new(frames).boxed())
            .unwrap();
    }
    full(200, data)
}

fn headers() -> Vec<(&'static str, String)> {
    vec![(KEY_HEADER, key())]
}

async fn install(
    server: &FakeServer,
    manifest: &RuntimeManifest,
    win64: &Path,
) -> Result<RuntimeInstallResult, RuntimeError> {
    install_runtime(InstallRequest {
        endpoint: &server.endpoint,
        headers: headers(),
        manifest,
        win64_dir: win64,
        cancel: None,
        on_progress: None,
    })
    .await
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn leftovers(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(DOWNLOAD_SUFFIX))
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------- runtime.test.ts

#[test]
fn runtime_set_id_names_the_exact_set_of_files_in_any_order() {
    let manifest = manifest_for(&make_runtime(1));
    let mut reversed = manifest.files.clone();
    reversed.reverse();
    assert_eq!(runtime_set_id(&manifest.files), runtime_set_id(&reversed));
    assert_ne!(
        runtime_set_id(&manifest.files),
        runtime_set_id(&manifest_for(&make_runtime(2)).files)
    );
    let id = runtime_set_id(&manifest.files);
    assert!(id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
}

#[test]
fn runtime_set_id_is_reforgeds_byte_for_byte() {
    let file = |name: &str, size: u64, hash: char| RuntimeFile {
        name: name.into(),
        size,
        sha256: hash.to_string().repeat(64),
    };
    let files = [file("winmm.dll", 40_001, 'b'), file("MysticParadox.dll", 200_001, 'a')];
    let lines = format!(
        "MysticParadox.dll:200001:{}\nwinmm.dll:40001:{}",
        "a".repeat(64),
        "b".repeat(64)
    );
    assert_eq!(runtime_set_id(&files), sha256_hex(lines.as_bytes()));
    // createHash("sha256").update("MysticParadox.dll:1:" + "a".repeat(64) + "\nwinmm.dll:2:" + "b".repeat(64))
    let small = [file("MysticParadox.dll", 1, 'a'), file("winmm.dll", 2, 'b')];
    assert_eq!(
        runtime_set_id(&small),
        sha256_hex(format!("MysticParadox.dll:1:{}\nwinmm.dll:2:{}", "a".repeat(64), "b".repeat(64)).as_bytes())
    );
}

#[test]
fn a_set_is_accepted_once_per_server_and_a_changed_set_is_not() {
    let first = manifest_for(&make_runtime(1));
    let changed = manifest_for(&make_runtime(3));
    let mut trusted = Vec::new();
    assert!(!is_trusted(&trusted, &first.files));
    let id = accept(&mut trusted, &first.files);
    accept(&mut trusted, &first.files);
    assert_eq!(trusted, [id]);
    assert!(is_trusted(&trusted, &first.files));
    assert!(!is_trusted(&trusted, &changed.files), "a changed runtime asks again");
}

#[tokio::test]
async fn fetch_runtime_manifest_ok_none_and_bad_answer() {
    let server = FakeServer::start(Some(make_runtime(1))).await;
    assert_eq!(
        fetch_runtime_manifest(&server.endpoint).await,
        RuntimeManifestResult::Ok(manifest_for(&make_runtime(1)))
    );
    assert!(
        server.with(|state| state.seen.iter().all(|(_, key)| key.is_none())),
        "no key header"
    );
    let empty = FakeServer::start(None).await;
    assert_eq!(
        fetch_runtime_manifest(&empty.endpoint).await,
        RuntimeManifestResult::None
    );
    let junk = FakeServer::start(None).await;
    junk.with(|state| {
        state.manifest_override =
            Some(json!({ "version": "x", "files": [{ "name": "evil.exe", "size": 1, "sha256": "0".repeat(64) }] }));
    });
    assert_eq!(
        fetch_runtime_manifest(&junk.endpoint).await,
        RuntimeManifestResult::BadAnswer
    );
    let nobody = Endpoint {
        port: 1,
        ..server.endpoint.clone()
    };
    assert_eq!(
        fetch_runtime_manifest(&nobody).await,
        RuntimeManifestResult::Unreachable
    );
}

#[tokio::test]
async fn check_runtime_untrusted_missing_outdated_ok_unavailable_and_local() {
    let win64 = temp();
    let win64 = win64.path();
    let runtime = make_runtime(1);
    let manifest = manifest_for(&runtime);
    assert_eq!(
        check_runtime(win64, Some(&manifest), false, None).await,
        (RuntimeState::Untrusted, false)
    );
    assert_eq!(
        check_runtime(win64, Some(&manifest), true, None).await,
        (RuntimeState::Missing, false)
    );
    assert_eq!(
        check_runtime(win64, None, false, None).await,
        (RuntimeState::Unavailable, false)
    );
    for (name, data) in &runtime {
        std::fs::write(win64.join(name), data).unwrap();
    }
    assert_eq!(
        check_runtime(win64, Some(&manifest), false, None).await,
        (RuntimeState::Ok, true),
        "the exact files are already there"
    );
    assert_eq!(
        check_runtime(win64, None, false, None).await,
        (RuntimeState::Ok, true),
        "one runtime build serves every server"
    );
    let newer = manifest_for(&make_runtime(3));
    assert_eq!(
        check_runtime(win64, Some(&newer), false, None).await,
        (RuntimeState::Untrusted, false),
        "a changed runtime asks again"
    );
    assert_eq!(
        check_runtime(win64, Some(&newer), true, None).await,
        (RuntimeState::Outdated, false)
    );
    let local = temp();
    assert_eq!(
        check_runtime(win64, Some(&manifest), true, Some(local.path())).await,
        (RuntimeState::Local, false)
    );
    for (name, data) in make_runtime(4) {
        std::fs::write(local.path().join(name), data).unwrap();
    }
    assert_eq!(
        check_runtime(win64, Some(&manifest), true, Some(local.path())).await,
        (RuntimeState::Local, true)
    );
}

#[tokio::test]
async fn install_downloads_with_the_key_checks_every_file_writes_the_marker_and_skips_files_already_right() {
    let runtime = make_runtime(1);
    let server = FakeServer::start(Some(runtime.clone())).await;
    let manifest = manifest_for(&runtime);
    let win64 = temp();
    let progress = Mutex::new(Vec::new());
    let on_progress = |p: RuntimeProgress| progress.lock().unwrap().push(p);
    install_runtime(InstallRequest {
        endpoint: &server.endpoint,
        headers: headers(),
        manifest: &manifest,
        win64_dir: win64.path(),
        cancel: None,
        on_progress: Some(&on_progress),
    })
    .await
    .unwrap();
    for (name, data) in &runtime {
        assert_eq!(std::fs::read(win64.path().join(name)).unwrap(), *data);
    }
    let marker: Value =
        serde_json::from_str(&std::fs::read_to_string(win64.path().join(RUNTIME_MARKER_FILE)).unwrap()).unwrap();
    assert_eq!(marker["version"], "20261014-abcdef12");
    assert_eq!(marker["files"], manifest_json(&runtime)["files"]);
    assert!(leftovers(win64.path()).is_empty());
    let progress = progress.into_inner().unwrap();
    let last = progress.last().unwrap();
    assert_eq!((last.files_done, last.done_bytes), (2, last.total_bytes));
    assert!(progress.windows(2).all(|pair| pair[0].done_bytes <= pair[1].done_bytes));
    let downloads = server.downloads();
    assert_eq!(downloads.len(), 2);
    assert!(
        downloads
            .iter()
            .all(|(_, sent)| sent.as_deref() == Some(key().as_str()))
    );
    server.with(|state| state.seen.clear());
    install(&server, &manifest, win64.path()).await.unwrap();
    assert!(
        server.downloads().is_empty(),
        "nothing downloaded when the files already match"
    );
}

#[tokio::test]
async fn a_file_that_does_not_match_the_list_never_replaces_the_old_one() {
    let runtime = make_runtime(1);
    let server = FakeServer::start(Some(runtime.clone())).await;
    server.with(|state| state.corrupt = true);
    let win64 = temp();
    std::fs::write(win64.path().join("MysticParadox.dll"), "old build").unwrap();
    let error = install(&server, &manifest_for(&runtime), win64.path())
        .await
        .unwrap_err();
    assert_eq!(error.code, RuntimeErrorCode::Different);
    assert_eq!(error.file.as_deref(), Some("MysticParadox.dll"));
    assert_eq!(
        std::fs::read_to_string(win64.path().join("MysticParadox.dll")).unwrap(),
        "old build"
    );
    assert!(leftovers(win64.path()).is_empty());
}

#[tokio::test]
async fn an_unknown_key_is_refused() {
    let runtime = make_runtime(1);
    let server = FakeServer::start(Some(runtime.clone())).await;
    let win64 = temp();
    let error = install_runtime(InstallRequest {
        endpoint: &server.endpoint,
        headers: vec![(KEY_HEADER, format!("RRK_{}", "9".repeat(48)))],
        manifest: &manifest_for(&runtime),
        win64_dir: win64.path(),
        cancel: None,
        on_progress: None,
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, RuntimeErrorCode::KeyRejected);
}

#[tokio::test]
async fn install_local_runtime_copies_a_hosts_own_build_next_to_the_game() {
    let local = temp();
    let win64 = temp();
    assert!(!is_complete_runtime_dir(local.path()).await);
    let files = make_runtime(7);
    std::fs::write(local.path().join(files[0].0), &files[0].1).unwrap();
    std::fs::write(local.path().join(files[1].0), b"").unwrap();
    assert!(
        !is_complete_runtime_dir(local.path()).await,
        "an empty file is no runtime"
    );
    for (name, data) in &files {
        std::fs::write(local.path().join(name), data).unwrap();
    }
    assert!(is_complete_runtime_dir(local.path()).await);
    let result = install_local_runtime(local.path(), win64.path()).await.unwrap();
    assert!(result.replaced_in_use.is_empty());
    for (name, data) in &files {
        assert_eq!(std::fs::read(win64.path().join(name)).unwrap(), *data);
    }
    assert!(leftovers(win64.path()).is_empty());
}

/// Windows: a DLL a running process has loaded can be renamed but not overwritten or deleted. These ops
/// act like that for the files in `loaded`.
struct WindowsLike {
    loaded: Mutex<Vec<PathBuf>>,
}

impl ReplaceOps for WindowsLike {
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut loaded = self.loaded.lock().unwrap();
        if loaded.iter().any(|file| file == to) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        std::fs::rename(from, to)?;
        if let Some(file) = loaded.iter_mut().find(|file| *file == from) {
            *file = to.to_path_buf();
        }
        Ok(())
    }

    fn remove(&self, file: &Path) -> io::Result<()> {
        if self.loaded.lock().unwrap().iter().any(|held| held == file) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        std::fs::remove_file(file)
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_loaded_runtime_file_is_moved_aside_and_the_new_one_takes_its_place() {
    let dir = temp();
    let target = dir.path().join("MysticParadox.dll");
    let tmp = with_suffix(&target, DOWNLOAD_SUFFIX);
    std::fs::write(&target, "old build").unwrap();
    std::fs::write(&tmp, "new build").unwrap();
    let ops = WindowsLike {
        loaded: Mutex::new(vec![target.clone()]),
    };
    assert_eq!(replace_file(&tmp, &target, &ops).unwrap(), Replaced::InUse);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new build");
    let aside: Vec<String> = names(dir.path())
        .into_iter()
        .filter(|name| name.starts_with("MysticParadox.dll.rr-old-"))
        .collect();
    assert_eq!(aside.len(), 1, "the old file stays while it is loaded");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(&aside[0])).unwrap(),
        "old build"
    );
    sweep_aside(dir.path());
    assert_eq!(names(dir.path()), ["MysticParadox.dll"], "swept once nothing holds it");
}

#[test]
fn a_free_file_is_replaced_in_place_and_one_that_cannot_be_moved_aside_is_kept() {
    struct Locked;
    impl ReplaceOps for Locked {
        fn rename(&self, _: &Path, _: &Path) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::ResourceBusy))
        }
        fn remove(&self, file: &Path) -> io::Result<()> {
            std::fs::remove_file(file)
        }
    }
    let dir = temp();
    let target = dir.path().join("winmm.dll");
    let tmp = with_suffix(&target, DOWNLOAD_SUFFIX);
    std::fs::write(&target, "old").unwrap();
    std::fs::write(&tmp, "new").unwrap();
    assert_eq!(replace_file(&tmp, &target, &DiskOps).unwrap(), Replaced::Replaced);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    std::fs::write(&tmp, "newer").unwrap();
    let error = replace_file(&tmp, &target, &Locked).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
}

#[test]
fn when_the_new_file_cannot_take_the_name_the_old_one_is_put_back() {
    /// Moves the old file aside, then refuses the new one
    struct Stubborn;
    impl ReplaceOps for Stubborn {
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            if from.to_string_lossy().ends_with(DOWNLOAD_SUFFIX) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            std::fs::rename(from, to)
        }
        fn remove(&self, file: &Path) -> io::Result<()> {
            std::fs::remove_file(file)
        }
    }
    let dir = temp();
    let target = dir.path().join("winmm.dll");
    let tmp = with_suffix(&target, DOWNLOAD_SUFFIX);
    std::fs::write(&target, "old").unwrap();
    std::fs::write(&tmp, "new").unwrap();
    assert!(replace_file(&tmp, &target, &Stubborn).is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    assert_eq!(names(dir.path()), ["winmm.dll", "winmm.dll.rr-download"]);
}

#[tokio::test]
async fn install_clears_old_runtime_files_left_aside_by_an_earlier_install() {
    let runtime = make_runtime(1);
    let server = FakeServer::start(Some(runtime.clone())).await;
    let win64 = temp();
    std::fs::write(win64.path().join("MysticParadox.dll.rr-old-123"), "old").unwrap();
    std::fs::write(win64.path().join("notes.txt.rr-old-1"), "not ours").unwrap();
    let result = install(&server, &manifest_for(&runtime), win64.path()).await.unwrap();
    assert!(result.replaced_in_use.is_empty());
    assert!(!win64.path().join("MysticParadox.dll.rr-old-123").exists());
    assert!(win64.path().join("notes.txt.rr-old-1").exists());
}

// ---------------------------------------------------------------- end to end

/// The launcher's whole round, as the controller drives it: fetch the manifest, ask, install, check
/// before PLAY; then a file changes on disk, the server publishes a new build, and a manifest lies about
/// the files it serves.
#[tokio::test]
async fn end_to_end_accept_install_check_tamper_and_a_changed_set() {
    let first = make_runtime(1);
    let server = FakeServer::start(Some(first.clone())).await;
    let win64 = temp();
    let win64 = win64.path();
    let mut trusted: Vec<String> = Vec::new();

    let RuntimeManifestResult::Ok(manifest) = fetch_runtime_manifest(&server.endpoint).await else {
        panic!("no manifest");
    };
    let check = |manifest: RuntimeManifest, trusted: bool| async move {
        check_runtime(win64, Some(&manifest), trusted, None).await
    };
    assert_eq!(
        check(manifest.clone(), is_trusted(&trusted, &manifest.files)).await,
        (RuntimeState::Untrusted, false),
        "nothing runs before the player accepts"
    );
    accept(&mut trusted, &manifest.files);
    install(&server, &manifest, win64).await.unwrap();
    assert_eq!(check(manifest.clone(), true).await, (RuntimeState::Ok, true));

    // A file changed on disk after the install (another program, a hand edit): PLAY refuses it
    let mut tampered = first[1].1.clone();
    tampered[100] ^= 1;
    std::fs::write(win64.join("winmm.dll"), &tampered).unwrap();
    assert_eq!(check(manifest.clone(), true).await, (RuntimeState::Outdated, false));
    // Installing again brings back only the file that changed
    server.with(|state| state.seen.clear());
    install(&server, &manifest, win64).await.unwrap();
    assert_eq!(server.downloads().len(), 1);
    assert_eq!(check(manifest.clone(), true).await, (RuntimeState::Ok, true));

    // The server publishes another build: it is not trusted until the player accepts it again, and the
    // old files no longer pass
    let second = make_runtime(2);
    server.with(|state| state.runtime = Some(second.clone()));
    let RuntimeManifestResult::Ok(changed) = fetch_runtime_manifest(&server.endpoint).await else {
        panic!("no manifest");
    };
    assert_ne!(runtime_set_id(&changed.files), runtime_set_id(&manifest.files));
    assert!(!is_trusted(&trusted, &changed.files));
    assert_eq!(check(changed.clone(), false).await, (RuntimeState::Untrusted, false));
    accept(&mut trusted, &changed.files);
    assert_eq!(check(changed.clone(), true).await, (RuntimeState::Outdated, false));
    install(&server, &changed, win64).await.unwrap();
    assert_eq!(check(changed.clone(), true).await, (RuntimeState::Ok, true));
    for (name, data) in &second {
        assert_eq!(std::fs::read(win64.join(name)).unwrap(), *data);
    }

    // A manifest that lies about the files it serves: the download is refused, the installed set stays
    server.with(|state| {
        state.runtime = Some(make_runtime(5));
        state.manifest_override = Some(manifest_json(&make_runtime(6)));
    });
    let RuntimeManifestResult::Ok(lying) = fetch_runtime_manifest(&server.endpoint).await else {
        panic!("no manifest");
    };
    let error = install(&server, &lying, win64).await.unwrap_err();
    assert_eq!(error.code, RuntimeErrorCode::Different);
    assert_eq!(check(changed, true).await, (RuntimeState::Ok, true));
    assert!(leftovers(win64).is_empty());
}

#[tokio::test]
async fn end_to_end_a_partial_download_leaves_the_old_runtime_in_place() {
    let first = make_runtime(1);
    let server = FakeServer::start(Some(first.clone())).await;
    let win64 = temp();
    let win64 = win64.path();
    let manifest = manifest_for(&first);
    install(&server, &manifest, win64).await.unwrap();

    let second = make_runtime(2);
    server.with(|state| {
        state.runtime = Some(second.clone());
        state.partial = true;
    });
    let changed = manifest_for(&second);
    let error = install(&server, &changed, win64).await.unwrap_err();
    assert_eq!(error.code, RuntimeErrorCode::DownloadFailed, "{error}");
    assert!(leftovers(win64).is_empty(), "no half file left behind");
    assert_eq!(
        check_runtime(win64, Some(&manifest), true, None).await,
        (RuntimeState::Ok, true),
        "the old runtime still works"
    );
    assert_eq!(
        check_runtime(win64, Some(&changed), true, None).await,
        (RuntimeState::Outdated, false)
    );

    // A body larger than the list says is cut off, never written in full
    server.with(|state| {
        state.partial = false;
        state.oversize = true;
    });
    let error = install(&server, &changed, win64).await.unwrap_err();
    assert_eq!(error.code, RuntimeErrorCode::Different);
    assert!(leftovers(win64).is_empty());

    // Once the server answers properly the next install completes
    server.with(|state| state.oversize = false);
    install(&server, &changed, win64).await.unwrap();
    assert_eq!(
        check_runtime(win64, Some(&changed), true, None).await,
        (RuntimeState::Ok, true)
    );
}

#[tokio::test]
async fn a_cancelled_install_says_so_and_a_missing_file_is_not_found() {
    let runtime = make_runtime(1);
    let server = FakeServer::start(Some(runtime.clone())).await;
    let win64 = temp();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = install_runtime(InstallRequest {
        endpoint: &server.endpoint,
        headers: headers(),
        manifest: &manifest_for(&runtime),
        win64_dir: win64.path(),
        cancel: Some(cancel),
        on_progress: None,
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, RuntimeErrorCode::Cancelled);
    server.with(|state| state.runtime = Some(vec![runtime[0].clone()]));
    let error = install(&server, &manifest_for(&runtime), win64.path())
        .await
        .unwrap_err();
    assert_eq!(
        (error.code, error.file.as_deref()),
        (RuntimeErrorCode::NotFound, Some("winmm.dll"))
    );
}

#[test]
fn file_names_are_encoded_as_encode_uri_component_does() {
    let encoded = percent_encoding::utf8_percent_encode("a b/c?.dll!~*'()", URI_COMPONENT).to_string();
    assert_eq!(encoded, "a%20b%2Fc%3F.dll!~*'()");
}

#[test]
fn page_infos_carry_the_manifest_files() {
    let manifest = manifest_for(&make_runtime(1));
    let infos = file_infos(&manifest.files);
    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].name, "MysticParadox.dll");
    assert_eq!(infos[0].sha256, manifest.files[0].sha256);
}
