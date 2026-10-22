//! The Dauntless Revived Launcher on Tauri 2 (MIGRATION_PLAN.md Part A). The page is the same
//! TypeScript page the Electron launcher shows; everything the Electron main process did moves here.

mod bridge;
mod controller;
mod core;
mod net;
#[cfg(feature = "offline")]
pub mod offline;
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
    platform::log::info(&format!("Dauntless Revived Launcher {version} starting"));
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
            commands::choose_install_dir,
            commands::use_existing_game_folder,
            commands::use_existing_game_path,
            commands::start_install,
            commands::pause_task,
            commands::resume_task,
            commands::cancel_task,
            commands::repair,
            commands::play,
            commands::start_offline,
            commands::stop_offline,
            commands::offline_status,
            commands::set_settings,
            commands::set_status_polling,
            commands::refresh_status,
            commands::get_news,
            commands::get_branding,
            commands::open_external,
            commands::open_game_folder,
            commands::logout,
            commands::install_update,
            commands::take_invite_link,
            commands::dismiss_notice,
            commands::dismiss_error,
            commands::offline_export_save,
            commands::offline_import_save,
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

/// Linux without a Secret Service (`ensureLinuxSecureStorageDependency` in `main.ts`): a packaged launcher
/// installs gnome-keyring and libsecret through the package manager once, then starts again. Without it, keys
/// cannot be saved ("unavailable"), as before. `DAUNTLESS_REVIVED_AUTO_INSTALL=0` turns this off.
#[cfg(target_os = "linux")]
mod secure_storage {
    use std::path::Path;
    use std::time::Duration;

    use crate::platform::linux_runtime::{DependencyKind, current_env, install_plan, run_install};
    use crate::platform::log;

    const BOOTSTRAPPED: &str = "DAUNTLESS_REVIVED_STORAGE_BOOTSTRAPPED";
    const PARENT: &str = "DAUNTLESS_REVIVED_PARENT_PID";

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
            || env.get("DAUNTLESS_REVIVED_AUTO_INSTALL").map(String::as_str) == Some("0")
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

/// The controller with this PC's folders, the compiled-in manifest and the app's resources.
fn make_controller(
    app: &AppHandle,
    paths: Paths,
    version: String,
    (encryptor, legacy_encryptor): (
        Arc<dyn platform::keystore::Encryptor>,
        Option<Arc<dyn platform::keystore::Encryptor>>,
    ),
) -> tauri::Result<ControllerHandle> {
    use crate::core::manifest::GAME_MANIFEST;
    use crate::core::pins::{EXE_RELATIVE_PATH, PINNED_EXE_SHA256};

    // Offline play's game executable is the current install's, known once the controller exists
    #[cfg(feature = "offline")]
    let game_root: Arc<OnceLock<std::sync::Weak<Controller>>> = Arc::default();
    let resources_dir = if cfg!(dev) {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets"))
    } else {
        app.path().resource_dir()?
    };
    let manifest = (!GAME_MANIFEST.files.is_empty()).then(|| GAME_MANIFEST.clone());
    match &manifest {
        Some(manifest) => platform::log::info(&format!(
            "game manifest {} files, fingerprint {}",
            manifest.files.len(),
            crate::core::manifest::manifest_fingerprint(manifest)
        )),
        None => platform::log::error("compiled-in game manifest is not usable"),
    }
    let relay_port = net::relay::relay_port_override(std::env::var("DAUNTLESS_REVIVED_RELAY_PORT").ok().as_deref())
        .unwrap_or(net::relay::DEFAULT_RELAY_PORT);
    let config = Config {
        user_data: paths.user_data.clone(),
        documents: Some(paths.documents.clone()),
        resources_dir: resources_dir.clone(),
        default_install_dir: paths.default_install_dir.clone(),
        host: platform::settings::Platform::current(),
        app_version: version,
        packaged: !cfg!(dev),
        language: core::i18n::os_language(),
        system_locale: sys_locale::get_locale().unwrap_or_else(|| "en-US".into()),
        manifest,
        game_config_dir: platform::game_config::default_config_dir(),
        relay_port,
        exe_pin: (EXE_RELATIVE_PATH.into(), PINNED_EXE_SHA256.into()),
        status_poll: std::time::Duration::from_millis(core::pins::STATUS_POLL_MS),
        #[cfg(feature = "offline")]
        offline: offline_setup(&paths, &resources_dir, &encryptor, &legacy_encryptor, game_root.clone()),
        encryptor,
        legacy_encryptor,
        spawn: None,
    };
    let platform = TauriPlatform {
        app: app.clone(),
        user_data: paths.user_data,
        documents: paths.documents,
        home: paths.home,
    };
    let controller = Controller::new(Arc::new(platform), config);
    #[cfg(feature = "offline")]
    let _ = game_root.set(Arc::downgrade(&controller));
    Ok(controller)
}

/// Offline play's stack and saves (Windows only, as in the Electron launcher).
#[cfg(feature = "offline")]
fn offline_setup(
    paths: &Paths,
    resources_dir: &std::path::Path,
    encryptor: &Arc<dyn platform::keystore::Encryptor>,
    legacy_encryptor: &Option<Arc<dyn platform::keystore::Encryptor>>,
    game_root: Arc<OnceLock<std::sync::Weak<Controller>>>,
) -> Option<controller::offline::OfflineSetup> {
    use offline::layout::OfflineLayout;
    use offline::stack::{ProcessRuntime, StackManager, before_start};

    if !cfg!(windows) {
        return None;
    }
    let program = std::env::current_exe().ok()?;
    let layout = OfflineLayout::new(&paths.user_data, resources_dir);
    // The offline identity stays in userData: no Documents copy, unlike online keys
    let keys = platform::keystore::KeyStore::new(paths.keys_dir(), encryptor.clone(), legacy_encryptor.clone(), None);
    let executable = move || {
        let root = game_root
            .get()
            .and_then(std::sync::Weak::upgrade)
            .map(|controller| controller.install_dir())
            .unwrap_or_default();
        std::path::Path::new(&root).join(core::pins::EXE_RELATIVE_PATH)
    };
    // Server diagnostics are opt-in apart from the game's own console setting
    let server_console = std::env::var("DAUNTLESS_REVIVED_LOCAL_SERVER_CONSOLE").as_deref() == Ok("1");
    let prepare = before_start(
        layout.clone(),
        executable,
        platform::game_config::default_config_dir(),
        server_console,
    );
    let runtime = ProcessRuntime::offline(program, &layout, prepare);
    Some(controller::offline::OfflineSetup {
        stack: Arc::new(StackManager::new(Arc::new(runtime))),
        save: Arc::new(offline::save::PortableSave::new(layout, paths.user_data.clone(), keys)),
        free_memory: Some(offline::free_memory),
        port: 61000,
    })
}
