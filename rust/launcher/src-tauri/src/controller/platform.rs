//! What the controller needs from the app and the operating system (`Platform` in `controller.ts`). The
//! app passes `TauriPlatform`; the tests pass fakes, so the whole controller runs without a window.
//! Reforged's members are here from RA2 on (`choose_dll`, `choose_pack`, `emit_host_kit`,
//! `emit_host_server`, `inject`); DR's device signals are gone, as in Reforged.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use tauri::{AppHandle, Emitter, Manager};

use crate::bridge::backend::{Backend, HOST_KIT_EVENT, HOST_SERVER_EVENT, SNAPSHOT_EVENT};
use crate::bridge::types::{HostKit, HostServerView, Language, Snapshot, TaskProgress};
use crate::platform::art::ArtStore;
use crate::platform::dialogs::{self, FolderPurpose};
use crate::platform::linux_runtime::PreparedLaunch;
use crate::window::MAIN;

pub type Boxed<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// How the game starts on this PC.
pub enum Prepared {
    /// Windows: the exe itself
    Native,
    /// Linux: through Proton or Wine
    Runtime(PreparedLaunch),
    /// Linux without Proton or Wine
    Missing,
}

pub trait Platform: Send + Sync {
    fn choose_folder(&self, language: Language, purpose: FolderPurpose, start: PathBuf) -> Boxed<Option<PathBuf>>;
    fn choose_save_file(&self, default_name: String) -> Boxed<Option<PathBuf>>;
    fn choose_open_file(&self) -> Boxed<Option<PathBuf>>;
    /// The Dumper-7 DLL (Host page); None when cancelled.
    fn choose_dll(&self) -> Boxed<Option<PathBuf>>;
    /// A game data pack, `.gz` (Host page); None when cancelled.
    fn choose_pack(&self) -> Boxed<Option<PathBuf>>;
    /// Opens a checked https URL in the browser.
    fn open_url(&self, url: &str, host_source: Option<&str>);
    fn open_folder(&self, dir: &Path);
    fn emit_snapshot(&self, snapshot: &Snapshot);
    fn emit_progress(&self, progress: &TaskProgress);
    fn emit_host_kit(&self, kit: &HostKit);
    /// Carries the newest invite link, a secret: never logged.
    fn emit_host_server(&self, view: &HostServerView);
    /// Registers a downloaded image for `rr-art://bg/<id>`.
    fn put_art(&self, id: &str, file: PathBuf, kind: &'static str);
    fn find_tailscale(&self) -> bool;
    fn find_running_clients(&self) -> Boxed<Vec<u32>>;
    fn prepare_game_launch(&self) -> Boxed<Prepared>;
    /// Loads a DLL into a running process (RD9; RA12).
    fn inject(&self, pid: u32, dll: PathBuf) -> Boxed<Result<(), crate::platform::injector::InjectError>>;
    /// The Visual C++ and DirectX files the game needs that Windows lacks.
    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>);
}

pub struct TauriPlatform {
    pub app: AppHandle,
    pub user_data: PathBuf,
    pub documents: PathBuf,
    pub home: PathBuf,
}

fn strings(list: Vec<&'static str>) -> Vec<String> {
    list.into_iter().map(str::to_owned).collect()
}

impl Platform for TauriPlatform {
    fn choose_folder(&self, language: Language, purpose: FolderPurpose, start: PathBuf) -> Boxed<Option<PathBuf>> {
        let app = self.app.clone();
        Box::pin(async move { dialogs::choose_folder(&app, language, purpose, &start).await })
    }

    fn choose_save_file(&self, default_name: String) -> Boxed<Option<PathBuf>> {
        let app = self.app.clone();
        let documents = self.documents.clone();
        Box::pin(async move { dialogs::choose_save_file(&app, &documents, &default_name, dialogs::TEXT_FILES).await })
    }

    fn choose_open_file(&self) -> Boxed<Option<PathBuf>> {
        let app = self.app.clone();
        let documents = self.documents.clone();
        Box::pin(async move { dialogs::choose_open_file(&app, Some(&documents), dialogs::TEXT_FILES).await })
    }

    fn choose_dll(&self) -> Boxed<Option<PathBuf>> {
        let app = self.app.clone();
        Box::pin(async move { dialogs::choose_open_file(&app, None, dialogs::DLL_FILES).await })
    }

    fn choose_pack(&self) -> Boxed<Option<PathBuf>> {
        let app = self.app.clone();
        Box::pin(async move { dialogs::choose_open_file(&app, None, dialogs::GAMEDATA_PACKS).await })
    }

    fn open_url(&self, url: &str, host_source: Option<&str>) {
        crate::platform::links::open(url, host_source);
    }

    fn open_folder(&self, dir: &Path) {
        crate::platform::links::open_folder(dir);
    }

    fn emit_snapshot(&self, snapshot: &Snapshot) {
        let _ = self.app.emit_to(MAIN, SNAPSHOT_EVENT, snapshot);
    }

    fn emit_host_kit(&self, kit: &HostKit) {
        let _ = self.app.emit_to(MAIN, HOST_KIT_EVENT, kit);
    }

    fn emit_host_server(&self, view: &HostServerView) {
        let _ = self.app.emit_to(MAIN, HOST_SERVER_EVENT, view);
    }

    fn emit_progress(&self, progress: &TaskProgress) {
        self.app.state::<Backend>().push_progress(progress);
    }

    fn put_art(&self, id: &str, file: PathBuf, kind: &'static str) {
        self.app.state::<ArtStore>().put(id, file, kind);
    }

    fn find_tailscale(&self) -> bool {
        crate::platform::system::find_tailscale().is_some()
    }

    fn find_running_clients(&self) -> Boxed<Vec<u32>> {
        Box::pin(crate::platform::launch::find_running_clients())
    }

    fn prepare_game_launch(&self) -> Boxed<Prepared> {
        if cfg!(windows) {
            return Box::pin(async { Prepared::Native });
        }
        let user_data = self.user_data.clone();
        let home = self.home.clone();
        Box::pin(async move {
            let env = crate::platform::linux_runtime::current_env();
            let auto = !cfg!(dev);
            match crate::platform::linux_runtime::prepare_linux_game_launch(&user_data, &env, &home, auto).await {
                Ok(prepared) => Prepared::Runtime(prepared),
                Err(_) => Prepared::Missing,
            }
        })
    }

    fn inject(&self, pid: u32, dll: PathBuf) -> Boxed<Result<(), crate::platform::injector::InjectError>> {
        Box::pin(crate::platform::injector::inject(pid, dll))
    }

    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>) {
        use crate::platform::launch::{missing_direct_x, missing_vc_runtime};
        (
            strings(missing_vc_runtime(Path::exists)),
            strings(missing_direct_x(Path::exists)),
        )
    }
}
