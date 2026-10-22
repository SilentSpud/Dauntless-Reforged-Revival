//! RH5: `update` and setup's install step from the server release, against releases these tests
//! make (both archive kinds on every OS) in a folder or behind a local HTTP server.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use host::cli::UpdateArgs;
use host::commands::setup::install::install_from_release;
use host::commands::update::{UpdateEnv, run_with};
use host::error::HostError;
use host::paths::Paths;
use host::selfupdate::{
    self, Attestation, Candidate, Location, Manifest, Problem, Source, UpdateError, UpdateResult, Wanted,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes a release of `files` (path -> text) for `platform` into `dir`: its archive, and its line in
/// `SHA256SUMS.txt` (added to what is there).
fn make_release(dir: &Path, version: &str, platform: &str, files: &[(&str, &str)]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let top = format!("reforged-revival-server-{version}");
    let mut all: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec()))
        .collect();
    let listed: Vec<Value> = all
        .iter()
        .map(|(p, d)| json!({ "path": p, "sha256": sha(d), "size": d.len() }))
        .collect();
    let manifest = json!({
        "name": "reforged-revival-server", "version": version, "commit": "0123456789abcdef0123",
        "target": "test", "platform": platform, "files": listed,
    });
    all.push(("release.json".into(), serde_json::to_vec_pretty(&manifest).unwrap()));
    let name = selfupdate::archive_name(version, platform);
    let archive = dir.join(&name);
    if name.ends_with(".zip") {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, data) in &all {
            zip.start_file(format!("{top}/{path}"), options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    } else {
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&archive).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (path, data) in &all {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(if path.starts_with("bin/") { 0o755 } else { 0o644 });
            header.set_cksum();
            tar.append_data(&mut header, format!("{top}/{path}"), data.as_slice())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }
    let line = format!("{}  {name}\n", sha(&std::fs::read(&archive).unwrap()));
    let sums = dir.join("SHA256SUMS.txt");
    let mut text = std::fs::read_to_string(&sums).unwrap_or_default();
    text.push_str(&line);
    std::fs::write(&sums, text).unwrap();
    archive
}

const V1: &[(&str, &str)] = &[
    ("bin/reforged-host", "host 1"),
    ("paradox/backend/build/server.js", "backend 1"),
    ("paradox/backend/node_modules/express/index.js", "express"),
    ("native/paradox-runtime/dllmain.cpp", "runtime 1"),
    ("docs/old.md", "goes in 2"),
];
const V2: &[(&str, &str)] = &[
    ("bin/reforged-host", "host 2"),
    ("paradox/backend/build/server.js", "backend 2"),
    ("paradox/backend/node_modules/express/index.js", "express"),
    ("native/paradox-runtime/dllmain.cpp", "runtime 2"),
];

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(path.split('/').fold(root.to_path_buf(), |p, s| p.join(s))).unwrap()
}

// ------------------------------------------------------------------ the checks

#[test]
fn a_checksum_failure_deletes_the_download_and_installs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    make_release(&releases, "1.0.0", "linux-x64", V1);
    // The archive changes after SHA256SUMS.txt was written.
    let archive = releases.join(selfupdate::archive_name("1.0.0", "linux-x64"));
    let mut data = std::fs::read(&archive).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xff;
    std::fs::write(&archive, data).unwrap();

    let candidate =
        selfupdate::find_release(&Source::Folder(releases.clone()), "linux-x64", Wanted::Newest(None)).unwrap();
    let work = dir.path().join("work");
    let Err(UpdateError::Problem(Problem::Checksum { file, .. })) = selfupdate::fetch(&candidate, &work) else {
        panic!("a changed archive must not pass")
    };
    assert_eq!(file, "ReforgedRevivalServer-1.0.0-linux-x64.tar.gz");
    assert_eq!(
        std::fs::read_dir(&work).unwrap().count(),
        0,
        "nothing is left in the work folder"
    );
}

#[test]
fn a_missing_file_stops_before_anything_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    make_release(&releases, "1.0.0", "windows-x64", V1);
    let source = Source::Folder(releases.clone());

    // No release for this platform at all
    let Err(UpdateError::Problem(p @ Problem::NoRelease(_))) =
        selfupdate::find_release(&source, "linux-x64", Wanted::Newest(None))
    else {
        panic!("linux-x64 has no archive")
    };
    assert_eq!(p.code(), "no_release");

    // SHA256SUMS.txt lists it, the archive is gone
    std::fs::remove_file(releases.join(selfupdate::archive_name("1.0.0", "windows-x64"))).unwrap();
    let candidate = selfupdate::find_release(&source, "windows-x64", Wanted::Newest(None)).unwrap();
    let Err(UpdateError::Problem(Problem::DownloadFailed(_))) = selfupdate::fetch(&candidate, &dir.path().join("w1"))
    else {
        panic!("a missing archive is a failed download")
    };

    // The archive is there, SHA256SUMS.txt does not list it
    let other = dir.path().join("other");
    make_release(&other, "1.0.0", "windows-x64", V1);
    let candidate = Candidate {
        sums_at: Location::File(releases.join("SHA256SUMS.txt")),
        archive: "ReforgedRevivalServer-9.9.9-windows-x64.zip".into(),
        ..selfupdate::find_release(&Source::Folder(other), "windows-x64", Wanted::Newest(None)).unwrap()
    };
    let Err(UpdateError::Problem(p @ Problem::NotInSums(_))) = selfupdate::fetch(&candidate, &dir.path().join("w2"))
    else {
        panic!("an archive SHA256SUMS.txt does not list is refused")
    };
    assert_eq!(p.code(), "not_in_sums");

    // A file of the release missing from the archive's folder: release.json does not match
    let bad = dir.path().join("bad");
    let archive = make_release(&bad, "1.0.0", "linux-x64", V1);
    let staged = selfupdate::extract(&archive, &dir.path().join("staging")).unwrap();
    std::fs::remove_file(staged.join("docs").join("old.md")).unwrap();
    let root = dir.path().join("server");
    std::fs::create_dir_all(&root).unwrap();
    let Err(UpdateError::Problem(Problem::BadArchive(why))) = selfupdate::install(&staged, &root, None) else {
        panic!("an incomplete release is not installed")
    };
    assert!(why.contains("docs/old.md"), "{why}");
    assert!(!root.join("release.json").exists());
}

/// Answers every request with `head` then `body`, then closes the connection.
fn serve(head: String, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn an_interrupted_download_is_refused_and_removed() {
    // 1000 bytes announced, 10 sent, then the connection closes
    let base = serve(
        "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n".into(),
        b"0123456789".to_vec(),
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("archive.part");
    let error = selfupdate::http::download(&format!("{base}/a.tar.gz"), &file).unwrap_err();
    assert!(error.contains("interrupted"), "{error}");

    let sums_dir = dir.path().join("sums");
    std::fs::create_dir_all(&sums_dir).unwrap();
    let name = selfupdate::archive_name("1.0.0", "linux-x64");
    std::fs::write(sums_dir.join("SHA256SUMS.txt"), format!("{}  {name}\n", "0".repeat(64))).unwrap();
    let candidate = Candidate {
        version: "1.0.0".into(),
        archive: name,
        archive_at: Location::Url(format!("{base}/archive")),
        sums_at: Location::File(sums_dir.join("SHA256SUMS.txt")),
        repository: None,
    };
    let work = dir.path().join("work");
    let Err(UpdateError::Problem(Problem::DownloadFailed(why))) = selfupdate::fetch(&candidate, &work) else {
        panic!("an interrupted download is refused")
    };
    assert!(why.contains("interrupted"), "{why}");
    assert_eq!(
        std::fs::read_dir(&work).unwrap().count(),
        0,
        "the partial file is removed"
    );
}

#[test]
fn a_http_error_is_a_failed_download() {
    let base = serve(
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        Vec::new(),
    );
    let Err(UpdateError::Problem(Problem::DownloadFailed(why))) =
        selfupdate::find_release(&Source::Url(base), "linux-x64", Wanted::Newest(None))
    else {
        panic!("a 404 for SHA256SUMS.txt")
    };
    assert!(why.contains("HTTP 404"), "{why}");
}

#[test]
fn downloads_a_release_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    let archive = make_release(&releases, "1.2.0", "linux-x64", V1);
    let data = std::fs::read(&archive).unwrap();
    let base = serve(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            data.len()
        ),
        data,
    );
    let file = dir.path().join("got");
    selfupdate::http::download(&format!("{base}/x"), &file).unwrap();
    assert_eq!(
        selfupdate::sha256_file(&file).unwrap(),
        selfupdate::sha256_file(&archive).unwrap()
    );
}

// ------------------------------------------------------------------ choosing a release

#[test]
fn offers_stable_releases_and_prereleases_to_the_canary() {
    use selfupdate::{Version, pick_newest};
    let v = |s| Version::parse(s).unwrap();
    assert!(v("1.0.0") > v("1.0.0-rc.2"));
    assert!(v("1.0.0-rc.10") > v("1.0.0-rc.2"));
    assert!(v("1.0.0-rc.1") > v("1.0.0-beta"));
    assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
    assert!(
        Version::parse("1.0").is_none() && Version::parse("01.0.0").is_none() && Version::parse("1.0.0-").is_none()
    );

    let all = ["0.1.0", "0.2.0-rc.1", "0.1.1"];
    assert_eq!(pick_newest(&all, Some("0.1.0")), Some("0.1.1"));
    assert_eq!(pick_newest(&all, None), Some("0.1.1"));
    assert_eq!(
        pick_newest(&all, Some("0.2.0-rc.0")),
        Some("0.2.0-rc.1"),
        "the canary takes prereleases"
    );
    assert_eq!(
        pick_newest(&["0.1.0-rc.1", "0.1.0-rc.2"], None),
        Some("0.1.0-rc.2"),
        "only prereleases yet"
    );
    assert_eq!(
        selfupdate::archive_version("ReforgedRevivalServer-0.2.0-rc.1-linux-x64.tar.gz", "linux-x64"),
        Some("0.2.0-rc.1")
    );
    assert_eq!(
        selfupdate::archive_version("ReforgedRevivalServer-0.2.0-windows-x64.zip", "linux-x64"),
        None
    );
}

#[test]
fn picks_the_release_from_githubs_list() {
    let asset = |name: &str| json!({ "name": name, "browser_download_url": format!("https://example.test/{name}") });
    let releases = json!([
        { "tag_name": "launcher-v9.0.0", "draft": false, "assets": [asset("ReforgedRevivalServer-9.0.0-linux-x64.tar.gz"), asset("SHA256SUMS.txt")] },
        { "tag_name": "server-v0.3.0", "draft": true, "assets": [asset("ReforgedRevivalServer-0.3.0-linux-x64.tar.gz"), asset("SHA256SUMS.txt")] },
        { "tag_name": "server-v0.2.0", "draft": false, "assets": [asset("ReforgedRevivalServer-0.2.0-windows-x64.zip"), asset("SHA256SUMS.txt")] },
        { "tag_name": "server-v0.1.0", "draft": false, "assets": [asset("ReforgedRevivalServer-0.1.0-linux-x64.tar.gz"), asset("SHA256SUMS.txt")] },
    ]);
    let c = selfupdate::candidates_from_github(&releases, "o/r", "linux-x64", Wanted::Newest(None)).unwrap();
    assert_eq!(
        c.version, "0.1.0",
        "not a draft, not another platform's, not the launcher's"
    );
    assert_eq!(
        c.archive_at,
        Location::Url("https://example.test/ReforgedRevivalServer-0.1.0-linux-x64.tar.gz".into())
    );
    assert_eq!(c.repository.as_deref(), Some("o/r"));
    let c = selfupdate::candidates_from_github(&releases, "o/r", "windows-x64", Wanted::Exactly("0.2.0")).unwrap();
    assert_eq!(c.version, "0.2.0");
    assert!(selfupdate::candidates_from_github(&releases, "o/r", "windows-x64", Wanted::Exactly("0.1.0")).is_err());
}

#[test]
fn reads_sha256sums_and_refuses_a_manifest_that_leaves_the_folder() {
    let sums = format!(
        "{}  a.zip\r\n{} *b.tar.gz\nnot a line\n",
        "A".repeat(64),
        "b".repeat(64)
    );
    assert_eq!(
        selfupdate::parse_sums(&sums),
        [
            ("a".repeat(64), "a.zip".to_owned()),
            ("b".repeat(64), "b.tar.gz".to_owned())
        ]
    );
    let manifest = |path: &str| {
        json!({ "name": "reforged-revival-server", "version": "1.0.0", "files": [{ "path": path, "sha256": "0".repeat(64) }] }).to_string()
    };
    assert!(Manifest::parse(&manifest("bin/gateway")).is_ok());
    for bad in ["../x", "/etc/passwd", "a/../../x", "C:/x", "a\\b", ""] {
        assert!(Manifest::parse(&manifest(bad)).is_err(), "{bad}");
    }
}

// ------------------------------------------------------------------ installing

#[test]
fn moves_a_node_kit_checkout_over_then_updates_it() {
    for platform in ["windows-x64", "linux-x64"] {
        let dir = tempfile::tempdir().unwrap();
        let releases = dir.path().join("releases");
        let root = dir.path().join("Reforged");
        // A Node-kit checkout: npm's builds, the host's own files
        for (path, text) in [
            ("paradox/backend/build/server.js", "npm build"),
            ("paradox/backend/node_modules/stale/index.js", "from npm"),
            ("paradox/backend/game-data/store.json", "the host's game data"),
            (".workspace/config/host.json", "{}"),
        ] {
            let file = path.split('/').fold(root.clone(), |p, s| p.join(s));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        make_release(&releases, "1.0.0", platform, V1);
        let source = Source::Folder(releases.clone());
        let c = selfupdate::find_release(&source, platform, Wanted::Newest(None)).unwrap();
        let archive = selfupdate::fetch(&c, &root.join(".workspace").join("update")).unwrap();
        let staged = selfupdate::extract(&archive, &root.join(".workspace").join("update").join("staging")).unwrap();
        let (m1, r1) = selfupdate::install(&staged, &root, None).unwrap();
        assert_eq!((m1.version.as_str(), r1.files, r1.runtime_changed), ("1.0.0", 5, None));
        assert_eq!(read(&root, "paradox/backend/build/server.js"), "backend 1");
        assert!(
            !root.join("paradox/backend/node_modules/stale").exists(),
            "npm's node_modules replaced"
        );
        assert_eq!(
            read(&root, "paradox/backend/game-data/store.json"),
            "the host's game data"
        );
        assert_eq!(read(&root, ".workspace/config/host.json"), "{}");
        assert!(selfupdate::damaged_files(&root, &m1).unwrap().is_empty());
        assert!(!selfupdate::installing_marker(&root).exists());
        #[cfg(unix)]
        if platform == "linux-x64" {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("bin/reforged-host"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the binaries stay executable");
        }

        // 2.0.0 over 1.0.0: changed files written, docs/old.md removed
        make_release(&releases, "2.0.0", platform, V2);
        let c = selfupdate::find_release(&source, platform, Wanted::Newest(Some("1.0.0"))).unwrap();
        assert_eq!(c.version, "2.0.0");
        let archive = selfupdate::fetch(&c, &root.join(".workspace").join("update")).unwrap();
        let staged = selfupdate::extract(&archive, &root.join(".workspace").join("update").join("staging")).unwrap();

        // The host changed a file of the release: refused, as git refused it
        std::fs::write(root.join("paradox/backend/build/server.js"), "edited").unwrap();
        let Err(UpdateError::Problem(Problem::LocalChanges(files))) = selfupdate::install(&staged, &root, Some(&m1))
        else {
            panic!("local changes")
        };
        assert_eq!(files, ["paradox/backend/build/server.js"]);
        std::fs::write(root.join("paradox/backend/build/server.js"), "backend 1").unwrap();

        let (m2, r2) = selfupdate::install(&staged, &root, Some(&m1)).unwrap();
        assert_eq!((m2.version.as_str(), r2.files, r2.removed), ("2.0.0", 3, 1));
        assert_eq!(r2.runtime_changed, Some(true));
        assert!(!root.join("docs").exists(), "an emptied folder goes too");
        assert_eq!(read(&root, "bin/reforged-host"), "host 2");
        assert_eq!(selfupdate::read_manifest(&root).unwrap().unwrap().version, "2.0.0");
    }
}

#[test]
fn a_node_kit_checkout_with_a_changed_tracked_file_is_refused() {
    let git = |root: &Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "core.autocrlf=false"])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Reforged");
    for (path, text) in [
        ("docs/old.md", "as committed"),
        ("paradox/backend/package-lock.json", "{}"),
    ] {
        let file = path.split('/').fold(root.clone(), |p, s| p.join(s));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "node kit"]);
    let mut files = V1.to_vec();
    files.push(("paradox/backend/package-lock.json", "{\"lockfileVersion\": 3}"));
    let archive = make_release(&dir.path().join("releases"), "1.0.0", "linux-x64", &files);
    let staged = selfupdate::extract(&archive, &dir.path().join("staging")).unwrap();

    // npm rewrote the lock file: that does not count. The host edited docs/old.md: refused.
    std::fs::write(root.join("paradox/backend/package-lock.json"), "{\"npm\": 11}").unwrap();
    std::fs::write(root.join("docs/old.md"), "edited by the host").unwrap();
    let Err(UpdateError::Problem(Problem::LocalChanges(changed))) = selfupdate::install(&staged, &root, None) else {
        panic!("a changed tracked file stops the move")
    };
    assert_eq!(changed, ["docs/old.md"]);

    // Put back: it moves over
    git(&root, &["checkout", "-q", "--", "docs/old.md"]);
    selfupdate::install(&staged, &root, None).unwrap();
    assert_eq!(read(&root, "docs/old.md"), "goes in 2");
}

#[test]
fn setup_puts_back_a_damaged_release_and_leaves_a_whole_one() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    let root = dir.path().join("Reforged");
    std::fs::create_dir_all(&root).unwrap();
    let archive = make_release(&releases, "1.0.0", "linux-x64", V1);
    let staged = selfupdate::extract(&archive, &dir.path().join("staging")).unwrap();
    let (m, _) = selfupdate::install(&staged, &root, None).unwrap();
    let log = dir.path().join("setup.log");
    let source = Source::Folder(releases.clone());

    let mut lines = Vec::new();
    install_from_release(&root, Some(&m), "linux-x64", &source, &log, &mut |l| {
        lines.push(l.to_owned())
    })
    .unwrap();
    assert_eq!(
        lines,
        ["- servers 1.0.0 (linux-x64, 0123456789ab) from the release: every file checked"]
    );

    std::fs::remove_file(root.join("bin/reforged-host")).unwrap();
    std::fs::write(root.join("paradox/backend/build/server.js"), "edited").unwrap();
    let mut lines = Vec::new();
    install_from_release(&root, Some(&m), "linux-x64", &source, &log, &mut |l| {
        lines.push(l.to_owned())
    })
    .unwrap();
    assert!(
        lines[0].starts_with("- 2 files of servers 1.0.0 are missing or changed"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "- SHA-256 checked against SHA256SUMS.txt"),
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "- servers 1.0.0 installed");
    assert_eq!(read(&root, "bin/reforged-host"), "host 1");
    assert_eq!(read(&root, "paradox/backend/build/server.js"), "backend 1");

    // A newer release is update's, not setup's
    make_release(&releases, "2.0.0", "linux-x64", V2);
    std::fs::remove_file(root.join("bin/reforged-host")).unwrap();
    install_from_release(&root, Some(&m), "linux-x64", &source, &log, &mut |_| {}).unwrap();
    assert_eq!(read(&root, "bin/reforged-host"), "host 1");
}

// ------------------------------------------------------------------ the command

struct Env {
    source: Source,
    platform: &'static str,
    setups: std::cell::RefCell<Vec<PathBuf>>,
}

impl UpdateEnv for Env {
    fn platform(&self) -> Result<&'static str, HostError> {
        Ok(self.platform)
    }
    fn source(&self, _installed: Option<&Manifest>) -> Source {
        self.source.clone()
    }
    fn verify_attestation(&self, _archive: &Path, _candidate: &Candidate, _log: &Path) -> UpdateResult<Attestation> {
        Ok(Attestation::Skipped("a test".into()))
    }
    fn checkout_commit(&self, _root: &Path) -> String {
        "abcdef012345".into()
    }
    fn run_setup(&self, program: &Path, _root: &Path) -> Result<i32, HostError> {
        self.setups.borrow_mut().push(program.to_path_buf());
        Ok(0)
    }
}

fn update(root: &Path, env: &Env, args: UpdateArgs) -> (Result<(), String>, Vec<String>) {
    let mut lines = Vec::new();
    let r = run_with(&Paths::new(root), &args, env, &mut |l| lines.push(l.to_owned()));
    (r.map(|_| ()).map_err(|e| e.to_string()), lines)
}

/// The JSON line (the launcher reads the last line that is one).
fn last_json(lines: &[String]) -> BTreeMap<String, Value> {
    lines.iter().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap()
}

#[test]
fn update_checks_installs_and_runs_setup_with_the_new_binary() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    let root = dir.path().join("Reforged");
    std::fs::create_dir_all(root.join(".workspace").join("config")).unwrap();
    make_release(&releases, "1.0.0", "linux-x64", V1);
    let env = Env {
        source: Source::Folder(releases.clone()),
        platform: "linux-x64",
        setups: Default::default(),
    };
    let json = UpdateArgs {
        json: true,
        ..Default::default()
    };

    let (r, lines) = update(
        &root,
        &env,
        UpdateArgs {
            check: true,
            ..json.clone()
        },
    );
    r.unwrap();
    assert_eq!(
        lines[0],
        "This server runs from a git checkout (abcdef012345): update moves it to the server release 1.0.0 (linux-x64)."
    );
    let check = last_json(&lines);
    assert_eq!(
        (check["behind"].clone(), check["head"].clone()),
        (json!(1), json!("abcdef012345"))
    );

    // Not set up yet: installed, and setup is left to the host
    let (r, lines) = update(&root, &env, json.clone());
    r.unwrap();
    let done = last_json(&lines);
    assert_eq!(
        (done["updated"].clone(), done["to"].clone(), done["files"].clone()),
        (json!(true), json!("1.0.0"), json!(5))
    );
    assert_eq!(lines.last().unwrap(), "Now run reforged-host setup.", "{lines:?}");

    let (r, lines) = update(
        &root,
        &env,
        UpdateArgs {
            check: true,
            ..json.clone()
        },
    );
    r.unwrap();
    assert_eq!(lines[0], "Up to date: servers 1.0.0 (linux-x64).");
    assert_eq!(last_json(&lines)["behind"], json!(0));

    // Set up: 2.0.0 is installed and setup runs again with the release's reforged-host
    std::fs::write(root.join(".workspace").join("config").join("host.json"), "{}").unwrap();
    make_release(&releases, "2.0.0", "linux-x64", V2);
    let (r, lines) = update(&root, &env, UpdateArgs::default());
    r.unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Updated to servers 2.0.0 from 1.0.0 (3 files written, 1 removed)")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("The game runtime changed")),
        "{lines:?}"
    );
    assert_eq!(
        *env.setups.borrow(),
        [root
            .join("bin")
            .join(format!("reforged-host{}", std::env::consts::EXE_SUFFIX))]
    );
    assert!(!root.join(".workspace").join("update").join("staging").exists());

    // A damaged download: the error the launcher reads, then the message
    make_release(&releases, "3.0.0", "linux-x64", V2);
    let archive = releases.join(selfupdate::archive_name("3.0.0", "linux-x64"));
    std::fs::write(&archive, b"not the archive").unwrap();
    let (r, lines) = update(&root, &env, json);
    let error = r.unwrap_err();
    assert!(
        error.starts_with("ReforgedRevivalServer-3.0.0-linux-x64.tar.gz is not the file the release lists"),
        "{error}"
    );
    let failed = last_json(&lines);
    assert_eq!(failed["error"], json!("checksum"));
    assert_eq!(
        selfupdate::read_manifest(&root).unwrap().unwrap().version,
        "2.0.0",
        "2.0.0 stays"
    );
}

#[test]
fn an_update_that_stopped_half_way_installs_again() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    let root = dir.path().join("Reforged");
    std::fs::create_dir_all(&root).unwrap();
    make_release(&releases, "1.0.0", "windows-x64", V1);
    let env = Env {
        source: Source::Folder(releases),
        platform: "windows-x64",
        setups: Default::default(),
    };
    update(
        &root,
        &env,
        UpdateArgs {
            no_setup: true,
            ..Default::default()
        },
    )
    .0
    .unwrap();
    std::fs::write(selfupdate::installing_marker(&root), "1.0.0").unwrap();
    let (r, lines) = update(
        &root,
        &env,
        UpdateArgs {
            check: true,
            ..Default::default()
        },
    );
    r.unwrap();
    assert!(
        lines[0].starts_with("The last update stopped before it was complete"),
        "{lines:?}"
    );
    let (r, lines) = update(
        &root,
        &env,
        UpdateArgs {
            no_setup: true,
            ..Default::default()
        },
    );
    r.unwrap();
    assert!(
        lines.iter().any(|l| l.starts_with("Updated to servers 1.0.0")),
        "{lines:?}"
    );
    assert!(!selfupdate::installing_marker(&root).exists());
}
