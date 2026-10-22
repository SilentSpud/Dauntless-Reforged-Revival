//! What the commands keep besides the controller (row L38 owns the rest): the progress channel the page
//! subscribed, and an invite link waiting for the page.

use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};

use super::progress::Throttle;
use super::types::TaskProgress;
use crate::window::MAIN;

/// Pushed to the page whenever the snapshot changes.
pub const SNAPSHOT_EVENT: &str = "snapshot";

#[derive(Default)]
pub struct Backend {
    progress: Mutex<Option<(Channel<TaskProgress>, Throttle)>>,
    /// An invite link the launcher was started with or handed, until the page takes it
    invite_link: Mutex<Option<String>>,
}

/// Tells the page an invite link is waiting (`onInviteLink`).
pub const INVITE_LINK_EVENT: &str = "invite-link";

/// A poisoned lock only means a panic elsewhere while holding it; the data is still whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Backend {
    /// Keeps an invite link for the page and tells it one is waiting.
    pub fn deliver_invite_link(&self, app: Option<&AppHandle>, link: Option<String>) {
        let Some(link) = link else { return };
        *lock(&self.invite_link) = Some(link);
        if let Some(app) = app {
            let _ = app.emit_to(MAIN, INVITE_LINK_EVENT, ());
        }
    }

    pub fn take_invite_link(&self) -> Option<String> {
        lock(&self.invite_link).take()
    }

    pub fn subscribe_progress(&self, channel: Channel<TaskProgress>) {
        *lock(&self.progress) = Some((channel, Throttle::default()));
    }

    /// Sends a progress value to the page, unless the last one went out too recently.
    pub fn push_progress(&self, progress: &TaskProgress) {
        if let Some((channel, throttle)) = lock(&self.progress).as_mut()
            && throttle.pass(Instant::now(), progress)
        {
            let _ = channel.send(progress.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::bridge::types::TaskKind;

    #[test]
    fn progress_reaches_the_subscribed_channel_throttled() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&sent);
        let channel = Channel::new(move |body| {
            lock(&sink).push(format!("{body:?}"));
            Ok(())
        });
        let backend = Backend::default();
        let mut progress = TaskProgress {
            kind: TaskKind::Download,
            paused: false,
            total_bytes: 10,
            done_bytes: 1,
            files_total: 1,
            files_done: 0,
            bytes_per_second: 0.0,
            eta_seconds: None,
            current_file: None,
        };
        backend.push_progress(&progress);
        assert!(lock(&sent).is_empty(), "nothing goes out before the page subscribes");
        backend.subscribe_progress(channel);
        backend.push_progress(&progress);
        progress.done_bytes = 2;
        backend.push_progress(&progress);
        assert_eq!(lock(&sent).len(), 1);
        progress.paused = true;
        backend.push_progress(&progress);
        assert_eq!(lock(&sent).len(), 2);
    }
}
