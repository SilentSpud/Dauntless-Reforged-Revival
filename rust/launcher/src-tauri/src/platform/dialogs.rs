//! The file and message dialogs (row L36), shown from Rust with `tauri-plugin-dialog` and the backend's
//! own text (row L14); the page has no dialog permission and never names a path. As in `src/main.ts`:
//! folders start where the current one is (when it exists), key backups and offline saves in Documents.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath, MessageDialogButtons, MessageDialogKind, MessageDialogResult};
use tokio::sync::oneshot;

use crate::core::i18n::{self, Language, translate};
use crate::window::MAIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPurpose {
    /// "Change folder…" for the install
    Install,
    /// "I already have the game files"
    Existing,
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
    let key = match purpose {
        FolderPurpose::Install => i18n::DIALOG_INSTALL_FOLDER,
        FolderPurpose::Existing => i18n::DIALOG_EXISTING_FOLDER,
    };
    let mut dialog = app
        .dialog()
        .file()
        .set_title(translate(language, key, &[]))
        .set_can_create_directories(true);
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

/// A file to open (a key backup or an offline save).
pub async fn choose_open_file(app: &AppHandle, start: Option<&Path>, filter: (&str, &str)) -> Option<PathBuf> {
    let mut dialog = app.dialog().file().add_filter(filter.0, &[filter.1]);
    if let Some(start) = start {
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
#[cfg_attr(
    not(feature = "offline"),
    expect(dead_code, reason = "offline play returns with the Rust services (row L42, X2)")
)]
pub const OFFLINE_SAVES: (&str, &str) = ("Dauntless Revived offline save", "drsave");

/// Why the launcher should stay open while the player closes its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepOpen {
    /// The public-mode relay carries the game's connection
    Relay,
    /// The offline services run the game's world
    #[cfg_attr(
        not(feature = "offline"),
        expect(dead_code, reason = "offline play returns with the Rust services (row L42, X2)")
    )]
    Offline,
}

/// Asks whether to close anyway; true means keep the launcher open (the default answer).
pub async fn confirm_keep_open(app: &AppHandle, language: Language, reason: KeepOpen) -> bool {
    let text = match reason {
        KeepOpen::Relay => i18n::CLOSE_TEXT,
        KeepOpen::Offline => i18n::CLOSE_TEXT_OFFLINE,
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
