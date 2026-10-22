//! One command per `LauncherApi` method (row L7), named after today's IPC channel without `dr:` (in
//! snake_case). Each checks its arguments again, whatever the isolation hook let through: the hook
//! guards the page, these guard the backend (`ipc-validate.ts`). Then it hands over to the controller
//! (row L38).

use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use super::backend::Backend;
use super::spec::{GAME_PATH_MAX, INVITE_MAX, KEY_TEXT_MAX, USERNAME_MAX, fits, is_server_id};
#[cfg(not(feature = "offline"))]
use super::types::OfflinePhase;
use super::types::{
    ActionResult, Branding, ErrorCode, ExternalTarget, InviteCheck, LauncherError, NewsItem, OfflineStatus,
    RegisterOutcome, Settings, SettingsPatch, Snapshot, TaskProgress,
};
use crate::controller::ControllerHandle;

type Backed<'a> = State<'a, Backend>;

/// The controller, owned (async commands cannot borrow state without returning a `Result`).
fn controller(app: &AppHandle) -> ControllerHandle {
    app.state::<ControllerHandle>().inner().clone()
}

#[tauri::command]
pub fn get_snapshot(app: AppHandle) -> Snapshot {
    controller(&app).snapshot()
}

/// Whether an invite can be joined. A public invite whose certificate differs from the one this PC's
/// key for that server belongs to says so.
#[tauri::command]
pub fn check_invite(text: String, app: AppHandle) -> InviteCheck {
    if !fits(&text, INVITE_MAX) {
        return InviteCheck::Err(LauncherError::new(ErrorCode::InviteInvalidFormat));
    }
    controller(&app).check_invite(&text)
}

#[tauri::command]
pub async fn submit_invite(text: String, accept_new_certificate: bool, app: AppHandle) -> ActionResult {
    if !fits(&text, INVITE_MAX) {
        return ActionResult::err(ErrorCode::InviteInvalidFormat);
    }
    controller(&app).submit_invite(&text, accept_new_certificate).await
}

#[tauri::command]
pub async fn retry_connect(app: AppHandle) -> ActionResult {
    controller(&app).connect().await
}

#[tauri::command]
pub async fn forget_server(app: AppHandle) -> ActionResult {
    controller(&app).forget_server().await
}

#[tauri::command]
pub async fn switch_server(id: String, app: AppHandle) -> ActionResult {
    if !is_server_id(&id) {
        return ActionResult::err(ErrorCode::InviteInvalidFormat);
    }
    controller(&app).switch_server(&id).await
}

#[tauri::command]
pub async fn remove_saved_server(id: String, app: AppHandle) -> ActionResult {
    if !is_server_id(&id) {
        return ActionResult::err(ErrorCode::InviteInvalidFormat);
    }
    controller(&app).remove_saved_server(&id).await
}

#[tauri::command]
pub async fn register(username: String, app: AppHandle) -> RegisterOutcome {
    if !fits(&username, USERNAME_MAX) {
        return RegisterOutcome::Err(LauncherError::new(ErrorCode::UsernameInvalid));
    }
    controller(&app).register(&username).await
}

#[tauri::command]
pub async fn use_existing_key(text: String, app: AppHandle) -> ActionResult {
    if !fits(&text, KEY_TEXT_MAX) {
        return ActionResult::err(ErrorCode::KeyInvalid);
    }
    controller(&app).use_existing_key(&text).await
}

#[tauri::command]
pub async fn import_key_file(app: AppHandle) -> ActionResult {
    controller(&app).import_key_file().await
}

#[tauri::command]
pub async fn save_key_backup(app: AppHandle) -> ActionResult {
    controller(&app).save_key_backup().await
}

#[tauri::command]
pub async fn dismiss_backup_offer(app: AppHandle) {
    controller(&app).dismiss_backup_offer().await;
}

#[tauri::command]
pub async fn choose_install_dir(app: AppHandle) -> ActionResult {
    controller(&app).choose_install_dir().await
}

#[tauri::command]
pub async fn use_existing_game_folder(app: AppHandle) -> ActionResult {
    controller(&app).use_existing_game_folder().await
}

#[tauri::command]
pub async fn use_existing_game_path(game_path: String, app: AppHandle) -> ActionResult {
    if !fits(&game_path, GAME_PATH_MAX) {
        return ActionResult::err(ErrorCode::FolderInvalid);
    }
    controller(&app).use_existing_game_path(&game_path).await
}

#[tauri::command]
pub async fn start_install(app: AppHandle) -> ActionResult {
    controller(&app).start_install().await
}

#[tauri::command]
pub fn pause_task(app: AppHandle) {
    controller(&app).pause_task();
}

#[tauri::command]
pub fn resume_task(app: AppHandle) {
    controller(&app).resume_task();
}

#[tauri::command]
pub fn cancel_task(app: AppHandle) {
    controller(&app).cancel_task();
}

#[tauri::command]
pub async fn repair(app: AppHandle) -> ActionResult {
    controller(&app).repair().await
}

#[tauri::command]
pub async fn play(app: AppHandle) -> ActionResult {
    controller(&app).play().await
}

/// Offline play is off in 0.2.0 (row L42, D1); it returns with the Rust services (X2). The page says so.
#[cfg_attr(
    feature = "offline",
    allow(dead_code, reason = "without an offline stack on this PC")
)]
fn offline_off() -> ActionResult {
    ActionResult::Err(LauncherError {
        code: ErrorCode::OfflineUnavailable,
        detail: Some("later_update".into()),
    })
}

#[tauri::command]
pub async fn start_offline(username: Option<String>, app: AppHandle) -> ActionResult {
    if username.as_deref().is_some_and(|name| !fits(name, USERNAME_MAX)) {
        return ActionResult::err(ErrorCode::UsernameInvalid);
    }
    #[cfg(feature = "offline")]
    return controller(&app).start_offline(username).await;
    #[cfg(not(feature = "offline"))]
    {
        let _ = app;
        offline_off()
    }
}

#[tauri::command]
pub async fn stop_offline(app: AppHandle) -> ActionResult {
    #[cfg(feature = "offline")]
    return controller(&app).stop_offline().await;
    #[cfg(not(feature = "offline"))]
    {
        let _ = app;
        ActionResult::Ok
    }
}

#[tauri::command]
pub fn offline_status(app: AppHandle) -> OfflineStatus {
    #[cfg(feature = "offline")]
    return controller(&app).offline_status();
    #[cfg(not(feature = "offline"))]
    {
        let _ = app;
        OfflineStatus {
            phase: OfflinePhase::Stopped,
            progress: None,
            error: None,
        }
    }
}

/// An unknown field or a wrong value refuses the whole patch, which then changes nothing, as today.
#[tauri::command]
pub async fn set_settings(patch: Value, app: AppHandle) -> Settings {
    let patch = serde_json::from_value::<SettingsPatch>(patch).unwrap_or_default();
    controller(&app).set_settings(patch).await
}

#[tauri::command]
pub fn set_status_polling(on: bool, app: AppHandle) {
    controller(&app).set_status_polling(on);
}

#[tauri::command]
pub async fn refresh_status(app: AppHandle) {
    controller(&app).poll_status().await;
}

#[tauri::command]
pub fn get_news(app: AppHandle) -> Vec<NewsItem> {
    controller(&app).news()
}

#[tauri::command]
pub fn get_branding(app: AppHandle) -> Branding {
    controller(&app).branding()
}

#[tauri::command]
pub fn open_external(target: Value, app: AppHandle) -> ActionResult {
    match serde_json::from_value::<ExternalTarget>(target) {
        Ok(target) => controller(&app).open_external(target),
        Err(_) => ActionResult::err(ErrorCode::Unknown),
    }
}

#[tauri::command]
pub fn open_game_folder(app: AppHandle) {
    controller(&app).open_game_folder();
}

#[tauri::command]
pub async fn logout(app: AppHandle) -> ActionResult {
    controller(&app).logout().await
}

/// Installs the update the player was offered (L46); nothing when none waits or a task or the game runs.
#[tauri::command]
pub async fn install_update(app: AppHandle) {
    let controller = controller(&app);
    crate::platform::updater::install(app, controller).await;
}

#[tauri::command]
pub fn take_invite_link(backend: Backed<'_>) -> Option<String> {
    backend.take_invite_link()
}

#[tauri::command]
pub fn dismiss_notice(app: AppHandle) {
    controller(&app).set_notice(None);
}

#[tauri::command]
pub fn dismiss_error(app: AppHandle) {
    controller(&app).dismiss_error();
}

#[tauri::command]
pub async fn offline_export_save(app: AppHandle) -> ActionResult {
    #[cfg(feature = "offline")]
    return controller(&app).offline_save(true).await;
    #[cfg(not(feature = "offline"))]
    {
        let _ = app;
        offline_off()
    }
}

#[tauri::command]
pub async fn offline_import_save(app: AppHandle) -> ActionResult {
    #[cfg(feature = "offline")]
    return controller(&app).offline_save(false).await;
    #[cfg(not(feature = "offline"))]
    {
        let _ = app;
        offline_off()
    }
}

#[tauri::command]
pub fn subscribe_progress(on_progress: Channel<TaskProgress>, backend: Backed<'_>) {
    backend.subscribe_progress(on_progress);
}
