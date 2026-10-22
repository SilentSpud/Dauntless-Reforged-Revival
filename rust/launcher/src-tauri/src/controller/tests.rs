//! The skeleton controller (RA2): a first start's snapshot, and every stub answering "not implemented"
//! with the row that fills it. RA13 replaces these with DR's and Reforged's controller tests.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::platform::{Boxed, Platform, Prepared};
use super::{Config, Controller, ControllerHandle};
use crate::bridge::types::{
    ActionResult, ErrorCode, ExternalTarget, GameProblem, HostAction, HostFolder, HostKit, HostServerState,
    HostServerView, HostSetupRequest, InviteCheck, Language, LauncherError, Phase, RegisterOutcome, RuntimeState,
    Snapshot, TaskProgress,
};
use crate::platform::dialogs::FolderPurpose;
use crate::platform::injector::InjectError;
use crate::platform::keystore::Encryptor;

struct NoKeys;

impl Encryptor for NoKeys {
    fn is_available(&self) -> bool {
        false
    }

    fn encrypt(&self, _: &str) -> Option<Vec<u8>> {
        None
    }

    fn decrypt(&self, _: &[u8]) -> Option<String> {
        None
    }
}

/// Records the snapshots the controller pushes; everything else does nothing.
#[derive(Default)]
struct Fake {
    snapshots: Mutex<Vec<Snapshot>>,
}

impl Platform for Fake {
    fn choose_folder(&self, _: Language, _: FolderPurpose, _: PathBuf) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn choose_save_file(&self, _: String) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn choose_open_file(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn choose_dll(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn choose_pack(&self) -> Boxed<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn open_url(&self, _: &str, _: Option<&str>) {}

    fn open_folder(&self, _: &Path) {}

    fn emit_snapshot(&self, snapshot: &Snapshot) {
        self.snapshots.lock().unwrap().push(snapshot.clone());
    }

    fn emit_progress(&self, _: &TaskProgress) {}

    fn emit_host_kit(&self, _: &HostKit) {}

    fn emit_host_server(&self, _: &HostServerView) {}

    fn put_art(&self, _: &str, _: PathBuf, _: &'static str) {}

    fn find_tailscale(&self) -> bool {
        false
    }

    fn find_running_clients(&self) -> Boxed<Vec<u32>> {
        Box::pin(async { Vec::new() })
    }

    fn prepare_game_launch(&self) -> Boxed<Prepared> {
        Box::pin(async { Prepared::Native })
    }

    fn inject(&self, _: u32, _: PathBuf) -> Boxed<Result<(), InjectError>> {
        Box::pin(async { Ok(()) })
    }

    fn missing_runtimes(&self) -> (Vec<String>, Vec<String>) {
        (Vec::new(), Vec::new())
    }
}

fn controller() -> (ControllerHandle, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let config = Config {
        user_data: PathBuf::from("user-data"),
        documents: None,
        resources_dir: PathBuf::from("resources"),
        host: crate::platform::settings::Platform::current(),
        app_version: "2.0.0-test".into(),
        packaged: false,
        language: Language::En,
        system_locale: "en-US".into(),
        game_config_dir: PathBuf::from("config"),
        relay_port: 61100,
        exe_pin: (String::new(), String::new()),
        status_poll: Duration::from_secs(15),
        encryptor: Arc::new(NoKeys),
        legacy_encryptor: None,
    };
    (Controller::new(fake.clone(), config), fake)
}

fn stub(row: &str) -> LauncherError {
    LauncherError::with_detail(ErrorCode::Unknown, &format!("not_implemented:{row}"))
}

#[tokio::test]
async fn a_first_start_asks_for_an_invite() {
    let (c, fake) = controller();
    c.init().await;
    let snapshot = fake.snapshots.lock().unwrap().last().cloned().unwrap();
    assert_eq!(snapshot, c.snapshot());
    assert_eq!(snapshot.phase, Phase::Join);
    assert!(snapshot.server.is_none());
    assert_eq!(snapshot.game.problem, Some(GameProblem::NotSet));
    assert_eq!(snapshot.runtime.state, RuntimeState::Unavailable);
    assert_eq!(snapshot.app.version, "2.0.0-test");
    assert_eq!(snapshot.app.platform, if cfg!(windows) { "win32" } else { "linux" });
    assert_eq!(snapshot.settings.language, Language::En);
    let page = serde_json::to_value(&snapshot).unwrap();
    for field in [
        "game",
        "runtime",
        "running",
        "savedServers",
        "statusUnsupported",
        "lastError",
    ] {
        assert!(page.get(field).is_some(), "{field}");
    }
    assert!(page.get("revived").is_none(), "1.4.4 is not ported (RD5)");
}

#[tokio::test]
async fn every_action_answers_its_row() {
    let (c, _) = controller();
    let ra13 = ActionResult::Err(stub("RA13"));
    let ra12 = ActionResult::Err(stub("RA12"));
    assert_eq!(c.check_invite("x"), InviteCheck::Err(stub("RA13")));
    assert_eq!(c.register("Slayer").await, RegisterOutcome::Err(stub("RA13")));
    for answer in [
        c.submit_invite("x", false, false).await,
        c.switch_server("0123456789abcdef01234567").await,
        c.remove_saved_server("0123456789abcdef01234567").await,
        c.forget_server().await,
        c.connect().await,
        c.use_existing_key("x").await,
        c.import_key_file().await,
        c.save_key_backup().await,
        c.logout().await,
        c.play().await,
        c.open_external(ExternalTarget::ProjectSource),
        c.choose_game_folder().await,
        c.use_game_path("C:\\Games").await,
        c.use_detected_game(0).await,
        c.verify_game().await,
        c.trust_runtime().await,
        c.install_runtime().await,
        c.choose_local_runtime().await,
        c.clear_local_runtime().await,
    ] {
        assert_eq!(answer, ra13);
    }
    let setup = HostSetupRequest {
        name: "x".into(),
        mode: crate::bridge::types::HostServerMode::Private,
        public_host: None,
    };
    for answer in [
        c.host_choose_workspace().await,
        c.host_choose_dumper().await,
        c.host_import_gamedata().await,
        c.host_import_sdk_folder().await,
        c.host_download_project().await,
        c.host_action(HostAction::BuildRuntime).await,
        c.host_open(HostFolder::Workspace),
        c.host_server_start().await,
        c.host_server_stop().await,
        c.host_setup(setup).await,
        c.host_create_invite(None).await,
        c.host_update().await,
        c.host_set_multipliers(None).await,
    ] {
        assert_eq!(answer, ra12);
    }
    assert!(c.news().is_empty());
    assert!(c.branding().backgrounds.is_empty());
    assert!(c.host_kit().await.requirements.is_empty());
    assert_eq!(c.host_server_view().state, HostServerState::Stopped);
    assert_eq!(c.set_settings(Default::default()).await, c.snapshot().settings);
}

#[tokio::test]
async fn notices_errors_and_shutdown() {
    let (c, fake) = controller();
    c.set_notice(Some(crate::bridge::types::NoticeCode::KeepOpen));
    assert_eq!(c.snapshot().notice, Some(crate::bridge::types::NoticeCode::KeepOpen));
    c.set_notice(None);
    c.dismiss_error();
    assert_eq!(fake.snapshots.lock().unwrap().len(), 3);
    c.set_update_ready();
    assert!(c.snapshot().app.update_ready);
    assert!(c.may_install_update());
    assert!(c.keep_open().is_none());
    c.shutdown().await;
    assert!(!c.may_install_update());
    assert_eq!(
        c.play().await,
        ActionResult::err(ErrorCode::Busy),
        "no PLAY while shutting down"
    );
}
