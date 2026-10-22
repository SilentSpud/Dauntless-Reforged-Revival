//! The host kit's workspace as the Host page sees it (Reforged's `src/main/hostkit.ts`), for the planned
//! layout only (RD23): a Dauntless Reforged Revival folder with `reforged-host` in it, Mystic Paradox
//! in `paradox/` and `native/` (RD1), and the host's own files git-ignored beside the code:
//!
//! ```text
//! <workspace>\reforged-host.exe                  the host kit (RD3), or bin\, or rust\target\release\
//! <workspace>\native\paradox-runtime\            the game runtime's source
//!     SDK.hpp, SDK\                              the Dumper-7 CppSDK, copied in by "Import SDK"
//!     x64\Release\MysticParadox.dll
//! <workspace>\native\runtime-loader\target\release\winmm.dll
//! <workspace>\native\catalog-exporter\x64\Release\CatalogExporter.dll
//! <workspace>\paradox\backend\game-data\, paradox\director\game-data\
//! <workspace>\paradox\Items_Analysis\            CatalogExporter's output, copied in from the game folder
//! <workspace>\.workspace\config\host.json        the server's settings (its secrets are in secrets.env)
//! <workspace>\.workspace\runtime\                what the content server hands to players
//! <workspace>\.workspace\logs\                   the logs of what the launcher started
//! ```
//!
//! Today's Node kit paths (`ReforgedHost\package.json`, `.workspace\Mystic-Paradox`, `npm run upstream`)
//! are not read: the game data, the pack import, the examples and the runtime's publication are
//! `reforged-host` commands (`hostserver`), so their formats exist once, in the host kit.

pub mod parse;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use self::parse::ServerMode;
use crate::bridge::types::{
    GameDataState, HostBuilds, HostDump, HostExports, HostGameData, HostKit, HostMultipliers, HostSdk,
    HostServerConfig, HostServerMode, HostToolchain, HostUpstream,
};

/// The two files `publish-runtime` puts in `.workspace\runtime\` for the players.
pub const RUNTIME_FILE_NAMES: [&str; 2] = ["MysticParadox.dll", "winmm.dll"];

pub const BACKEND_GAME_DATA: [&str; 7] = [
    "progression_config.json",
    "slayers_path.json",
    "slayers_path_definitions.json",
    "ladyluck_store.json",
    "platinum_store.json",
    "reward_cache_store.json",
    "inventory_storage_policy.json",
];

pub const DIRECTOR_GAME_DATA: [&str; 7] = [
    "player_hunts_table.json",
    "matchmaker_hunts_table.json",
    "arena_easy_matchmaker_hunts.json",
    "arena_hard_matchmaker_hunts.json",
    "arena_elite_matchmaker_hunts.json",
    "arena_hard_matchmaker_hunts_new.json",
    "arena_elite_matchmaker_hunts_new.json",
];

/// The flags file CatalogExporter reads from the game's working folder (Mystic Paradox's
/// `docs/GENERATING_GAME_DATA.md`): the catalog, Slayer's Path, hunts, the store and the progression
/// tracks, nothing client-only. CRLF, exactly as Reforged writes it.
pub const EXPORT_FLAGS: &str = "# Written by the Dauntless Reforged Revival launcher (Host page). See docs/sdk-and-game-data.md.\r\n\
EXPORT_CATALOG=1\r\n\
EXPORT_SLAYERS_PATH=1\r\n\
EXPORT_HUNTS=1\r\n\
EXPORT_STORE_RUNTIME=1\r\n\
EXPORT_PROGRESSION=1\r\n\
EXPORT_COMBAT=0\r\n\
EXPORT_SKINS=0\r\n\
EXPORT_SKINS_RESOLVE_STRINGTABLE=0\r\n\
EXPORT_WEAPON_SLOTS=0\r\n\
EXPORT_TABLE_INVENTORY=0\r\n\
EXPORT_CELLS=0\r\n\
EXPORT_DROP_TABLES=0\r\n\
EXPORT_CURRENCY_NAMES=0\r\n";

/// Dumper-7 writes to `C:\Dumper-7\<game and version>\CppSDK` unless its ini says otherwise.
pub const DUMPER_OUTPUT_ROOT: &str = "C:\\Dumper-7";

/// The host kit's program (RD3).
pub const HOST_BINARY: &str = if cfg!(windows) {
    "reforged-host.exe"
} else {
    "reforged-host"
};

/// Where everything is in a workspace (Reforged's `workspacePaths`, planned layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaths {
    pub root: PathBuf,
    pub runtime_source: PathBuf,
    pub sdk_header: PathBuf,
    pub sdk_dir: PathBuf,
    pub runtime_solution: PathBuf,
    pub runtime_dll: PathBuf,
    pub loader_dir: PathBuf,
    pub loader_dll: PathBuf,
    pub exporter_project: PathBuf,
    pub exporter_dll: PathBuf,
    pub backend_game_data: PathBuf,
    pub director_game_data: PathBuf,
    pub exports: PathBuf,
    pub publish: PathBuf,
    pub logs: PathBuf,
    pub host_json: PathBuf,
    pub gamedata_packs: PathBuf,
}

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base.to_path_buf(), |path, part| path.join(part))
}

impl WorkspacePaths {
    pub fn new(root: &Path) -> Self {
        let paradox = root.join("paradox");
        let native = root.join("native");
        let runtime_source = native.join("paradox-runtime");
        let loader_dir = native.join("runtime-loader");
        let exporter_dir = native.join("catalog-exporter");
        let workspace = root.join(".workspace");
        Self {
            sdk_header: runtime_source.join("SDK.hpp"),
            sdk_dir: runtime_source.join("SDK"),
            runtime_solution: runtime_source.join("MysticParadox.sln"),
            runtime_dll: join(&runtime_source, &["x64", "Release", "MysticParadox.dll"]),
            loader_dll: join(&loader_dir, &["target", "release", "winmm.dll"]),
            exporter_project: exporter_dir.join("CatalogExporter.vcxproj"),
            exporter_dll: join(&exporter_dir, &["x64", "Release", "CatalogExporter.dll"]),
            backend_game_data: join(&paradox, &["backend", "game-data"]),
            director_game_data: join(&paradox, &["director", "game-data"]),
            exports: paradox.join("Items_Analysis"),
            publish: workspace.join("runtime"),
            logs: workspace.join("logs"),
            host_json: join(&workspace, &["config", "host.json"]),
            gamedata_packs: workspace.join("gamedata-packs"),
            root: root.to_path_buf(),
            runtime_source,
            loader_dir,
        }
    }

    /// `.workspace\logs\launcher-<name>.log`
    pub fn launcher_log(&self, name: &str) -> PathBuf {
        self.logs.join(format!("launcher-{name}.log"))
    }
}

fn is_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file())
}

fn is_dir(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_dir())
}

/// `reforged-host` in a workspace: beside the release's files, in its `bin\`, or a build in the
/// checkout's `rust\target\release\`.
pub fn find_host_binary(root: &Path) -> Option<PathBuf> {
    [
        root.join(HOST_BINARY),
        root.join("bin").join(HOST_BINARY),
        join(root, &["rust", "target", "release", HOST_BINARY]),
    ]
    .into_iter()
    .find(|candidate| is_file(candidate))
}

/// A folder the Host page can work in: it has `reforged-host` and Mystic Paradox's `paradox\`.
pub fn is_workspace(root: &Path) -> bool {
    find_host_binary(root).is_some() && is_dir(&root.join("paradox"))
}

/// `SDK\*_functions.cpp`: an SDK the runtime builds with has them.
pub fn count_function_files(sdk_dir: &Path) -> u32 {
    std::fs::read_dir(sdk_dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with("_functions.cpp"))
            .count() as u32
    })
}

/// `real` when every file exists and none is byte for byte its shipped `*.example.json`, `synthetic`
/// when an example is in use, `missing` otherwise (the backend's own rule).
pub fn detect_game_data(dir: &Path, files: &[&str]) -> GameDataState {
    let mut synthetic = false;
    for name in files {
        let Ok(data) = std::fs::read(dir.join(name)) else {
            return GameDataState::Missing;
        };
        let example = dir.join(name.replace(".json", ".example.json"));
        if std::fs::read(example).is_ok_and(|e| e == data) {
            synthetic = true;
        }
    }
    if synthetic {
        GameDataState::Synthetic
    } else {
        GameDataState::Real
    }
}

fn iso_time(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Dumper-7 outputs: every `<root>\<name>\CppSDK` with an `SDK.hpp`, among at most 200 entries, newest
/// first.
pub fn find_dumps(root: &Path) -> Vec<HostDump> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<(SystemTime, HostDump)> = entries
        .filter_map(Result::ok)
        .take(200)
        .filter_map(|entry| {
            let dir = entry.path().join("CppSDK");
            let meta = dir.join("SDK.hpp").metadata().ok().filter(|m| m.is_file())?;
            let modified = meta.modified().ok()?;
            Some((
                modified,
                HostDump {
                    dir: dir.to_string_lossy().into_owned(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    modified: iso_time(modified),
                },
            ))
        })
        .collect();
    out.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    out.into_iter().map(|(_, dump)| dump).collect()
}

/// Visual Studio with the C++ workload, its MSBuild, and Rust's cargo for the loader.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolchain {
    pub visual_studio: Option<String>,
    pub major_version: Option<u32>,
    pub msbuild: Option<PathBuf>,
    pub cargo: Option<PathBuf>,
}

impl Toolchain {
    pub fn view(&self) -> HostToolchain {
        HostToolchain {
            visual_studio: self.visual_studio.clone(),
            msbuild: self.msbuild.is_some(),
            cargo: self.cargo.is_some(),
        }
    }
}

/// An external tool's output: 20 s and 4 MiB at most (Reforged's `execText`).
pub async fn exec_text(program: &Path, args: &[&str]) -> Option<String> {
    crate::platform::launch::run_hidden(program, args, Duration::from_secs(20), 4 * 1024 * 1024).await
}

pub fn vswhere_path(env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    let base = env("ProgramFiles(x86)").map_or_else(|| PathBuf::from("C:\\Program Files (x86)"), PathBuf::from);
    join(&base, &["Microsoft Visual Studio", "Installer", "vswhere.exe"])
}

/// `cargo.exe` in `%USERPROFILE%\.cargo\bin` or `%CARGO_HOME%\bin`.
pub fn cargo_candidates(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = env("USERPROFILE") {
        out.push(join(Path::new(&home), &[".cargo", "bin", "cargo.exe"]));
    }
    if let Some(home) = env("CARGO_HOME") {
        out.push(join(Path::new(&home), &["bin", "cargo.exe"]));
    }
    out
}

pub fn system_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

/// Visual Studio and MSBuild through `vswhere` (as Mystic Paradox's `_build.bat` does), and cargo.
/// Windows only.
pub async fn find_toolchain() -> Toolchain {
    if !cfg!(windows) {
        return Toolchain::default();
    }
    let mut tools = Toolchain::default();
    let vswhere = vswhere_path(&system_env);
    if is_file(&vswhere) {
        let json = exec_text(
            &vswhere,
            &[
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-format",
                "json",
                "-utf8",
            ],
        )
        .await;
        if let Some((name, major)) = json.as_deref().and_then(parse::vswhere_install) {
            tools.visual_studio = Some(name);
            tools.major_version = major;
        }
        let found = exec_text(
            &vswhere,
            &[
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.Component.MSBuild",
                "-find",
                "MSBuild\\**\\Bin\\MSBuild.exe",
            ],
        )
        .await;
        tools.msbuild = found.as_deref().and_then(parse::vswhere_msbuild).map(PathBuf::from);
    }
    tools.cargo = cargo_candidates(&system_env).into_iter().find(|c| is_file(c));
    tools
}

/// Any Visual Studio, with or without the C++ workload (the Requirements tab says which is missing).
pub async fn any_visual_studio() -> Option<String> {
    let vswhere = vswhere_path(&system_env);
    if !cfg!(windows) || !is_file(&vswhere) {
        return None;
    }
    let json = exec_text(&vswhere, &["-latest", "-products", "*", "-format", "json", "-utf8"]).await?;
    parse::vswhere_install(&json).map(|(name, _)| name)
}

/// Release x64 with the toolset this Visual Studio has.
pub fn msbuild_args(project: &Path, toolset: &str) -> Vec<String> {
    vec![
        project.to_string_lossy().into_owned(),
        "/m".into(),
        "/p:Configuration=Release".into(),
        "/p:Platform=x64".into(),
        format!("/p:PlatformToolset={toolset}"),
        "/nologo".into(),
        "/verbosity:minimal".into(),
    ]
}

fn not_configured() -> HostServerConfig {
    HostServerConfig {
        configured: false,
        name: None,
        mode: None,
        host: None,
        multipliers: None,
    }
}

/// The server's `host.json` as the Host page shows it (Reforged's `readServerConfig`).
pub fn read_server_config(root: &Path) -> HostServerConfig {
    let path = WorkspacePaths::new(root).host_json;
    // host.json is small; a huge one is not the host kit's.
    let text = match path.metadata() {
        Ok(meta) if meta.len() <= 1024 * 1024 => std::fs::read_to_string(&path).ok(),
        _ => None,
    };
    let Some(config) = text.as_deref().and_then(parse::server_config) else {
        return not_configured();
    };
    HostServerConfig {
        configured: true,
        name: config.name,
        mode: Some(match config.mode {
            ServerMode::Private => HostServerMode::Private,
            ServerMode::Public => HostServerMode::Public,
        }),
        host: config.host,
        multipliers: config.multipliers.map(|(xp, rewards)| HostMultipliers { xp, rewards }),
    }
}

/// What the Host page shows of the workspace (Reforged's `detectHostKit`). The controller adds the
/// Dumper-7 DLL, the requirements, the job, the last update and where the project goes.
pub fn detect_host_kit(workspace: Option<&Path>, toolchain: &Toolchain, dump_root: &Path) -> HostKit {
    let mut kit = HostKit {
        workspace: workspace.map(|w| w.to_string_lossy().into_owned()),
        workspace_ok: false,
        upstream: HostUpstream {
            present: false,
            patched: false,
        },
        sdk: HostSdk {
            present: false,
            function_files: 0,
        },
        dumps: find_dumps(dump_root),
        dumper_dll: None,
        builds: HostBuilds {
            runtime: false,
            loader: false,
            exporter: false,
            published: false,
        },
        toolchain: toolchain.view(),
        game_data: HostGameData {
            backend: GameDataState::Missing,
            director: GameDataState::Missing,
        },
        exports: HostExports {
            present: false,
            in_game: false,
        },
        server_config: not_configured(),
        requirements: Vec::new(),
        job: None,
        update: None,
        project_target: None,
    };
    let Some(root) = workspace else {
        return kit;
    };
    let w = WorkspacePaths::new(root);
    kit.workspace_ok = is_workspace(root);
    // Mystic Paradox is part of the checkout (RD1): its patches are commits, so present is patched.
    kit.upstream.present = is_file(&w.runtime_source.join("dllmain.cpp"));
    kit.upstream.patched = kit.upstream.present;
    kit.sdk.present = is_file(&w.sdk_header);
    kit.sdk.function_files = count_function_files(&w.sdk_dir);
    kit.builds.runtime = is_file(&w.runtime_dll);
    kit.builds.loader = is_file(&w.loader_dll);
    kit.builds.exporter = is_file(&w.exporter_dll);
    kit.builds.published = RUNTIME_FILE_NAMES.iter().all(|n| is_file(&w.publish.join(n)));
    kit.game_data.backend = detect_game_data(&w.backend_game_data, &BACKEND_GAME_DATA);
    kit.game_data.director = detect_game_data(&w.director_game_data, &DIRECTOR_GAME_DATA);
    kit.exports.present = is_dir(&w.exports);
    kit.server_config = read_server_config(root);
    kit
}

// ------------------------------------------------------------------ file steps

/// Copies a folder tree: files and folders only, links skipped. Returns how many files.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u32> {
    std::fs::create_dir_all(to)?;
    let mut count = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        // DirEntry's type does not follow links.
        let kind = entry.file_type()?;
        let (a, b) = (entry.path(), to.join(entry.file_name()));
        if kind.is_dir() {
            count += copy_tree(&a, &b)?;
        } else if kind.is_file() {
            std::fs::copy(&a, &b)?;
            count += 1;
        }
    }
    Ok(count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkError {
    /// The folder has no SDK.hpp
    NoHeader,
    /// The runtime source (dllmain.cpp) is not there
    NoRuntimeSource,
    /// No *_functions.cpp arrived
    NoFunctions,
    Io,
}

impl SdkError {
    pub fn code(self) -> &'static str {
        match self {
            Self::NoHeader => "sdk_folder",
            Self::NoRuntimeSource => "workspace",
            Self::NoFunctions => "sdk_no_functions",
            Self::Io => "sdk_copy",
        }
    }
}

/// Copies a Dumper-7 CppSDK into the runtime source, in the layout Mystic Paradox's build expects
/// (`SDK.hpp`, `SDK\…` and the files beside `SDK.hpp`).
pub fn import_sdk(dump_dir: &Path, runtime_source: &Path) -> Result<u32, SdkError> {
    if !is_file(&dump_dir.join("SDK.hpp")) {
        return Err(SdkError::NoHeader);
    }
    if !is_file(&runtime_source.join("dllmain.cpp")) {
        return Err(SdkError::NoRuntimeSource);
    }
    let copied = copy_tree(dump_dir, runtime_source).map_err(|_| SdkError::Io)?;
    if count_function_files(&runtime_source.join("SDK")) == 0 {
        return Err(SdkError::NoFunctions);
    }
    Ok(copied)
}

/// A build's exit code and the last non-empty line it printed (at most 300 characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    pub code: i32,
    pub last_line: Option<String>,
}

/// Runs a build tool (MSBuild, cargo) with its output into `log_file`: a header with the command and
/// folder, a footer with the exit code (Reforged's `runLogged`).
pub async fn run_logged(program: &Path, args: &[String], cwd: &Path, log_file: &Path) -> Logged {
    use tokio::io::AsyncWriteExt;
    if let Some(dir) = log_file.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let mut log = tokio::fs::File::create(log_file).await.ok();
    let header = format!(
        "> {} {}\r\n(in {})\r\n\r\n",
        program.display(),
        args.join(" "),
        cwd.display()
    );
    if let Some(log) = log.as_mut() {
        let _ = log.write_all(header.as_bytes()).await;
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            if let Some(log) = log.as_mut() {
                let _ = log.write_all(format!("\r\nfailed to start: {e}\r\n").as_bytes()).await;
            }
            return Logged {
                code: -1,
                last_line: Some(parse::cut(&e.to_string(), 300)),
            };
        }
    };
    let (sender, mut chunks) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, sender.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, sender.clone()));
    }
    drop(sender);
    let mut tail = Vec::<u8>::new();
    while let Some(chunk) = chunks.recv().await {
        if let Some(log) = log.as_mut() {
            let _ = log.write_all(&chunk).await;
        }
        tail.extend_from_slice(&chunk);
        if tail.len() > 8000 {
            tail.drain(..tail.len() - 4000);
        }
    }
    let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);
    if let Some(log) = log.as_mut() {
        let _ = log.write_all(format!("\r\nexit code {code}\r\n").as_bytes()).await;
        let _ = log.flush().await;
    }
    let text = String::from_utf8_lossy(&tail);
    let last_line = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| parse::cut(line, 300));
    Logged { code, last_line }
}

/// A pipe's bytes, chunk by chunk, until it ends.
pub(crate) async fn pump<R: tokio::io::AsyncRead + Unpin>(
    mut pipe: R,
    to: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    let mut buf = vec![0u8; 8192];
    while let Ok(n) = pipe.read(&mut buf).await {
        if n == 0 || to.send(buf[..n].to_vec()).is_err() {
            break;
        }
    }
}

/// The Host page's test stand-in (`fixtures/host`): built once per test run with this toolchain.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    fn out_dir() -> &'static Path {
        static BUILT: OnceLock<PathBuf> = OnceLock::new();
        BUILT.get_or_init(|| {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            let target = manifest.join("..").join("..").join("target").join("host-fixtures");
            let status = std::process::Command::new(env!("CARGO"))
                .args(["build", "--quiet", "--locked", "--manifest-path"])
                .arg(manifest.join("fixtures").join("host").join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target)
                .status()
                .expect("cargo runs");
            assert!(status.success(), "the host fixtures build");
            target.join("debug")
        })
    }

    /// `fake-host-kit`.
    pub fn exe(name: &str) -> PathBuf {
        out_dir().join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(file: &Path, data: &[u8]) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, data).unwrap();
    }

    #[test]
    fn catalog_exporter_flags_are_the_catalog_slayers_path_hunts_store_and_progression() {
        let on: Vec<&str> = EXPORT_FLAGS
            .split("\r\n")
            .filter_map(|l| l.strip_suffix("=1"))
            .collect();
        assert_eq!(
            on,
            [
                "EXPORT_CATALOG",
                "EXPORT_SLAYERS_PATH",
                "EXPORT_HUNTS",
                "EXPORT_STORE_RUNTIME",
                "EXPORT_PROGRESSION"
            ]
        );
        assert!(EXPORT_FLAGS.contains("EXPORT_SKINS_RESOLVE_STRINGTABLE=0"));
        assert!(EXPORT_FLAGS.ends_with("EXPORT_CURRENCY_NAMES=0\r\n"));
        assert!(!EXPORT_FLAGS.replace("\r\n", "").contains('\n'), "CRLF only");
    }

    #[test]
    fn workspace_paths_follow_the_checkout_mystic_paradox_in_paradox_and_native() {
        let root = Path::new("rr");
        let w = WorkspacePaths::new(root);
        assert_eq!(
            w.runtime_dll,
            join(
                root,
                &["native", "paradox-runtime", "x64", "Release", "MysticParadox.dll"]
            )
        );
        assert_eq!(
            w.loader_dll,
            join(root, &["native", "runtime-loader", "target", "release", "winmm.dll"])
        );
        assert_eq!(
            w.exporter_dll,
            join(
                root,
                &["native", "catalog-exporter", "x64", "Release", "CatalogExporter.dll"]
            )
        );
        assert_eq!(w.backend_game_data, join(root, &["paradox", "backend", "game-data"]));
        assert_eq!(w.exports, join(root, &["paradox", "Items_Analysis"]));
        assert_eq!(w.publish, join(root, &[".workspace", "runtime"]));
        assert_eq!(w.host_json, join(root, &[".workspace", "config", "host.json"]));
        assert_eq!(
            w.launcher_log("setup"),
            join(root, &[".workspace", "logs", "launcher-setup.log"])
        );
    }

    #[test]
    fn a_workspace_is_reforged_host_with_paradox() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        assert!(!is_workspace(root));
        std::fs::create_dir(root.join("paradox")).unwrap();
        assert!(!is_workspace(root), "no host kit");
        write(&join(root, &["rust", "target", "release", HOST_BINARY]), b"");
        assert_eq!(
            find_host_binary(root),
            Some(join(root, &["rust", "target", "release", HOST_BINARY]))
        );
        write(&root.join(HOST_BINARY), b"");
        assert_eq!(
            find_host_binary(root),
            Some(root.join(HOST_BINARY)),
            "the release's first"
        );
        assert!(is_workspace(root));
        assert!(!is_workspace(&root.join("paradox")));
        write(&root.join("legacy").join("ReforgedHost").join("package.json"), b"{}");
        std::fs::remove_file(root.join(HOST_BINARY)).unwrap();
        std::fs::remove_dir_all(root.join("rust")).unwrap();
        assert!(
            !is_workspace(root),
            "the Node kit alone is not the planned layout (RD23)"
        );
    }

    #[test]
    fn game_data_missing_synthetic_and_real() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert_eq!(detect_game_data(dir, &BACKEND_GAME_DATA), GameDataState::Missing);
        for name in BACKEND_GAME_DATA {
            let example = format!("{{\"example\":\"{name}\"}}");
            write(&dir.join(name.replace(".json", ".example.json")), example.as_bytes());
            write(&dir.join(name), example.as_bytes());
        }
        assert_eq!(detect_game_data(dir, &BACKEND_GAME_DATA), GameDataState::Synthetic);
        write(&dir.join(BACKEND_GAME_DATA[0]), b"{\"real\":true}");
        assert_eq!(
            detect_game_data(dir, &BACKEND_GAME_DATA),
            GameDataState::Synthetic,
            "one example left"
        );
        for name in BACKEND_GAME_DATA {
            write(&dir.join(name), format!("{{\"real\":\"{name}\"}}").as_bytes());
        }
        assert_eq!(detect_game_data(dir, &BACKEND_GAME_DATA), GameDataState::Real);
    }

    #[test]
    fn dumper_output_the_newest_cppsdk_is_found_and_imported_into_the_runtime_source() {
        let root = tempfile::tempdir().unwrap();
        let older = join(root.path(), &["4.26.2-647472-Dauntless-old", "CppSDK"]);
        let newer = join(root.path(), &["4.26.2-647472-Dauntless", "CppSDK"]);
        write(&older.join("SDK.hpp"), b"// old");
        write(&newer.join("SDK.hpp"), b"// new");
        write(&join(&newer, &["SDK", "Engine_functions.cpp"]), b"// fn");
        write(&join(&newer, &["SDK", "Engine_classes.hpp"]), b"// classes");
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(older.join("SDK.hpp"))
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
        let dumps = find_dumps(root.path());
        assert_eq!(dumps.len(), 2);
        assert_eq!(dumps[0].dir, newer.to_string_lossy());
        assert_eq!(dumps[0].name, "4.26.2-647472-Dauntless");
        assert!(dumps[0].modified.ends_with('Z'));
        assert!(find_dumps(&root.path().join("none")).is_empty());

        let ws = tempfile::tempdir().unwrap();
        let w = WorkspacePaths::new(ws.path());
        assert_eq!(import_sdk(&newer, &w.runtime_source), Err(SdkError::NoRuntimeSource));
        assert_eq!(
            import_sdk(&older.join("none"), &w.runtime_source),
            Err(SdkError::NoHeader)
        );
        write(&w.runtime_source.join("dllmain.cpp"), b"// runtime");
        assert!(import_sdk(&newer, &w.runtime_source).unwrap() >= 3);
        assert_eq!(std::fs::read_to_string(&w.sdk_header).unwrap(), "// new");
        assert_eq!(count_function_files(&w.sdk_dir), 1);
        let empty = tempfile::tempdir().unwrap();
        let w2 = WorkspacePaths::new(empty.path());
        write(&w2.runtime_source.join("dllmain.cpp"), b"//");
        assert_eq!(import_sdk(&older, &w2.runtime_source), Err(SdkError::NoFunctions));
    }

    #[test]
    fn detect_host_kit_reads_a_workspace_without_a_toolchain() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join(HOST_BINARY), b"");
        let w = WorkspacePaths::new(root);
        write(&w.runtime_source.join("dllmain.cpp"), b"//");
        std::fs::create_dir_all(&w.backend_game_data).unwrap();
        let kit = detect_host_kit(Some(root), &Toolchain::default(), &root.join("no-dumps"));
        assert!(kit.workspace_ok);
        assert_eq!(
            kit.upstream,
            HostUpstream {
                present: true,
                patched: true
            }
        );
        assert_eq!(
            kit.sdk,
            HostSdk {
                present: false,
                function_files: 0
            }
        );
        assert!(!kit.builds.published);
        assert_eq!(
            kit.toolchain,
            HostToolchain {
                visual_studio: None,
                msbuild: false,
                cargo: false
            }
        );
        assert!(!kit.server_config.configured);

        write(
            &w.host_json,
            br#"{"name":"Friday","mode":"private","multipliers":{"xp":5,"rewards":2}}"#,
        );
        for name in RUNTIME_FILE_NAMES {
            write(&w.publish.join(name), b"x");
        }
        let kit = detect_host_kit(Some(root), &Toolchain::default(), &root.join("no-dumps"));
        assert!(kit.builds.published);
        assert_eq!(kit.server_config.mode, Some(HostServerMode::Private));
        assert_eq!(
            kit.server_config.multipliers,
            Some(HostMultipliers { xp: 5.0, rewards: 2.0 })
        );

        let none = detect_host_kit(None, &Toolchain::default(), &root.join("no-dumps"));
        assert!(!none.workspace_ok && none.workspace.is_none());
    }

    #[test]
    fn build_settings_release_x64_with_the_toolset() {
        let args = msbuild_args(Path::new("C:\\x\\MysticParadox.sln"), parse::platform_toolset(Some(17)));
        for flag in [
            "/p:Configuration=Release",
            "/p:Platform=x64",
            "/p:PlatformToolset=v143",
            "/m",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag}");
        }
        let env = |name: &str| match name {
            "USERPROFILE" => Some(OsString::from("C:\\Users\\h")),
            "CARGO_HOME" => Some(OsString::from("D:\\cargo")),
            _ => None,
        };
        assert_eq!(
            cargo_candidates(&env),
            [
                join(Path::new("C:\\Users\\h"), &[".cargo", "bin", "cargo.exe"]),
                join(Path::new("D:\\cargo"), &["bin", "cargo.exe"])
            ]
        );
        assert_eq!(
            vswhere_path(&|_| None),
            join(
                Path::new("C:\\Program Files (x86)"),
                &["Microsoft Visual Studio", "Installer", "vswhere.exe"]
            )
        );
    }

    #[test]
    fn copy_tree_copies_files_and_folders() {
        let from = tempfile::tempdir().unwrap();
        write(&join(from.path(), &["a", "b.txt"]), b"b");
        write(&from.path().join("c.txt"), b"c");
        let to = tempfile::tempdir().unwrap();
        assert_eq!(copy_tree(from.path(), &to.path().join("out")).unwrap(), 2);
        assert_eq!(std::fs::read(join(to.path(), &["out", "a", "b.txt"])).unwrap(), b"b");
    }

    #[tokio::test]
    async fn run_logged_keeps_the_log_and_the_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = join(dir.path(), &["logs", "build.log"]);
        let (program, args): (PathBuf, Vec<String>) = if cfg!(windows) {
            (
                crate::platform::launch::system_root().join("System32").join("cmd.exe"),
                vec!["/c".into(), "echo building& echo done 1>&2& exit 3".into()],
            )
        } else {
            (
                "/bin/sh".into(),
                vec!["-c".into(), "echo building; echo done >&2; exit 3".into()],
            )
        };
        let r = run_logged(&program, &args, dir.path(), &log).await;
        assert_eq!(r.code, 3);
        assert_eq!(r.last_line.as_deref(), Some("done"));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("> "), "{text}");
        assert!(text.contains("building") && text.contains("exit code 3"), "{text}");

        let missing = run_logged(&dir.path().join("nope.exe"), &[], dir.path(), &log).await;
        assert_eq!(missing.code, -1);
        assert!(missing.last_line.is_some());
    }
}
