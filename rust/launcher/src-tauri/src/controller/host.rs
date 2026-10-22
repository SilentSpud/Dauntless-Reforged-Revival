//! The Host page (Reforged's `controller.ts` 1765–2351): the host kit's workspace, requirements, builds,
//! game data and the server a host runs from the launcher. RA2's stub; RA12 fills it for the planned
//! layout only (RD23), on `platform::hostkit`, `platform::hostserver`, `platform::requirements` and
//! `platform::injector`, driving `reforged-host`.

use super::Controller;
use crate::bridge::types::{
    ActionResult, GameDataState, HostAction, HostBuilds, HostExports, HostFolder, HostGameData, HostKit,
    HostMultipliers, HostSdk, HostServerConfig, HostServerState, HostServerView, HostSetupRequest, HostToolchain,
    HostUpstream,
};

const ROW: &str = "RA12";

/// What the Host page shows before anything is detected.
fn empty_kit() -> HostKit {
    HostKit {
        workspace: None,
        workspace_ok: false,
        upstream: HostUpstream {
            present: false,
            patched: false,
        },
        sdk: HostSdk {
            present: false,
            function_files: 0,
        },
        dumps: Vec::new(),
        dumper_dll: None,
        builds: HostBuilds {
            runtime: false,
            loader: false,
            exporter: false,
            published: false,
        },
        toolchain: HostToolchain {
            visual_studio: None,
            msbuild: false,
            cargo: false,
        },
        game_data: HostGameData {
            backend: GameDataState::Missing,
            director: GameDataState::Missing,
        },
        exports: HostExports {
            present: false,
            in_game: false,
        },
        server_config: HostServerConfig {
            configured: false,
            name: None,
            mode: None,
            host: None,
            multipliers: None,
        },
        requirements: Vec::new(),
        job: None,
        update: None,
        project_target: None,
    }
}

impl Controller {
    /// `hostGet`: the requirements are looked at afresh each time the page asks.
    pub async fn host_kit(&self) -> HostKit {
        self.stub("host_kit", ROW);
        empty_kit()
    }

    pub async fn host_choose_workspace(&self) -> ActionResult {
        self.stub("host_choose_workspace", ROW).into()
    }

    pub async fn host_choose_dumper(&self) -> ActionResult {
        self.stub("host_choose_dumper", ROW).into()
    }

    pub async fn host_import_gamedata(&self) -> ActionResult {
        self.stub("host_import_gamedata", ROW).into()
    }

    pub async fn host_import_sdk_folder(&self) -> ActionResult {
        self.stub("host_import_sdk_folder", ROW).into()
    }

    /// `ensureHostWorkspace`: find or download the project beside the game folder.
    pub async fn host_download_project(&self) -> ActionResult {
        self.stub("host_download_project", ROW).into()
    }

    pub async fn host_action(&self, _action: HostAction) -> ActionResult {
        self.stub("host_action", ROW).into()
    }

    pub fn host_open(&self, _folder: HostFolder) -> ActionResult {
        self.stub("host_open", ROW).into()
    }

    pub fn host_server_view(&self) -> HostServerView {
        self.stub("host_server_view", ROW);
        HostServerView {
            state: HostServerState::Stopped,
            message: None,
            lines: Vec::new(),
            invite: None,
        }
    }

    pub async fn host_server_start(&self) -> ActionResult {
        self.stub("host_server_start", ROW).into()
    }

    pub async fn host_server_stop(&self) -> ActionResult {
        self.stub("host_server_stop", ROW).into()
    }

    /// The setup form, already checked (`commands::host_setup_request`).
    pub async fn host_setup(&self, _request: HostSetupRequest) -> ActionResult {
        self.stub("host_setup", ROW).into()
    }

    /// Who the invite is for, already trimmed and checked; None for nobody in particular.
    pub async fn host_create_invite(&self, _for_name: Option<String>) -> ActionResult {
        self.stub("host_create_invite", ROW).into()
    }

    pub async fn host_update(&self) -> ActionResult {
        self.stub("host_update", ROW).into()
    }

    /// None turns the multipliers off.
    pub async fn host_set_multipliers(&self, _multipliers: Option<HostMultipliers>) -> ActionResult {
        self.stub("host_set_multipliers", ROW).into()
    }
}
