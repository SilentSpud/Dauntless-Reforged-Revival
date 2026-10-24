//! Timeouts: a ban the admin API lifts itself when it runs out. The backend has no ban end time, so
//! the end is kept here, in one JSON file the host kit names, rewritten whole (temp file, then rename)
//! on every change.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeout {
    pub user_id: String,
    pub username: String,
    /// When it ends, ms since 1970.
    pub until: i64,
    /// The ban reason the backend holds. A different one at the end means someone banned the player
    /// again since, so the ban is left alone.
    pub reason: String,
    pub by: String,
    pub at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    timeouts: Vec<Timeout>,
}

pub struct Timeouts {
    file: PathBuf,
    entries: Mutex<Vec<Timeout>>,
}

fn save(file: &Path, entries: &[Timeout]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&File {
        version: 1,
        timeouts: entries.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))
}

impl Timeouts {
    /// A missing file is no timeouts. One that cannot be read stops the start: running on without it
    /// would leave every timed-out player banned for good.
    pub fn load(file: &Path) -> Result<Self, String> {
        let entries = match std::fs::read(file) {
            Ok(bytes) => {
                let parsed: File = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
                parsed.timeouts
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        Ok(Self {
            file: file.to_path_buf(),
            entries: Mutex::new(entries),
        })
    }

    pub async fn get(&self, user_id: &str) -> Option<Timeout> {
        self.entries.lock().await.iter().find(|t| t.user_id == user_id).cloned()
    }

    /// Records `timeout`, replacing the player's earlier one.
    pub async fn put(&self, timeout: Timeout) -> Result<(), String> {
        let mut entries = self.entries.lock().await;
        let mut next: Vec<Timeout> = entries
            .iter()
            .filter(|t| t.user_id != timeout.user_id)
            .cloned()
            .collect();
        next.push(timeout);
        save(&self.file, &next)?;
        *entries = next;
        Ok(())
    }

    pub async fn remove(&self, user_id: &str) -> Result<(), String> {
        let mut entries = self.entries.lock().await;
        if !entries.iter().any(|t| t.user_id == user_id) {
            return Ok(());
        }
        let next: Vec<Timeout> = entries.iter().filter(|t| t.user_id != user_id).cloned().collect();
        save(&self.file, &next)?;
        *entries = next;
        Ok(())
    }

    /// The timeouts that have ended by `now`.
    pub async fn due(&self, now: i64) -> Vec<Timeout> {
        self.entries
            .lock()
            .await
            .iter()
            .filter(|t| t.until <= now)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeout(user_id: &str, until: i64) -> Timeout {
        Timeout {
            user_id: user_id.into(),
            username: format!("name_{user_id}"),
            until,
            reason: "Timed out".into(),
            by: "42".into(),
            at: 1,
        }
    }

    #[tokio::test]
    async fn survive_a_restart_and_come_due_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("admin").join("timeouts.json");
        let store = Timeouts::load(&file).unwrap();
        assert!(store.due(i64::MAX).await.is_empty());
        store.put(timeout("a", 100)).await.unwrap();
        store.put(timeout("b", 200)).await.unwrap();
        store.put(timeout("a", 300)).await.unwrap();

        let again = Timeouts::load(&file).unwrap();
        assert_eq!(again.get("a").await.map(|t| t.until), Some(300));
        assert_eq!(
            again
                .due(250)
                .await
                .iter()
                .map(|t| t.user_id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        again.remove("b").await.unwrap();
        again.remove("nobody").await.unwrap();
        assert_eq!(Timeouts::load(&file).unwrap().due(i64::MAX).await.len(), 1);
        assert!(!dir.path().join("admin").join("timeouts.json.tmp").exists());
    }

    #[test]
    fn a_damaged_file_stops_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("timeouts.json");
        std::fs::write(&file, "{").unwrap();
        assert!(Timeouts::load(&file).err().unwrap().contains("timeouts.json"));
    }
}
