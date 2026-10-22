//! The server's game runtime (Reforged's `controller.ts` 1081–1226): accept a server's runtime set once,
//! install it next to the game, or use a host's local build instead, on RA7's `net::runtime_files`. It
//! replaces DR's 1.4.4 download, repair, pause and resume. This is plan section 6's "runtime acceptance
//! per server": `trustedRuntimes[serverId]` holds the ids of the sets the player accepted.

use std::path::{Path, PathBuf};
use std::time::Instant;

use protocol::manifest::RUNTIME_FILE_NAMES;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{Controller, State};
use crate::bridge::types::{
    ActionResult, ConnectProblem, ErrorCode, NoticeCode, RuntimeInfo, RuntimeState, TaskKind, TaskProgress,
};
use crate::net::hostapi::key_headers;
use crate::net::runtime_files::{
    InstallRequest, RuntimeErrorCode, RuntimeManifestResult, RuntimeProgress, accept, check_runtime,
    fetch_runtime_manifest, file_infos, install_runtime, is_complete_runtime_dir, is_trusted,
};
use crate::platform::dialogs::FolderPurpose;
use crate::platform::log;

impl Controller {
    /// Whether the player accepted the set the server offers now.
    fn trusted_set(st: &State) -> bool {
        let (Some(sv), Some(manifest)) = (st.s().server.as_ref(), st.runtime_manifest.as_ref()) else {
            return false;
        };
        st.s()
            .trusted_runtimes
            .get(&sv.id())
            .is_some_and(|trusted| is_trusted(trusted, &manifest.files))
    }

    /// Asks the server what it offers; a certificate that is not the invite's flips the connection.
    pub(super) async fn refresh_runtime_offer(&self) {
        let Some(endpoint) = self.server().map(|sv| sv.endpoint()) else {
            return;
        };
        match fetch_runtime_manifest(&endpoint).await {
            RuntimeManifestResult::Ok(manifest) => self.st().runtime_manifest = Some(manifest),
            RuntimeManifestResult::None => self.st().runtime_manifest = None,
            RuntimeManifestResult::CertMismatch => {
                let mut st = self.st();
                st.reachable = Some(false);
                st.connect_problem = Some(ConnectProblem::CertMismatch);
            }
            RuntimeManifestResult::BadAnswer | RuntimeManifestResult::Unreachable => {}
        }
        self.inspect_runtime().await;
    }

    /// The runtime next to the game against the server's list (`check_runtime`, hashing both files).
    pub(super) async fn inspect_runtime(&self) {
        let (dir, manifest, trusted, local_dir) = {
            let st = self.st();
            (
                Self::win64_dir_of(&st),
                st.runtime_manifest.clone(),
                Self::trusted_set(&st),
                st.s().local_runtime_dir.clone(),
            )
        };
        let version = manifest.as_ref().map(|manifest| manifest.version.clone());
        let files = manifest
            .as_ref()
            .map(|manifest| file_infos(&manifest.files))
            .unwrap_or_default();
        let (state, playable) = match &dir {
            None if manifest.is_some() => (RuntimeState::Missing, false),
            None => (RuntimeState::Unavailable, false),
            Some(dir) => check_runtime(dir, manifest.as_ref(), trusted, local_dir.as_deref().map(Path::new)).await,
        };
        let mut st = self.st();
        st.runtime = RuntimeInfo {
            state,
            version,
            files,
            local_dir,
        };
        st.runtime_playable = playable;
    }

    /// The player accepted the runtime the server offers: the exact file set is remembered for this
    /// server, then installed.
    pub async fn trust_runtime(&self) -> ActionResult {
        let (sid, manifest) = {
            let st = self.st();
            (st.s().server.as_ref().map(|sv| sv.id()), st.runtime_manifest.clone())
        };
        let (Some(sid), Some(manifest)) = (sid, manifest) else {
            return ActionResult::Err(self.fail(ErrorCode::RuntimeUnavailable, None));
        };
        let mut trusted = self.s().trusted_runtimes.get(&sid).cloned().unwrap_or_default();
        let id = accept(&mut trusted, &manifest.files);
        self.update_settings(|map| {
            let list = Value::Array(trusted.into_iter().map(Value::String).collect());
            match map.get_mut("trustedRuntimes") {
                Some(Value::Object(all)) => {
                    all.insert(sid, list);
                }
                _ => {
                    map.insert("trustedRuntimes".into(), serde_json::json!({ sid: list }));
                }
            }
        })
        .await;
        log::info(&format!(
            "runtime {} of this server accepted ({})",
            manifest.version,
            &id[..12]
        ));
        self.install_runtime().await
    }

    /// Downloads the trusted runtime into `Win64` as a `runtime` task, with the account key, over the
    /// same pinned connection as everything else.
    pub async fn install_runtime(&self) -> ActionResult {
        if self.st().is_busy() {
            return ActionResult::err(ErrorCode::Busy);
        }
        if self.game.running() {
            return ActionResult::Err(self.fail(ErrorCode::AlreadyRunning, None));
        }
        let (sv, manifest, dir, trusted) = {
            let st = self.st();
            (
                st.s().server.clone(),
                st.runtime_manifest.clone(),
                Self::win64_dir_of(&st),
                Self::trusted_set(&st),
            )
        };
        let Some(sv) = sv else {
            return ActionResult::Err(self.fail(ErrorCode::ServerUnreachable, None));
        };
        let Some(dir) = dir else {
            return ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None));
        };
        let Some(manifest) = manifest else {
            return ActionResult::Err(self.fail(ErrorCode::RuntimeUnavailable, None));
        };
        if !trusted {
            return ActionResult::Err(self.fail(ErrorCode::RuntimeUntrusted, None));
        }
        let Some(key) = self.keys.load(&sv.slot()).ok().flatten() else {
            return ActionResult::Err(self.fail(ErrorCode::KeyInvalid, None));
        };
        let cancel = CancellationToken::new();
        let total: u64 = manifest.files.iter().map(|file| file.size).sum();
        {
            let mut st = self.st();
            if st.is_busy() {
                return ActionResult::err(ErrorCode::Busy);
            }
            st.cancel = Some(cancel.clone());
            st.last_error = None;
            st.task = Some(TaskProgress {
                kind: TaskKind::Runtime,
                total_bytes: total,
                done_bytes: 0,
                files_total: manifest.files.len() as u32,
                files_done: 0,
                bytes_per_second: 0.0,
                eta_seconds: None,
                current_file: None,
            });
        }
        self.publish();
        let started = Instant::now();
        let on_progress = |progress: RuntimeProgress| {
            let seconds = started.elapsed().as_secs_f64().max(0.001);
            let rate = progress.done_bytes as f64 / seconds;
            self.set_task(TaskProgress {
                kind: TaskKind::Runtime,
                total_bytes: progress.total_bytes,
                done_bytes: progress.done_bytes,
                files_total: progress.files_total as u32,
                files_done: progress.files_done as u32,
                bytes_per_second: rate,
                eta_seconds: (rate > 0.0)
                    .then(|| progress.total_bytes.saturating_sub(progress.done_bytes) as f64 / rate),
                current_file: progress.current_file,
            });
        };
        let endpoint = sv.endpoint();
        let installed = install_runtime(InstallRequest {
            endpoint: &endpoint,
            headers: key_headers(&endpoint, Some(&key)),
            manifest: &manifest,
            win64_dir: &dir,
            cancel: Some(cancel),
            on_progress: Some(&on_progress),
        })
        .await;
        let outcome = match installed {
            Ok(done) => {
                log::info(&format!("runtime {} installed next to the game", manifest.version));
                // The old files were loaded somewhere: on a host's PC, its own game servers. They keep the
                // old runtime until the server restarts
                if !done.replaced_in_use.is_empty() {
                    log::info(&format!(
                        "runtime: replaced while in use ({})",
                        done.replaced_in_use.join(", ")
                    ));
                }
                let mut st = self.st();
                st.notice = Some(if !done.replaced_in_use.is_empty() && st.host_server_up {
                    NoticeCode::RuntimeInstalledRestart
                } else {
                    NoticeCode::RuntimeInstalled
                });
                ActionResult::Ok
            }
            Err(error) => {
                log::error(&format!("runtime install stopped: {error}"));
                let code = match error.code {
                    RuntimeErrorCode::Cancelled => None,
                    RuntimeErrorCode::KeyRejected => {
                        self.st().key_rejected = true;
                        Some(ErrorCode::KeyRejected)
                    }
                    RuntimeErrorCode::CertMismatch => {
                        let mut st = self.st();
                        st.reachable = Some(false);
                        st.connect_problem = Some(ConnectProblem::CertMismatch);
                        Some(ErrorCode::CertMismatch)
                    }
                    RuntimeErrorCode::NotFound => Some(ErrorCode::RuntimeUnavailable),
                    RuntimeErrorCode::Different => Some(ErrorCode::RuntimeDifferentOnServer),
                    RuntimeErrorCode::DownloadFailed => Some(ErrorCode::DownloadFailed),
                    RuntimeErrorCode::DiskError => Some(ErrorCode::FolderInvalid),
                    RuntimeErrorCode::InUse => Some(ErrorCode::RuntimeInUse),
                    RuntimeErrorCode::Failed => Some(ErrorCode::RuntimeFailed),
                };
                match code {
                    None => ActionResult::err(ErrorCode::Cancelled),
                    // The certificate mismatch has no file to name
                    Some(ErrorCode::CertMismatch) => ActionResult::Err(self.fail(ErrorCode::CertMismatch, None)),
                    Some(code) => ActionResult::Err(self.fail(code, error.file)),
                }
            }
        };
        {
            let mut st = self.st();
            st.task = None;
            st.cancel = None;
        }
        self.inspect_runtime().await;
        self.publish();
        outcome
    }

    /// Host or developer option: a folder with `MysticParadox.dll` and `winmm.dll` built on this PC, used
    /// instead of what the server offers until cleared.
    pub async fn choose_local_runtime(&self) -> ActionResult {
        if self.st().is_busy() || self.game.running() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let start = PathBuf::from(self.s().local_runtime_dir.unwrap_or_default());
        let Some(picked) = self
            .p
            .choose_folder(self.language(), FolderPurpose::Runtime, start)
            .await
        else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        if !is_complete_runtime_dir(&picked).await {
            return ActionResult::Err(self.fail(ErrorCode::LocalRuntimeInvalid, Some(RUNTIME_FILE_NAMES.join(", "))));
        }
        let picked = picked.to_string_lossy().into_owned();
        self.update_settings(|map| super::stored::set_text(map, "localRuntimeDir", Some(&picked)))
            .await;
        self.inspect_runtime().await;
        self.publish();
        ActionResult::Ok
    }

    pub async fn clear_local_runtime(&self) -> ActionResult {
        self.update_settings(|map| super::stored::set_text(map, "localRuntimeDir", None))
            .await;
        self.inspect_runtime().await;
        self.publish();
        ActionResult::Ok
    }
}
