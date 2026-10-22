//! The Host page (Reforged's `controller.ts` 1765–2351): the host kit's workspace, requirements, builds,
//! game data and the server a host runs from the launcher, for the planned layout only (RD23), on
//! `platform::hostkit`, `platform::hostserver`, `platform::requirements` and
//! `platform::loader_session`, driving `reforged-host`.
//!
//! Dumper-7 and CatalogExporter are loaded by the runtime's own loader through its `mystic_loader.ini`
//! (RD9): a session places the DLL and the ini, starts the game, and `mod.rs` ends the session when that
//! game exits. Nothing here touches another process.
//!
//! Not ported (RD23): the Node kit's paths (`npm run upstream`, `npm install`), git's clone and fast
//! forward. The project folder is a server release (or a checkout with `reforged-host` built); when the
//! one beside the game is not there, the page says so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Controller, State, lock};
use crate::bridge::types::{
    ActionResult, ErrorCode, GameDataState, GameProblem, HostAction, HostFolder, HostInvite, HostJob, HostJobKind,
    HostJobState, HostKit, HostMultipliers, HostServerMode, HostServerState, HostServerView, HostSetupRequest,
    HostUpdateState, HostUpdateView, NoticeCode, Requirement,
};
use crate::platform::dialogs::FolderPurpose;
use crate::platform::hostkit::parse::{
    invite_line, last_json, pack_written, platform_toolset, published_version, update_check, update_done,
    update_failure,
};
use crate::platform::hostkit::{
    DUMPER_OUTPUT_ROOT, EXPORT_FLAGS, WorkspacePaths, copy_tree, detect_host_kit, find_dumps, find_toolchain,
    import_sdk, is_workspace, msbuild_args, read_server_config, run_logged,
};
use crate::platform::hostserver::{CommandOutput, HostKitError, HostServer, HostTool, Probe, STOP_TIMEOUT};
use crate::platform::loader_session::{self, EXPORTER_DLL, LoaderSession, RECORD_FILE, SessionError, SessionKind};
use crate::platform::log;
use crate::platform::requirements::{Inputs, check_requirements_here};
use crate::platform::settings::Platform as HostPlatform;

/// Where a downloaded project goes beside the game folder (Reforged's `PROJECT_FOLDER_NAME`).
const PROJECT_FOLDER_NAME: &str = "Reforged";
/// The requirements are looked at again after this long (vswhere and friends are slow).
const REQUIREMENTS_TTL: Duration = Duration::from_secs(15);
/// What the Dumper-7 game starts with: no anti-cheat, no Epic or EOS sign-in.
const DUMP_ARGS: [&str; 3] = ["-NoEAC", "-EpicPortal", "-DisableEOS"];

struct CachedRequirements {
    at: Instant,
    workspace: Option<String>,
    list: Vec<Requirement>,
}

/// The Host page's own state, beside the controller's (`State` keeps what `mod.rs` acts on:
/// `host_server_up`, `dumping` and `loader_session`).
pub struct HostState {
    server: HostServer,
    job: Mutex<Option<HostJob>>,
    update: Mutex<Option<HostUpdateView>>,
    requirements: Mutex<Option<CachedRequirements>>,
    /// Where Dumper-7 writes its SDKs (`C:\Dumper-7`; tests: a folder of their own)
    dump_root: PathBuf,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            server: HostServer::default(),
            job: Mutex::new(None),
            update: Mutex::new(None),
            requirements: Mutex::new(None),
            dump_root: PathBuf::from(DUMPER_OUTPUT_ROOT),
        }
    }
}

impl HostState {
    /// The tests' seams: whether a server answers on port 61000, and where the dumps are.
    pub fn with(probe: Option<Probe>, dump_root: Option<PathBuf>) -> Self {
        let mut state = Self::default();
        if let Some(probe) = probe {
            state.server = HostServer::new(probe, STOP_TIMEOUT);
        }
        if let Some(root) = dump_root {
            state.dump_root = root;
        }
        state
    }
}

/// The steps that work in Mystic Paradox's source (`NEEDS_UPSTREAM`).
fn needs_upstream(action: HostAction) -> bool {
    matches!(
        action,
        HostAction::ImportSdk | HostAction::BuildLoader | HostAction::SyntheticData | HostAction::GenerateGamedata
    )
}

fn kind_name(kind: HostJobKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file())
}

fn is_dir(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_dir())
}

fn busy() -> ActionResult {
    ActionResult::err(ErrorCode::Busy)
}

impl Controller {
    // ------------------------------------------------------------------ seams for mod.rs

    /// Wires the Host page's server to the page and to `State::host_server_up`.
    pub(super) fn host_attach(&self) {
        let me = self.me.clone();
        self.host.server.set_listener(Arc::new(move |view: HostServerView| {
            let Some(this) = me.upgrade() else { return };
            this.st().host_server_up = matches!(
                view.state,
                HostServerState::Starting
                    | HostServerState::Running
                    | HostServerState::Stopping
                    | HostServerState::External
            );
            this.p.emit_host_server(&view);
        }));
    }

    /// The server the Host page started stops with the launcher (the page says so).
    pub(super) async fn host_shutdown(&self) {
        if self.host.server.running() {
            self.host.server.stop().await;
            log::info("host: server stopped with the launcher");
        }
    }

    /// `ensureHostWorkspace`: the project folder a host's server runs from. With Host tools on and none
    /// chosen yet, the one beside the game is taken when it is there. `auto` never shows an error.
    pub(super) async fn ensure_host_workspace(&self, auto: bool) -> ActionResult {
        if auto && !self.s().host_tools {
            return ActionResult::Ok;
        }
        if self.workspace().is_some() {
            return ActionResult::Ok;
        }
        if self.job_running() {
            return busy();
        }
        let quiet = |detail: &str| {
            if auto { ActionResult::Ok } else { self.host_fail(detail) }
        };
        let Some(target) = self.project_target() else {
            return quiet("project_no_game");
        };
        if !is_workspace(&target) {
            return quiet(if target.exists() {
                "project_folder_taken"
            } else {
                "project_missing"
            });
        }
        let text = path_text(&target);
        self.update_settings(|map| super::stored::set_text(map, "hostWorkspace", Some(&text)))
            .await;
        log::info(&format!("host: project folder {text}"));
        self.emit_host_kit();
        self.publish();
        ActionResult::Ok
    }

    /// Where the session record is kept.
    fn loader_record(&self) -> PathBuf {
        self.c.user_data.join(RECORD_FILE)
    }

    /// The Host tools' DLL names a PLAY refuses beside the loader: the host's Dumper-7 and CatalogExporter.
    fn loader_known(&self) -> Vec<String> {
        let mut known = vec![EXPORTER_DLL.to_owned()];
        if let Some(dll) = self.s().dumper_dll {
            known.push(file_name(Path::new(&dll)));
        }
        known
    }

    /// The pre-launch check (RD9): what must not be beside the loader for an ordinary PLAY.
    pub(super) fn loader_leftovers(&self, win64: &Path) -> Vec<String> {
        let known = self.loader_known();
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        loader_session::leftovers(win64, &self.loader_record(), &known)
    }

    /// Begins a loader session in `win64` for `dll`; the page's error when it cannot.
    pub(super) fn begin_loader_session(
        &self,
        kind: SessionKind,
        win64: &Path,
        dll: &Path,
    ) -> Result<LoaderSession, crate::bridge::types::LauncherError> {
        let known = self.loader_known();
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        loader_session::begin(kind, win64, dll, &self.loader_record(), &known).map_err(|error| {
            log::warn(&format!("host: loader session not begun: {}", error.detail()));
            match error {
                SessionError::Leftovers(files) => self.fail(ErrorCode::LoaderLeftovers, Some(files.join(", "))),
                other => self.fail(ErrorCode::HostFailed, Some(other.detail())),
            }
        })
    }

    /// Ends a session: the ini and the DLL go, the runtime comes back. A DLL the game just let go of may
    /// still be held for a moment, so it is tried a few times.
    pub(super) async fn end_loader_session(&self, session: LoaderSession) {
        let mut last = None;
        for attempt in 0..5 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            let try_end = session.clone();
            match tokio::task::spawn_blocking(move || try_end.end()).await {
                Ok(Ok(())) => {
                    log::info(&format!(
                        "host: {} and {} removed",
                        loader_session::LOADER_INI,
                        session.dll
                    ));
                    return;
                }
                Ok(Err(error)) => last = Some(log::describe_error(&error)),
                Err(error) => last = Some(error.to_string()),
            }
        }
        log::error(&format!(
            "host: the loader session's files were not removed ({}); PLAY refuses until they are",
            last.unwrap_or_default()
        ));
    }

    /// A session a closed launcher did not finish is undone at the next start.
    pub(super) async fn recover_loader_session(&self) {
        let record = self.loader_record();
        match tokio::task::spawn_blocking(move || loader_session::recover(&record)).await {
            Ok(Some(Ok(()))) => log::info("host: an unfinished loader session was undone"),
            Ok(Some(Err(error))) => log::warn(&format!(
                "host: an unfinished loader session could not be undone: {}",
                log::describe_error(&error)
            )),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ helpers

    fn host_fail(&self, detail: &str) -> ActionResult {
        ActionResult::Err(self.fail(ErrorCode::HostFailed, Some(detail.to_owned())))
    }

    fn host_kit_failure(&self, error: &HostKitError) -> ActionResult {
        log::error(&format!("host kit: {}", error.detail()));
        self.host_fail(&error.detail())
    }

    /// The workspace in the settings when it still is one.
    fn workspace(&self) -> Option<PathBuf> {
        self.s().host_workspace.map(PathBuf::from).filter(|ws| is_workspace(ws))
    }

    fn job_running(&self) -> bool {
        lock(&self.host.job)
            .as_ref()
            .is_some_and(|job| job.state == HostJobState::Running)
    }

    fn set_job(&self, job: Option<HostJob>) {
        *lock(&self.host.job) = job;
        self.emit_host_kit();
    }

    fn start_job(&self, kind: HostJobKind, message: Option<String>, log_file: Option<&Path>) {
        self.set_job(Some(HostJob {
            kind,
            state: HostJobState::Running,
            message,
            log_file: log_file.map(path_text),
        }));
    }

    fn finish_job(
        &self,
        kind: HostJobKind,
        ok: bool,
        message: Option<String>,
        log_file: Option<&Path>,
    ) -> ActionResult {
        self.set_job(Some(HostJob {
            kind,
            state: if ok { HostJobState::Done } else { HostJobState::Failed },
            message: message.clone(),
            log_file: log_file.map(path_text),
        }));
        if ok {
            self.st().notice = Some(if kind == HostJobKind::ImportSdk {
                NoticeCode::SdkImported
            } else {
                NoticeCode::HostDone
            });
            self.publish();
            return ActionResult::Ok;
        }
        let detail = message.unwrap_or_else(|| kind_name(kind));
        ActionResult::Err(self.fail(ErrorCode::HostFailed, Some(detail)))
    }

    /// The page hears of the kit after a change (in the background: the toolchain check is slow).
    fn emit_host_kit(&self) {
        let Some(this) = self.me.upgrade() else { return };
        self.rt.spawn(async move {
            let kit = this.host_kit().await;
            this.p.emit_host_kit(&kit);
        });
    }

    /// Where the project folder goes: beside the game's folder (`D:\Games\Dauntless` ->
    /// `D:\Games\Reforged`). None without a game folder.
    fn project_target(&self) -> Option<PathBuf> {
        let st = self.st();
        if matches!(st.game.problem, Some(GameProblem::NotFound | GameProblem::NotSet)) {
            return None;
        }
        let root = PathBuf::from(st.game.root.as_deref()?);
        root.parent().map(|parent| parent.join(PROJECT_FOLDER_NAME))
    }

    fn game_ok(st: &State) -> bool {
        st.game.root.is_some()
            && !matches!(
                st.game.problem,
                Some(GameProblem::NotFound | GameProblem::NotSet | GameProblem::WrongVersion)
            )
    }

    fn tool(&self, ws: &Path) -> Result<HostTool, ActionResult> {
        HostTool::find(ws).ok_or_else(|| self.host_kit_failure(&HostKitError::Missing))
    }

    /// A one-shot `reforged-host` command in the workspace.
    async fn run_host(&self, ws: &Path, args: &[&str], log_name: &str) -> Result<CommandOutput, ActionResult> {
        let tool = self.tool(ws)?;
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.host
            .server
            .command(&tool, ws, &args, log_name)
            .await
            .map_err(|error| self.host_kit_failure(&error))
    }

    /// A server this launcher runs is stopped for a change and started again after it; one started in a
    /// terminal (`external`) cannot be stopped from here, so the callers refuse it first.
    async fn while_stopped<T>(&self, ws: &Path, work: impl Future<Output = T>) -> T {
        let was_running = self.host.server.running();
        if was_running {
            self.host.server.stop().await;
        }
        let out = work.await;
        if was_running {
            match HostTool::find(ws) {
                Some(tool) => {
                    if let Err(error) = self.host.server.start(&tool, ws).await {
                        log::warn(&format!("host: server not started again: {}", error.detail()));
                    }
                }
                None => log::warn("host: server not started again: no reforged-host"),
            }
        }
        out
    }

    // ------------------------------------------------------------------ the page's commands

    /// `hostGet`: the requirements are looked at afresh after 15 s.
    pub async fn host_kit(&self) -> HostKit {
        let toolchain = find_toolchain().await;
        let s = self.s();
        let workspace = s.host_workspace.clone();
        let mut kit = detect_host_kit(workspace.as_deref().map(Path::new), &toolchain, &self.host.dump_root);
        let cached = {
            let cache = lock(&self.host.requirements);
            cache
                .as_ref()
                .filter(|c| c.workspace == workspace && c.at.elapsed() < REQUIREMENTS_TTL)
                .map(|c| c.list.clone())
        };
        kit.requirements = match cached {
            Some(list) => list,
            None => {
                let (game_ok, version) = {
                    let st = self.st();
                    (Self::game_ok(&st), st.game.version_text.clone())
                };
                let dumper = s.dumper_dll.as_deref().map(PathBuf::from);
                let inputs = Inputs {
                    game: (game_ok, version),
                    toolchain: &toolchain,
                    dumper_dll: dumper.as_deref(),
                    tailscale: self.p.find_tailscale(),
                    missing_vc_runtime: self.p.missing_runtimes().0.len(),
                };
                let list = check_requirements_here(&inputs).await;
                *lock(&self.host.requirements) = Some(CachedRequirements {
                    at: Instant::now(),
                    workspace,
                    list: list.clone(),
                });
                list
            }
        };
        let win64 = Self::win64_dir_of(&self.st());
        kit.exports.in_game = win64.is_some_and(|dir| is_dir(&dir.join("Items_Analysis")));
        kit.dumper_dll = s.dumper_dll;
        kit.job = lock(&self.host.job).clone();
        kit.update = lock(&self.host.update).clone();
        kit.project_target = self.project_target().map(|p| path_text(&p));
        kit
    }

    pub async fn host_choose_workspace(&self) -> ActionResult {
        let start = self.s().host_workspace.map(PathBuf::from).unwrap_or_default();
        let Some(picked) = self
            .p
            .choose_folder(self.language(), FolderPurpose::Workspace, start)
            .await
        else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        if !is_workspace(&picked) {
            return self.host_fail("workspace");
        }
        let text = path_text(&picked);
        self.update_settings(|map| super::stored::set_text(map, "hostWorkspace", Some(&text)))
            .await;
        *lock(&self.host.requirements) = None;
        self.emit_host_kit();
        self.publish();
        ActionResult::Ok
    }

    pub async fn host_choose_dumper(&self) -> ActionResult {
        let Some(picked) = self.p.choose_dll().await else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        let dll = picked.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll"));
        if !dll || !is_file(&picked) {
            return self.host_fail("dumper");
        }
        let text = path_text(&picked);
        self.update_settings(|map| super::stored::set_text(map, "dumperDll", Some(&text)))
            .await;
        *lock(&self.host.requirements) = None;
        self.emit_host_kit();
        ActionResult::Ok
    }

    /// A game data pack the host exported earlier (`reforged-host gamedata --import`).
    pub async fn host_import_gamedata(&self) -> ActionResult {
        if self.job_running() {
            return busy();
        }
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        let Some(file) = self.p.choose_pack().await else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        let name = file_name(&file);
        let log_file = WorkspacePaths::new(&ws).launcher_log("gamedata-import");
        self.start_job(HostJobKind::ImportGamedata, Some(name.clone()), Some(&log_file));
        let file_arg = path_text(&file);
        let r = match self
            .run_host(&ws, &["gamedata", "--import", &file_arg], "gamedata-import")
            .await
        {
            Ok(r) => r,
            Err(failed) => {
                self.set_job(None);
                return failed;
            }
        };
        if r.code == 0 {
            log::info(&format!("host: game data pack {name} imported"));
        }
        let message = if r.code == 0 { Some(name) } else { r.failure };
        self.finish_job(HostJobKind::ImportGamedata, r.code == 0, message, Some(&log_file))
    }

    /// A Dumper-7 output that is not under `C:\Dumper-7`: the CppSDK folder itself, or the folder that
    /// holds it.
    pub async fn host_import_sdk_folder(&self) -> ActionResult {
        if self.job_running() {
            return busy();
        }
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        let Some(picked) = self
            .p
            .choose_folder(self.language(), FolderPurpose::Sdk, self.host.dump_root.clone())
            .await
        else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        let dir = if is_file(&picked.join("SDK.hpp")) {
            picked
        } else if is_file(&picked.join("CppSDK").join("SDK.hpp")) {
            picked.join("CppSDK")
        } else {
            return self.host_fail("sdk_folder");
        };
        if let Err(failed) = self.ensure_upstream(&ws).await {
            return failed;
        }
        let name = dir
            .parent()
            .map(file_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| file_name(&dir));
        self.import_sdk_from(&ws, dir, name).await
    }

    /// `ensureHostWorkspace` from the page's button: errors are shown.
    pub async fn host_download_project(&self) -> ActionResult {
        self.ensure_host_workspace(false).await
    }

    pub async fn host_action(&self, action: HostAction) -> ActionResult {
        if self.job_running() {
            return busy();
        }
        match action {
            HostAction::InjectDumper => return self.start_dump().await,
            HostAction::InjectExporter => return self.start_export().await,
            _ => {}
        }
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        let w = WorkspacePaths::new(&ws);
        if needs_upstream(action)
            && let Err(failed) = self.ensure_upstream(&ws).await
        {
            return failed;
        }
        match action {
            HostAction::InjectDumper | HostAction::InjectExporter => unreachable!("handled above"),
            HostAction::ImportSdk => {
                let Some(dump) = find_dumps(&self.host.dump_root).into_iter().next() else {
                    return self.host_fail("no_dump");
                };
                self.import_sdk_from(&ws, PathBuf::from(dump.dir), dump.name).await
            }
            HostAction::BuildRuntime | HostAction::BuildExporter => {
                let tools = find_toolchain().await;
                let Some(msbuild) = tools.msbuild else {
                    return self.host_fail("msbuild");
                };
                let (kind, project, name) = if action == HostAction::BuildRuntime {
                    (HostJobKind::BuildRuntime, w.runtime_solution.clone(), "build_runtime")
                } else {
                    (HostJobKind::BuildExporter, w.exporter_project.clone(), "build_exporter")
                };
                let log_file = w.logs.join(format!("{name}.log"));
                self.start_job(kind, None, Some(&log_file));
                let cwd = project.parent().map_or_else(|| ws.clone(), Path::to_path_buf);
                let args = msbuild_args(&project, platform_toolset(tools.major_version));
                let r = run_logged(&msbuild, &args, &cwd, &log_file).await;
                self.finish_job(kind, r.code == 0, r.last_line, Some(&log_file))
            }
            HostAction::BuildLoader => {
                let Some(cargo) = find_toolchain().await.cargo else {
                    return self.host_fail("cargo");
                };
                let log_file = w.logs.join("build_loader.log");
                self.start_job(HostJobKind::BuildLoader, None, Some(&log_file));
                let args = ["build".to_owned(), "--release".to_owned()];
                let r = run_logged(&cargo, &args, &w.loader_dir, &log_file).await;
                self.finish_job(HostJobKind::BuildLoader, r.code == 0, r.last_line, Some(&log_file))
            }
            HostAction::CopyExports => {
                let Some(from) = self.game_exports() else {
                    return self.host_fail("no_exports");
                };
                self.start_job(HostJobKind::CopyExports, None, None);
                let to = w.exports.clone();
                match tokio::task::spawn_blocking(move || copy_tree(&from, &to)).await {
                    Ok(Ok(n)) => self.finish_job(HostJobKind::CopyExports, true, Some(format!("{n} files")), None),
                    Ok(Err(error)) => {
                        self.finish_job(HostJobKind::CopyExports, false, Some(log::describe_error(&error)), None)
                    }
                    Err(error) => self.finish_job(HostJobKind::CopyExports, false, Some(error.to_string()), None),
                }
            }
            HostAction::SyntheticData => {
                let log_file = w.launcher_log("gamedata");
                self.start_job(HostJobKind::SyntheticData, None, Some(&log_file));
                self.one_shot(
                    HostJobKind::SyntheticData,
                    &ws,
                    &["gamedata", "--examples"],
                    "gamedata",
                    None,
                )
                .await
            }
            HostAction::PublishRuntime => {
                let log_file = w.launcher_log("publish-runtime");
                self.start_job(HostJobKind::PublishRuntime, None, Some(&log_file));
                let r = match self.run_host(&ws, &["publish-runtime"], "publish-runtime").await {
                    Ok(r) => r,
                    Err(failed) => {
                        self.set_job(None);
                        return failed;
                    }
                };
                let version = published_version(&r.lines);
                if r.code == 0 {
                    log::info(&format!(
                        "host: runtime {} published to the content server",
                        version.as_deref().unwrap_or("?")
                    ));
                }
                let message = if r.code == 0 { version } else { r.failure };
                self.finish_job(HostJobKind::PublishRuntime, r.code == 0, message, Some(&log_file))
            }
            HostAction::GenerateGamedata => self.generate_game_data(&ws).await,
            HostAction::ExportGamedata => {
                let log_file = w.launcher_log("gamedata-export");
                self.start_job(HostJobKind::ExportGamedata, None, Some(&log_file));
                let r = match self.run_host(&ws, &["gamedata", "--export"], "gamedata-export").await {
                    Ok(r) => r,
                    Err(failed) => {
                        self.set_job(None);
                        return failed;
                    }
                };
                let written = pack_written(&r.lines).map(|file| file_name(Path::new(&file)));
                let message = if r.code == 0 { written } else { r.failure };
                self.finish_job(HostJobKind::ExportGamedata, r.code == 0, message, Some(&log_file))
            }
        }
    }

    pub fn host_open(&self, folder: HostFolder) -> ActionResult {
        let ws = self.s().host_workspace.map(PathBuf::from);
        let w = ws.as_deref().map(WorkspacePaths::new);
        let dir = match folder {
            HostFolder::Workspace => ws.clone(),
            HostFolder::RuntimeSource => w.as_ref().map(|w| w.runtime_source.clone()),
            HostFolder::Dumps => Some(self.host.dump_root.clone()),
            HostFolder::GameData => w
                .as_ref()
                .and_then(|w| w.backend_game_data.parent().map(Path::to_path_buf)),
            HostFolder::RuntimePublish => w.as_ref().map(|w| w.publish.clone()),
            HostFolder::HostLogs => w.as_ref().map(|w| w.logs.clone()),
            HostFolder::GamedataPacks => w.as_ref().map(|w| w.gamedata_packs.clone()),
        };
        match dir.filter(|dir| is_dir(dir)) {
            Some(dir) => {
                self.p.open_folder(&dir);
                ActionResult::Ok
            }
            None => self.host_fail("folder"),
        }
    }

    /// The server's view now; whether one started elsewhere answers is looked at in the background and
    /// reaches the page as a `host-server` event.
    pub fn host_server_view(&self) -> HostServerView {
        let server = self.host.server.clone();
        self.rt.spawn(async move { server.refresh().await });
        self.host.server.view()
    }

    pub async fn host_server_start(&self) -> ActionResult {
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        if self.job_running() {
            return busy();
        }
        let tool = match self.tool(&ws) {
            Ok(tool) => tool,
            Err(failed) => return failed,
        };
        let toolchain = crate::platform::hostkit::Toolchain::default();
        let kit = detect_host_kit(Some(&ws), &toolchain, &self.host.dump_root);
        if !kit.server_config.configured {
            return self.host_fail("not_configured");
        }
        if kit.game_data.backend == GameDataState::Missing || kit.game_data.director == GameDataState::Missing {
            return self.host_fail("gamedata_missing");
        }
        match self.host.server.start(&tool, &ws).await {
            Ok(()) => {
                log::info("host: server start requested from the Host page");
                ActionResult::Ok
            }
            Err(error) => self.host_kit_failure(&error),
        }
    }

    pub async fn host_server_stop(&self) -> ActionResult {
        self.host.server.stop().await;
        log::info("host: server stopped from the Host page");
        ActionResult::Ok
    }

    /// `reforged-host setup` from the page's form, already checked (`commands::host_setup_request`). The
    /// game folder is the one this launcher plays from.
    pub async fn host_setup(&self, request: HostSetupRequest) -> ActionResult {
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        if self.job_running() {
            return busy();
        }
        if self.host.server.running() || self.host.server.state() == HostServerState::External {
            return self.host_fail("server_running");
        }
        let mode = match request.mode {
            HostServerMode::Private => "private",
            HostServerMode::Public => "public",
        };
        let mut args = vec!["setup", "--mode", mode, "--name", request.name.as_str()];
        if request.mode == HostServerMode::Public
            && let Some(host) = request.public_host.as_deref()
        {
            args.extend(["--host", host]);
        }
        let root = self.st().game.root.clone();
        if let Some(root) = root.as_deref() {
            args.extend(["--game", root]);
        }
        let log_file = WorkspacePaths::new(&ws).launcher_log("setup");
        self.start_job(HostJobKind::Setup, None, Some(&log_file));
        let r = match self.run_host(&ws, &args, "setup").await {
            Ok(r) => r,
            Err(failed) => {
                self.set_job(None);
                return failed;
            }
        };
        *lock(&self.host.requirements) = None;
        let message = if r.code == 0 { Some(request.name) } else { r.failure };
        self.finish_job(HostJobKind::Setup, r.code == 0, message, Some(&log_file))
    }

    /// Who the invite is for, already trimmed and checked; None for nobody in particular. The link is a
    /// secret: it goes to the page's view only, never to the log.
    pub async fn host_create_invite(&self, for_name: Option<String>) -> ActionResult {
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        if !matches!(
            self.host.server.state(),
            HostServerState::Running | HostServerState::External
        ) {
            return self.host_fail("server_not_running");
        }
        let mut args = vec!["invite", "--json"];
        if let Some(name) = for_name.as_deref() {
            args.extend(["--for", name]);
        }
        let r = match self.run_host(&ws, &args, "invite").await {
            Ok(r) => r,
            Err(failed) => return failed,
        };
        let invite = if r.code == 0 { invite_line(&r.lines) } else { None };
        let Some(invite) = invite else {
            return self.host_fail(r.failure.as_deref().unwrap_or("invite"));
        };
        self.host.server.set_invite(Some(HostInvite {
            link: invite.link,
            for_name: invite.for_name,
        }));
        log::info(&format!(
            "host: invite created{}",
            if for_name.is_some() { " (with a note)" } else { "" }
        ));
        ActionResult::Ok
    }

    /// `reforged-host update`: compares first (a server stays up when there is nothing to update), then
    /// updates with the server stopped and runs setup again with this server's settings.
    pub async fn host_update(&self) -> ActionResult {
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        if self.job_running() {
            return busy();
        }
        if self.host.server.state() == HostServerState::External {
            return self.host_fail("server_external");
        }
        let log_file = WorkspacePaths::new(&ws).launcher_log("update");
        self.start_job(HostJobKind::Update, None, Some(&log_file));
        let check = match self.run_host(&ws, &["update", "--check", "--json"], "update").await {
            Ok(r) => r,
            Err(failed) => {
                self.set_job(None);
                return failed;
            }
        };
        let checked_json = last_json(&check.lines);
        let checked = checked_json.as_ref().and_then(update_check);
        let Some(checked) = checked.filter(|_| check.code == 0) else {
            let why = update_failure(checked_json.as_ref(), check.failure.as_deref());
            return self.finish_job(HostJobKind::Update, false, Some(why), Some(&log_file));
        };
        if checked.behind == 0 {
            *lock(&self.host.update) = Some(HostUpdateView {
                state: HostUpdateState::UpToDate,
                head: checked.head,
                files: 0,
                runtime_changed: false,
            });
            return self.finish_job(HostJobKind::Update, true, None, Some(&log_file));
        }
        let configured = read_server_config(&ws).configured;
        self.while_stopped(&ws, async {
            let r = match self.run_host(&ws, &["update", "--json", "--no-setup"], "update").await {
                Ok(r) => r,
                Err(failed) => {
                    self.set_job(None);
                    return failed;
                }
            };
            let done_json = last_json(&r.lines);
            let Some(done) = done_json.as_ref().and_then(update_done).filter(|_| r.code == 0) else {
                let why = update_failure(done_json.as_ref(), r.failure.as_deref());
                return self.finish_job(HostJobKind::Update, false, Some(why), Some(&log_file));
            };
            log::info(&format!("host: project updated to {} ({} files)", done.to, done.files));
            *lock(&self.host.update) = Some(HostUpdateView {
                state: HostUpdateState::Updated,
                head: done.to,
                files: done.files,
                runtime_changed: done.runtime_changed,
            });
            if configured {
                let s = match self.run_host(&ws, &["setup"], "setup").await {
                    Ok(s) => s,
                    Err(failed) => {
                        self.set_job(None);
                        return failed;
                    }
                };
                *lock(&self.host.requirements) = None;
                if s.code != 0 {
                    let why = s.failure.unwrap_or_else(|| "setup".into());
                    return self.finish_job(HostJobKind::Update, false, Some(why), Some(&log_file));
                }
            }
            self.finish_job(HostJobKind::Update, true, None, Some(&log_file))
        })
        .await
    }

    /// `reforged-host multipliers` (None turns them off). The backend reads them when it starts.
    pub async fn host_set_multipliers(&self, multipliers: Option<HostMultipliers>) -> ActionResult {
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        if self.job_running() {
            return busy();
        }
        if self.host.server.state() == HostServerState::External {
            return self.host_fail("server_external");
        }
        let config = read_server_config(&ws);
        if !config.configured {
            return self.host_fail("not_configured");
        }
        if config.mode != Some(HostServerMode::Private) {
            return self.host_fail("multipliers_public");
        }
        let (xp, rewards) = multipliers.map_or((String::new(), String::new()), |m| {
            (m.xp.to_string(), m.rewards.to_string())
        });
        let args: Vec<&str> = if multipliers.is_some() {
            vec!["multipliers", "--xp", &xp, "--rewards", &rewards]
        } else {
            vec!["multipliers", "--off"]
        };
        let log_file = WorkspacePaths::new(&ws).launcher_log("multipliers");
        self.start_job(HostJobKind::Multipliers, None, Some(&log_file));
        let r = self.while_stopped(&ws, self.run_host(&ws, &args, "multipliers")).await;
        let r = match r {
            Ok(r) => r,
            Err(failed) => {
                self.set_job(None);
                return failed;
            }
        };
        log::info(&format!(
            "host: test multipliers {}",
            multipliers.map_or_else(|| "off".to_owned(), |m| format!("XP x{}, rewards x{}", m.xp, m.rewards))
        ));
        let message = if r.code == 0 { None } else { r.failure };
        self.finish_job(HostJobKind::Multipliers, r.code == 0, message, Some(&log_file))
    }

    // ------------------------------------------------------------------ steps

    /// A host kit command as a job: done or failed on its exit code.
    async fn one_shot(
        &self,
        kind: HostJobKind,
        ws: &Path,
        args: &[&str],
        log_name: &str,
        ok_message: Option<String>,
    ) -> ActionResult {
        let log_file = WorkspacePaths::new(ws).launcher_log(log_name);
        let r = match self.run_host(ws, args, log_name).await {
            Ok(r) => r,
            Err(failed) => {
                self.set_job(None);
                return failed;
            }
        };
        let message = if r.code == 0 { ok_message } else { r.failure };
        self.finish_job(kind, r.code == 0, message, Some(&log_file))
    }

    /// Mystic Paradox's source is part of the checkout (RD1); a release without it gets it from
    /// `reforged-host upstream`, so the SDK, the loader and the game data steps do not wait for setup.
    async fn ensure_upstream(&self, ws: &Path) -> Result<(), ActionResult> {
        let w = WorkspacePaths::new(ws);
        if is_file(&w.runtime_source.join("dllmain.cpp")) {
            return Ok(());
        }
        let log_file = w.launcher_log("upstream");
        self.start_job(HostJobKind::Upstream, None, Some(&log_file));
        let r = match self.run_host(ws, &["upstream"], "upstream").await {
            Ok(r) => r,
            Err(failed) => {
                self.set_job(None);
                return Err(failed);
            }
        };
        if r.code != 0 {
            let why = r.failure.unwrap_or_else(|| "upstream".into());
            return Err(self.finish_job(HostJobKind::Upstream, false, Some(why), Some(&log_file)));
        }
        if !is_file(&w.runtime_source.join("dllmain.cpp")) {
            self.set_job(None);
            return Err(self.host_fail("upstream_old"));
        }
        log::info("host: Mystic Paradox downloaded for the Build tab");
        Ok(())
    }

    async fn import_sdk_from(&self, ws: &Path, dir: PathBuf, name: String) -> ActionResult {
        self.start_job(HostJobKind::ImportSdk, Some(name.clone()), None);
        let source = WorkspacePaths::new(ws).runtime_source;
        match tokio::task::spawn_blocking(move || import_sdk(&dir, &source)).await {
            Ok(Ok(n)) => {
                log::info(&format!("host: imported {n} SDK files from {name}"));
                self.finish_job(HostJobKind::ImportSdk, true, Some(format!("{n} files")), None)
            }
            Ok(Err(error)) => self.finish_job(HostJobKind::ImportSdk, false, Some(error.code().into()), None),
            Err(error) => self.finish_job(HostJobKind::ImportSdk, false, Some(error.to_string()), None),
        }
    }

    /// The game's `Items_Analysis` (CatalogExporter's output), when it is there.
    fn game_exports(&self) -> Option<PathBuf> {
        let dir = Self::win64_dir_of(&self.st())?.join("Items_Analysis");
        is_dir(&dir).then_some(dir)
    }

    /// The game's export into the workspace when there is one, then `reforged-host gamedata`. A running
    /// server reads the new data once it is started again.
    async fn generate_game_data(&self, ws: &Path) -> ActionResult {
        let w = WorkspacePaths::new(ws);
        let log_file = w.launcher_log("gamedata");
        self.start_job(HostJobKind::GenerateGamedata, None, Some(&log_file));
        if let Some(from) = self.game_exports() {
            let to = w.exports.clone();
            match tokio::task::spawn_blocking(move || copy_tree(&from, &to)).await {
                Ok(Ok(n)) => self.host.server.note(&format!(
                    "Copied {n} files of the game's CatalogExporter export into the workspace."
                )),
                Ok(Err(error)) => {
                    let why = log::describe_error(&error);
                    return self.finish_job(HostJobKind::GenerateGamedata, false, Some(why), Some(&log_file));
                }
                Err(error) => {
                    return self.finish_job(
                        HostJobKind::GenerateGamedata,
                        false,
                        Some(error.to_string()),
                        Some(&log_file),
                    );
                }
            }
        } else if !is_dir(&w.exports) {
            self.set_job(None);
            return self.host_fail("no_exports");
        }
        let restart = self.host.server.running().then(|| "restart".to_owned());
        self.one_shot(HostJobKind::GenerateGamedata, ws, &["gamedata"], "gamedata", restart)
            .await
    }

    /// Where the Dumper-7 or CatalogExporter session runs: the game's Win64 folder, after the checks
    /// both sessions share.
    fn session_folder(&self) -> Result<PathBuf, ActionResult> {
        if self.c.host != HostPlatform::Windows {
            // Under Proton the loader works the same way; RA12a's spike decides (RD4)
            return Err(self.host_fail("windows_only"));
        }
        {
            let st = self.st();
            if st.is_busy() || st.shutting_down {
                return Err(busy());
            }
        }
        if self.game.running() {
            return Err(self.host_fail("game_running"));
        }
        let (dir, problem) = {
            let st = self.st();
            (Self::win64_dir_of(&st), st.game.problem)
        };
        match (dir, problem) {
            (None, _) | (_, Some(GameProblem::NotFound | GameProblem::NotSet)) => {
                Err(ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None)))
            }
            // The SDK and the export must come from the exact build the runtime targets
            (_, Some(GameProblem::WrongVersion)) => {
                Err(ActionResult::Err(self.fail(ErrorCode::GameWrongVersion, None)))
            }
            (Some(dir), _) => Ok(dir),
        }
    }

    /// The Dumper-7 session: the game starts with the loader and Dumper-7 on its list, without the
    /// runtime. The SDK appears under `C:\Dumper-7` a minute or two later; "Import SDK" copies it into the
    /// workspace. Closing the game ends the session (`mod.rs`).
    async fn start_dump(&self) -> ActionResult {
        let dir = match self.session_folder() {
            Ok(dir) => dir,
            Err(failed) => return failed,
        };
        let Some(dll) = self.s().dumper_dll.map(PathBuf::from).filter(|dll| is_file(dll)) else {
            return self.host_fail("dumper");
        };
        if !self.p.find_running_clients().await.is_empty() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let session = match self.begin_loader_session(SessionKind::Dump, &dir, &dll) {
            Ok(session) => session,
            Err(error) => return ActionResult::Err(error),
        };
        self.start_job(HostJobKind::Inject, Some("Dumper-7".into()), None);
        {
            let mut st = self.st();
            st.dumping = true;
            st.loader_session = Some(session);
        }
        // The game's own environment, without anything that would point a runtime somewhere
        let base = self.seams.env.clone().unwrap_or_else(|| std::env::vars_os().collect());
        let env: Vec<_> = base
            .into_iter()
            .filter(|(name, _)| {
                let name = name.to_string_lossy().to_ascii_uppercase();
                !name.starts_with("REFORGED_") && !name.starts_with("MYSTICPARADOX_")
            })
            .collect();
        let args: Vec<String> = DUMP_ARGS.iter().map(|a| (*a).to_owned()).collect();
        log::info(&format!(
            "host: starting the game for Dumper-7 ({} through {})",
            file_name(&dll),
            loader_session::LOADER_INI
        ));
        if let Err(error) = self.game.start(&dir, &args, None, &env) {
            let session = {
                let mut st = self.st();
                st.dumping = false;
                st.loader_session.take()
            };
            if let Some(session) = session {
                self.end_loader_session(session).await;
            }
            return self.finish_job(HostJobKind::Inject, false, Some(log::describe_error(&error)), None);
        }
        self.finish_job(HostJobKind::Inject, true, Some("Dumper-7".into()), None)
    }

    /// The CatalogExporter session: `export_flags.txt` in the game's working folder, then the game
    /// starts as PLAY starts it (with the runtime, through the relay) and the loader loads CatalogExporter
    /// after the runtime. It writes `Items_Analysis` there; closing the game ends the session.
    async fn start_export(&self) -> ActionResult {
        let Some(ws) = self.workspace() else {
            return self.host_fail("workspace");
        };
        let dir = match self.session_folder() {
            Ok(dir) => dir,
            Err(failed) => return failed,
        };
        let exporter = WorkspacePaths::new(&ws).exporter_dll;
        if !is_file(&exporter) {
            return self.host_fail("exporter");
        }
        if let Err(error) = tokio::fs::write(dir.join("export_flags.txt"), EXPORT_FLAGS).await {
            return self.host_fail(&format!("export_flags:{}", log::describe_error(&error)));
        }
        self.start_job(HostJobKind::Inject, Some("CatalogExporter".into()), None);
        match self.play_with(Some(exporter)).await {
            ActionResult::Ok => {
                log::info("host: the game started with CatalogExporter on the loader's list");
                self.finish_job(HostJobKind::Inject, true, Some("CatalogExporter".into()), None)
            }
            failed => {
                self.set_job(None);
                failed
            }
        }
    }
}
