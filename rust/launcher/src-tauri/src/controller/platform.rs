//! What the controller needs from the app and the operating system (`Platform` in `controller.ts`). The
//! app passes `TauriPlatform`; the tests pass fakes, so the whole controller runs without a window.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::bridge::backend::{Backend, SNAPSHOT_EVENT};
use crate::bridge::types::{Language, Snapshot, TaskProgress};
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
    /// Opens a checked https URL in the browser.
    fn open_url(&self, url: &str, host_source: Option<&str>);
    fn open_folder(&self, dir: &Path);
    fn emit_snapshot(&self, snapshot: &Snapshot);
    fn emit_progress(&self, progress: &TaskProgress);
    /// Registers a downloaded image for `dr-art://bg/<id>`.
    fn put_art(&self, id: &str, file: PathBuf, kind: &'static str);
    fn find_tailscale(&self) -> bool;
    fn find_running_clients(&self) -> Boxed<Vec<u32>>;
    fn prepare_game_launch(&self) -> Boxed<Prepared>;
    fn collect_device_signals(&self) -> Boxed<Value>;
    /// The Visual C++ and DirectX files the game needs that Windows lacks.
    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>);
    /// Pushes offline play's status to the page (`offline-status-changed`).
    #[cfg(feature = "offline")]
    fn emit_offline_status(&self, _status: &crate::bridge::types::OfflineStatus) {}
    /// The `.drsave` file to back up to (`export`) or restore from; None when cancelled.
    #[cfg(feature = "offline")]
    fn choose_offline_save(&self, _export: bool) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }
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

    fn open_url(&self, url: &str, host_source: Option<&str>) {
        crate::platform::links::open(url, host_source);
    }

    fn open_folder(&self, dir: &Path) {
        crate::platform::links::open_folder(dir);
    }

    fn emit_snapshot(&self, snapshot: &Snapshot) {
        let _ = self.app.emit_to(MAIN, SNAPSHOT_EVENT, snapshot);
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

    fn collect_device_signals(&self) -> Boxed<Value> {
        Box::pin(async {
            serde_json::to_value(crate::platform::system::collect_device_signals().await).unwrap_or(Value::Null)
        })
    }

    #[cfg(feature = "offline")]
    fn emit_offline_status(&self, status: &crate::bridge::types::OfflineStatus) {
        let _ = self.app.emit_to(MAIN, "offline-status-changed", status);
    }

    #[cfg(feature = "offline")]
    fn choose_offline_save(&self, export: bool) -> Boxed<Option<PathBuf>> {
        let app = self.app.clone();
        let documents = self.documents.clone();
        Box::pin(async move {
            if export {
                let stamp = chrono::Utc::now().format("%Y-%m-%d");
                let name = format!("Dauntless Revived offline save {stamp}.drsave");
                dialogs::choose_save_file(&app, &documents, &name, dialogs::OFFLINE_SAVES).await
            } else {
                dialogs::choose_open_file(&app, Some(&documents), dialogs::OFFLINE_SAVES).await
            }
        })
    }

    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>) {
        use crate::platform::launch::{missing_direct_x, missing_vc_runtime};
        (
            strings(missing_vc_runtime(Path::exists)),
            strings(missing_direct_x(Path::exists)),
        )
    }
}
