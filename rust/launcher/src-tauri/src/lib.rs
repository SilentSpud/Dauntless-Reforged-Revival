//! The Dauntless Reforged Revival Launcher on Tauri 2 (MIGRATION_PLAN.md section 4.6), on Dauntless
//! Revived's launcher backend (e1c5fec). The page is the same TypeScript page the Electron launcher shows
//! (`legacy/ReforgedLauncher`, built by `../vite.config.ts`); everything the Electron main process did
//! moves here.
//!
//! RA1 and RA2 set the seams: the app's identity, the window, every command the page calls (`bridge/`)
//! and a stub for every module a later row filled, so each of those rows only filled its own files.
//! Until RA13 wires the controller to them, the network and platform layers are not called outside their
//! tests.

mod bridge;
mod controller;
#[allow(dead_code, reason = "RA13's controller uses the core rules")]
mod core;
#[allow(dead_code, reason = "RA13's controller uses the network layer")]
mod net;
#[allow(dead_code, reason = "RA13's controller uses the platform layer")]
mod platform;
mod window;

use bridge::backend::Backend;
use bridge::commands;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Manager, RunEvent};

use controller::platform::TauriPlatform;
use controller::{Config, Controller, ControllerHandle};
use platform::paths::Paths;

use platform::invite_link::{self, Claim};

/// The running app, for what arrives from a second launcher.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// A second launcher's arguments: bring the window forward and take its invite link.
fn on_second_launcher(args: Vec<String>) {
    let Some(app) = APP.get() else { return };
    if let Some(window) = app.get_webview_window(window::MAIN) {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    app.state::<Backend>()
        .deliver_invite_link(Some(app), invite_link::invite_from_args(&args));
}

/// Builds and runs the app until the last window closes; hands over to the launcher already running.
pub fn run() -> tauri::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(target_os = "linux")]
    secure_storage::wait_for_parent();
    let on_args: invite_link::OnArgs = Arc::new(|args| Box::pin(async move { on_second_launcher(args) }));
    if tauri::async_runtime::block_on(invite_link::claim(&invite_link::channel_name(), &args, on_args)) == Claim::Handed
    {
        return Ok(());
    }
    platform::log::install();
    // The updater's HTTP client (rustls without a built-in provider) uses the same provider as the rest
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let paths = Paths::from_system().ok_or_else(|| tauri::Error::UnknownPath)?;
    platform::log::log_to_file(paths.logs_dir());
    let context = tauri::generate_context!();
    let version = context.package_info().version.to_string();
    platform::log::info(&format!("Dauntless Reforged Revival Launcher {version} starting"));
    let encryptors = tauri::async_runtime::block_on(platform::keystore::os::encryptors(&paths.user_data));
    #[cfg(target_os = "linux")]
    if !encryptors.0.is_available() && secure_storage::bootstrap(&args, &paths.home) {
        return Ok(());
    }
    let backend = Backend::default();
    backend.deliver_invite_link(None, invite_link::invite_from_args(&args));
    tauri::Builder::default()
        // Called from Rust only: the page has no dialog or updater permission (L36, L46)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::new(platform::updater::Updates::default()))
        .manage(window::CloseGuard::default())
        .manage(platform::art::ArtStore::default())
        // The host's art, from the controller's cache (L37); nothing from the network
        .register_asynchronous_uri_scheme_protocol(platform::art::SCHEME, |context, request, responder| {
            let response = context
                .app_handle()
                .state::<platform::art::ArtStore>()
                .respond(request.uri());
            responder.respond(response);
        })
        .manage(backend)
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::check_invite,
            commands::submit_invite,
            commands::retry_connect,
            commands::forget_server,
            commands::switch_server,
            commands::remove_saved_server,
            commands::register,
            commands::use_existing_key,
            commands::import_key_file,
            commands::save_key_backup,
            commands::dismiss_backup_offer,
            commands::choose_game_folder,
            commands::use_game_path,
            commands::use_detected_game,
            commands::verify_game,
            commands::trust_runtime,
            commands::install_runtime,
            commands::choose_local_runtime,
            commands::clear_local_runtime,
            commands::cancel_task,
            commands::play,
            commands::set_settings,
            commands::set_status_polling,
            commands::refresh_status,
            commands::get_news,
            commands::get_branding,
            commands::open_external,
            commands::open_game_folder,
            commands::open_logs_folder,
            commands::logout,
            commands::install_update,
            commands::host_get,
            commands::host_choose_workspace,
            commands::host_choose_dumper,
            commands::host_import_gamedata,
            commands::host_import_sdk_folder,
            commands::host_download_project,
            commands::host_action,
            commands::host_open,
            commands::host_server_get,
            commands::host_server_start,
            commands::host_server_stop,
            commands::host_setup,
            commands::host_create_invite,
            commands::host_update,
            commands::host_set_multipliers,
            commands::copy_text,
            commands::take_invite_link,
            commands::dismiss_notice,
            commands::dismiss_error,
            commands::subscribe_progress,
        ])
        .setup(move |app| {
            let _ = APP.set(app.handle().clone());
            if !cfg!(dev)
                && let Ok(exe) = std::env::current_exe()
            {
                invite_link::register_scheme(&exe);
            }
            let controller = make_controller(app.handle(), paths, version, encryptors)?;
            let guard = app.state::<window::CloseGuard>();
            let (reason, kept) = (Arc::downgrade(&controller), Arc::downgrade(&controller));
            guard.set(
                move || reason.upgrade().and_then(|controller| controller.keep_open()),
                move || {
                    if let Some(controller) = kept.upgrade() {
                        controller.set_notice(Some(bridge::types::NoticeCode::KeepOpen));
                    }
                },
            );
            app.manage(controller.clone());
            window::create(app.handle())?;
            // Packaged builds only: a development build has no release to update to
            if !cfg!(dev) {
                platform::updater::start(app.handle().clone(), controller.clone());
            }
            tauri::async_runtime::spawn(async move { controller.init().await });
            Ok(())
        })
        .build(context)?
        .run(|app, event| {
            // As the Electron launcher on window-all-closed: stop the relay and wait for the background work
            static STOPPED: AtomicBool = AtomicBool::new(false);
            if let RunEvent::ExitRequested { api, .. } = event
                && !STOPPED.swap(true, Ordering::SeqCst)
            {
                api.prevent_exit();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    app.state::<ControllerHandle>().inner().clone().shutdown().await;
                    app.exit(0);
                });
            }
        });
    Ok(())
}

/// Linux without a Secret Service (DR's `ensureLinuxSecureStorageDependency`): a packaged launcher
/// installs gnome-keyring and libsecret through the package manager once, then starts again. Without it,
/// keys cannot be saved ("unavailable"), as before. `REFORGED_AUTO_INSTALL=0` turns this off.
#[cfg(target_os = "linux")]
mod secure_storage {
    use std::path::Path;
    use std::time::Duration;

    use crate::platform::linux_runtime::{DependencyKind, current_env, install_plan, run_install};
    use crate::platform::log;

    const BOOTSTRAPPED: &str = "REFORGED_STORAGE_BOOTSTRAPPED";
    const PARENT: &str = "REFORGED_PARENT_PID";

    /// A launcher started again after the install waits for the old one to exit, which still holds the
    /// single-launcher channel; else the new one would hand its arguments to it and stop.
    pub fn wait_for_parent() {
        let Some(pid) = std::env::var(PARENT).ok().and_then(|pid| pid.parse::<u32>().ok()) else {
            return;
        };
        for _ in 0..100 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// True when the dependencies were installed and a new launcher was started: this one stops.
    pub fn bootstrap(args: &[String], home: &Path) -> bool {
        log::warn("secure storage unavailable (no Secret Service)");
        let env = current_env();
        if cfg!(dev)
            || env.get("REFORGED_AUTO_INSTALL").map(String::as_str) == Some("0")
            || env.get(BOOTSTRAPPED).map(String::as_str) == Some("1")
        {
            return false;
        }
        let Some(plan) = install_plan(DependencyKind::SecureStorage, &env) else {
            return false;
        };
        log::info(&format!(
            "installing Linux secure-storage dependencies with {}",
            plan.name
        ));
        if let Err(error) = tauri::async_runtime::block_on(run_install(&plan, &env, home)) {
            log::error(&format!("secure-storage dependency install failed: {error}"));
            return false;
        }
        let Ok(exe) = std::env::current_exe() else { return false };
        let started = std::process::Command::new(exe)
            .args(args.iter().skip(1))
            .env(BOOTSTRAPPED, "1")
            .env(PARENT, std::process::id().to_string())
            .spawn();
        match started {
            Ok(_) => {
                log::info("Linux secure-storage dependencies installed; restarting launcher");
                true
            }
            Err(error) => {
                log::error(&format!("the launcher could not start again: {error}"));
                false
            }
        }
    }
}

/// The controller with this PC's folders and the app's resources.
fn make_controller(
    app: &AppHandle,
    paths: Paths,
    version: String,
    (encryptor, legacy_encryptor): (
        Arc<dyn platform::keystore::Encryptor>,
        Option<Arc<dyn platform::keystore::Encryptor>>,
    ),
) -> tauri::Result<ControllerHandle> {
    use crate::core::pins::{EXE_RELATIVE_PATH, PINNED_EXE_SHA256};

    let resources_dir = if cfg!(dev) {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../brand/launcher"))
    } else {
        app.path().resource_dir()?
    };
    let relay_port = net::relay::relay_port_override(std::env::var("REFORGED_RELAY_PORT").ok().as_deref())
        .unwrap_or(net::relay::DEFAULT_RELAY_PORT);
    let config = Config {
        user_data: paths.user_data.clone(),
        documents: Some(paths.documents.clone()),
        resources_dir,
        host: platform::settings::Platform::current(),
        app_version: version,
        packaged: !cfg!(dev),
        language: core::i18n::os_language(),
        system_locale: sys_locale::get_locale().unwrap_or_else(|| "en-US".into()),
        game_config_dir: platform::game_config::default_config_dir(),
        relay_port,
        exe_pin: (EXE_RELATIVE_PATH.into(), PINNED_EXE_SHA256.into()),
        status_poll: std::time::Duration::from_millis(core::pins::STATUS_POLL_MS),
        encryptor,
        legacy_encryptor,
    };
    let platform = TauriPlatform {
        app: app.clone(),
        user_data: paths.user_data,
        documents: paths.documents,
        home: paths.home,
    };
    Ok(Controller::new(Arc::new(platform), config))
}
