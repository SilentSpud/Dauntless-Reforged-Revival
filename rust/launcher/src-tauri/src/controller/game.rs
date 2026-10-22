//! The game folder (Reforged's `controller.ts` 949–1077): find, choose and check an existing Dauntless
//! 1.14.7, on RA9's `platform::game_folder`. It replaces DR's install folder and 1.4.4 download
//! (`choose_install_dir`, `use_existing_game_folder`, `use_existing_game_path`): the launcher never
//! downloads the game.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{Controller, lock, under};
use crate::bridge::types::{
    ActionResult, DetectedGame, ErrorCode, GameInfo, GameProblem, NoticeCode, TaskKind, TaskProgress,
};
use crate::core::pins::EXE_NAME;
use crate::net::verify::{Abort, VerifyError};
use crate::platform::dialogs::FolderPurpose;
use crate::platform::game_folder::{
    GameFolder, StoreEnv, detect_store_installs, has_game_exe, locate_existing_game, read_game_version,
};
use crate::platform::log;

/// How often the `verify` task tells the page how far it is.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

impl Controller {
    fn may_change_game(&self) -> bool {
        !self.st().is_busy() && !self.game.running()
    }

    /// Looks at the chosen game folder (and, with `detect`, asks the stores what is installed). Uses the
    /// cached hash of the executable only: `verify_game` hashes it.
    pub(super) async fn inspect_game(&self, detect: bool) {
        let host = self.c.host;
        let detected: Vec<DetectedGame> = if detect {
            let env = self.seams.store_env.clone().unwrap_or_else(StoreEnv::from_system);
            tokio::task::spawn_blocking(move || detect_store_installs(&env, host))
                .await
                .unwrap_or_default()
        } else {
            self.st().game.detected.clone()
        };
        let mut root = self.s().game_root;
        // Nothing chosen yet: take an install the stores know about, the 1.14.7 one first
        if root.is_none() && !detected.is_empty() {
            let roots: Vec<String> = detected.iter().map(|found| found.root.clone()).collect();
            let versions = tokio::task::spawn_blocking(move || {
                roots
                    .iter()
                    .map(|root| read_game_version(&PathBuf::from(root)).ok)
                    .collect::<Vec<bool>>()
            })
            .await
            .unwrap_or_default();
            let pick = versions
                .iter()
                .position(|ok| *ok)
                .map(|index| &detected[index])
                .or_else(|| (detected.len() == 1).then(|| &detected[0]));
            if let Some(pick) = pick {
                root = Some(pick.root.clone());
                self.update_settings(|map| super::stored::set_text(map, "gameRoot", Some(&pick.root)))
                    .await;
                let source = serde_json::to_value(pick.source).unwrap_or_default();
                log::info(&format!(
                    "using the Dauntless install found through {}",
                    source.as_str().unwrap_or("a store")
                ));
            }
        }
        let info = self.game_info(root, detected).await;
        self.st().game = info;
    }

    async fn game_info(&self, root: Option<String>, detected: Vec<DetectedGame>) -> GameInfo {
        let mut info = GameInfo {
            root: root.clone(),
            problem: None,
            version_text: None,
            version_ok: false,
            exe_verified: false,
            detected,
        };
        let Some(root) = root else {
            info.problem = Some(GameProblem::NotSet);
            return info;
        };
        let looked = {
            let root = root.clone();
            tokio::task::spawn_blocking(move || has_game_exe(&root).then(|| read_game_version(&PathBuf::from(&root))))
                .await
                .ok()
                .flatten()
        };
        let Some(version) = looked else {
            info.problem = Some(GameProblem::NotFound);
            return info;
        };
        info.version_ok = version.ok;
        let exe = under(&root, &self.c.exe_pin.0);
        let cached = self.hashes.lock().await.cached(&exe);
        info.problem = match cached {
            None if version.text.is_some() && !version.ok => Some(GameProblem::WrongVersion),
            None => Some(GameProblem::Unverified),
            Some(digest) if digest != self.c.exe_pin.1 => Some(GameProblem::WrongVersion),
            Some(_) => {
                info.exe_verified = true;
                None
            }
        };
        info.version_text = version.text;
        info
    }

    /// Hashes the executable (about 120 MB) as a `verify` task and compares it with the pinned 1.14.7
    /// one.
    pub async fn verify_game(&self) -> ActionResult {
        let abort = Abort::default();
        let root = {
            let mut st = self.st();
            if st.is_busy() || self.game.running() {
                return ActionResult::err(ErrorCode::Busy);
            }
            let Some(root) = st.game.root.clone() else {
                drop(st);
                return ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None));
            };
            st.abort = Some(abort.clone());
            root
        };
        let exe = under(&root, &self.c.exe_pin.0);
        let Ok(metadata) = tokio::fs::metadata(&exe).await else {
            self.st().abort = None;
            self.inspect_game(false).await;
            return ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None));
        };
        let total = metadata.len();
        let progress = move |done: u64| TaskProgress {
            kind: TaskKind::Verify,
            total_bytes: total,
            done_bytes: done,
            files_total: 1,
            files_done: 0,
            bytes_per_second: 0.0,
            eta_seconds: None,
            current_file: Some(EXE_NAME.to_owned()),
        };
        self.st().task = Some(progress(0));
        self.publish();
        let started = Instant::now();
        let done = Arc::new(AtomicU64::new(0));
        let last = Arc::new(std::sync::Mutex::new(Instant::now()));
        let me = self.me.clone();
        let on_bytes = Arc::new(move |n: u64| {
            let done = done.fetch_add(n, Ordering::Relaxed) + n;
            let mut last = lock(&last);
            if last.elapsed() > PROGRESS_EVERY {
                *last = Instant::now();
                if let Some(this) = me.upgrade() {
                    this.set_task(progress(done));
                }
            }
        });
        let digest = {
            let mut hashes = self.hashes.lock().await;
            let digest = hashes.hash(&exe, abort, on_bytes).await;
            let _ = hashes.save().await;
            digest
        };
        {
            let mut st = self.st();
            st.task = None;
            st.abort = None;
        }
        let outcome = match digest {
            Ok(digest) => {
                let build = if digest == self.c.exe_pin.1 {
                    "Dauntless 1.14.7"
                } else {
                    "another build"
                };
                log::info(&format!(
                    "game executable checked in {} ms: {build}",
                    started.elapsed().as_millis()
                ));
                self.inspect_game(false).await;
                self.inspect_runtime().await;
                if self.st().game.problem == Some(GameProblem::WrongVersion) {
                    ActionResult::Err(self.fail(ErrorCode::GameWrongVersion, None))
                } else {
                    self.st().notice = Some(NoticeCode::GameVerified);
                    ActionResult::Ok
                }
            }
            Err(VerifyError::Aborted) => ActionResult::err(ErrorCode::Cancelled),
            Err(VerifyError::Io(error)) => {
                log::error(&format!(
                    "could not check the game executable: {}",
                    log::describe_error(&error)
                ));
                ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None))
            }
        };
        self.publish();
        outcome
    }

    /// "Choose folder…": a folder dialog, then the same checks as a pasted path.
    pub async fn choose_game_folder(&self) -> ActionResult {
        if !self.may_change_game() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let start = PathBuf::from(self.st().game.root.clone().unwrap_or_default());
        let Some(picked) = self.p.choose_folder(self.language(), FolderPurpose::Game, start).await else {
            return ActionResult::err(ErrorCode::Cancelled);
        };
        self.use_game_path(&picked.to_string_lossy()).await
    }

    /// A pasted path or a picked folder: the Dauntless folder, its parent, `Archon` or `Win64`; verified
    /// at once when unverified.
    pub async fn use_game_path(&self, game_path: &str) -> ActionResult {
        if !self.may_change_game() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let host = self.c.host;
        let found = {
            let input = game_path.to_owned();
            tokio::task::spawn_blocking(move || locate_existing_game(&input, host))
                .await
                .unwrap_or(GameFolder::InvalidPath)
        };
        // A pasted path leaves the window usable while the folder is looked at
        if !self.may_change_game() {
            return ActionResult::err(ErrorCode::Busy);
        }
        let root = match found {
            GameFolder::Found(root) => root,
            GameFolder::NotFound => return ActionResult::Err(self.fail(ErrorCode::GameFolderNotFound, None)),
            GameFolder::InvalidPath | GameFolder::TooLong => {
                return ActionResult::Err(self.fail(ErrorCode::FolderInvalid, None));
            }
        };
        self.update_settings(|map| super::stored::set_text(map, "gameRoot", Some(&root)))
            .await;
        self.inspect_game(false).await;
        self.inspect_runtime().await;
        self.publish();
        if self.st().game.problem == Some(GameProblem::Unverified) {
            return self.verify_game().await;
        }
        ActionResult::Ok
    }

    /// One of `game.detected`, by index (already checked to be below 16).
    pub async fn use_detected_game(&self, index: usize) -> ActionResult {
        let root = self.st().game.detected.get(index).map(|found| found.root.clone());
        match root {
            Some(root) => self.use_game_path(&root).await,
            None => ActionResult::Err(self.fail(ErrorCode::FolderInvalid, None)),
        }
    }

    pub fn open_game_folder(&self) {
        let root = self.st().game.root.clone();
        if let Some(root) = root.map(PathBuf::from).filter(|root| root.exists()) {
            self.p.open_folder(&root);
        }
    }
}
