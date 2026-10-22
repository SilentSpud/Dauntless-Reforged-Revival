//! The file and message dialogs (rows L36, RA11), shown from Rust with `tauri-plugin-dialog` and the
//! backend's own text (row L14); the page has no dialog permission and never names a path. As in Reforged's
//! `src/main.ts`: each picker is titled by its purpose (the `dialog_*` keys of the language files), folders
//! start where the current one is (when it exists), key backups in Documents.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath, MessageDialogButtons, MessageDialogKind, MessageDialogResult};
use tokio::sync::oneshot;

use crate::core::i18n::{self, Language, translate};
use crate::window::MAIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPurpose {
    /// The player's Dauntless 1.14.7 folder
    Game,
    /// A folder with the runtime's two files (a local runtime)
    Runtime,
    /// Dumper-7's CppSDK folder (Host page)
    Sdk,
    /// The host's workspace (Host page)
    Workspace,
}

// The language files' keys (`src/shared/i18n.ts`); RA3 lists them with the backend's other keys
pub const DIALOG_GAME_FOLDER: &str = "dialog_game_folder";
pub const DIALOG_RUNTIME_FOLDER: &str = "dialog_runtime_folder";
pub const DIALOG_SDK_FOLDER: &str = "dialog_sdk_folder";
pub const DIALOG_WORKSPACE_FOLDER: &str = "dialog_workspace_folder";
pub const DIALOG_DUMPER_DLL: &str = "dialog_dumper_dll";
pub const DIALOG_GAMEDATA_PACK: &str = "dialog_gamedata_pack";

/// The title key of a folder picker.
pub fn folder_title(purpose: FolderPurpose) -> &'static str {
    match purpose {
        FolderPurpose::Game => DIALOG_GAME_FOLDER,
        FolderPurpose::Runtime => DIALOG_RUNTIME_FOLDER,
        FolderPurpose::Sdk => DIALOG_SDK_FOLDER,
        FolderPurpose::Workspace => DIALOG_WORKSPACE_FOLDER,
    }
}

fn path_of(picked: Option<FilePath>) -> Option<PathBuf> {
    picked?.into_path().ok()
}

async fn answer<T: Send + 'static>(ask: impl FnOnce(oneshot::Sender<T>)) -> Option<T> {
    let (tx, rx) = oneshot::channel();
    ask(tx);
    rx.await.ok()
}

pub async fn choose_folder(
    app: &AppHandle,
    language: Language,
    purpose: FolderPurpose,
    start: &Path,
) -> Option<PathBuf> {
    let mut dialog = app
        .dialog()
        .file()
        .set_title(translate(language, folder_title(purpose), &[]));
    if start.exists() {
        dialog = dialog.set_directory(start);
    }
    if let Some(window) = app.get_webview_window(MAIN) {
        dialog = dialog.set_parent(&window);
    }
    answer(|tx| dialog.pick_folder(move |picked| drop(tx.send(path_of(picked)))))
        .await
        .flatten()
}

/// A text file (a key backup) to save, starting in Documents with `default_name`.
pub async fn choose_save_file(
    app: &AppHandle,
    documents: &Path,
    default_name: &str,
    filter: (&str, &str),
) -> Option<PathBuf> {
    let mut dialog = app
        .dialog()
        .file()
        .set_directory(documents)
        .set_file_name(default_name)
        .add_filter(filter.0, &[filter.1]);
    if let Some(window) = app.get_webview_window(MAIN) {
        dialog = dialog.set_parent(&window);
    }
    answer(|tx| dialog.save_file(move |picked| drop(tx.send(path_of(picked)))))
        .await
        .flatten()
}

/// A file to open (a key backup).
pub async fn choose_open_file(app: &AppHandle, start: Option<&Path>, filter: (&str, &str)) -> Option<PathBuf> {
    choose_titled_file(app, None, start, filter).await
}

/// The Dumper-7 DLL to inject, starting where the current one is.
pub async fn choose_dll(app: &AppHandle, language: Language, start: Option<&Path>) -> Option<PathBuf> {
    let title = translate(language, DIALOG_DUMPER_DLL, &[]);
    choose_titled_file(app, Some(title), start, DLL_FILES).await
}

/// A game data pack to import.
pub async fn choose_pack(app: &AppHandle, language: Language) -> Option<PathBuf> {
    let title = translate(language, DIALOG_GAMEDATA_PACK, &[]);
    choose_titled_file(app, Some(title), None, GAMEDATA_PACKS).await
}

async fn choose_titled_file(
    app: &AppHandle,
    title: Option<String>,
    start: Option<&Path>,
    filter: (&str, &str),
) -> Option<PathBuf> {
    let mut dialog = app.dialog().file().add_filter(filter.0, &[filter.1]);
    if let Some(title) = title {
        dialog = dialog.set_title(title);
    }
    if let Some(start) = start.filter(|start| start.exists()) {
        dialog = dialog.set_directory(start);
    }
    if let Some(window) = app.get_webview_window(MAIN) {
        dialog = dialog.set_parent(&window);
    }
    answer(|tx| dialog.pick_file(move |picked| drop(tx.send(path_of(picked)))))
        .await
        .flatten()
}

pub const TEXT_FILES: (&str, &str) = ("Text", "txt");
/// The Host page's pickers (`choose_dll`, `choose_pack`)
pub const DLL_FILES: (&str, &str) = ("DLL", "dll");
pub const GAMEDATA_PACKS: (&str, &str) = ("Game data pack", "gz");

/// Why the launcher should stay open while the player closes its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepOpen {
    /// The local relay carries the game's connection
    Relay,
}

/// Asks whether to close anyway; true means keep the launcher open (the default answer).
pub async fn confirm_keep_open(app: &AppHandle, language: Language, reason: KeepOpen) -> bool {
    let text = match reason {
        KeepOpen::Relay => i18n::CLOSE_TEXT,
    };
    let message = format!(
        "{}\n\n{}",
        translate(language, i18n::CLOSE_TITLE, &[]),
        translate(language, text, &[])
    );
    let anyway = translate(language, i18n::CLOSE_ANYWAY, &[]);
    let mut dialog = app
        .dialog()
        .message(message)
        .title(translate(language, i18n::APP_TITLE, &[]))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            translate(language, i18n::CLOSE_KEEP, &[]),
            anyway.clone(),
        ));
    if let Some(window) = app.get_webview_window(MAIN) {
        dialog = dialog.parent(&window);
    }
    // "Keep open" is the default; only a press of "Close anyway" closes (Esc and the dialog's own close
    // button keep the launcher, as Electron's cancelId did)
    let result = answer(|tx| dialog.show_with_result(move |result| drop(tx.send(result)))).await;
    !matches!(result, Some(MessageDialogResult::Custom(label)) if label == anyway)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_picker_is_titled_by_its_purpose() {
        assert_eq!(folder_title(FolderPurpose::Game), "dialog_game_folder");
        assert_eq!(folder_title(FolderPurpose::Runtime), "dialog_runtime_folder");
        assert_eq!(folder_title(FolderPurpose::Sdk), "dialog_sdk_folder");
        assert_eq!(folder_title(FolderPurpose::Workspace), "dialog_workspace_folder");
        assert_eq!((DLL_FILES, GAMEDATA_PACKS), (("DLL", "dll"), ("Game data pack", "gz")));
    }
}
