//! Account keys, one per server (row L27): `src/main/keystore.ts`. The key never goes to the page; it is
//! read here when a request or the game launch needs it.
//!
//! A public server's key is tied to the certificate fingerprint it was saved with. An invite for the same
//! host:port with another fingerprint (a rebuilt server, or someone in the middle) cannot load it: the
//! key moves to a new fingerprint only through `rebind`, after the player confirmed.
//!
//! Files: `keys/<id>.key2`, encrypted by this launcher (`os.rs`: DPAPI on Windows, AES-256-GCM with a key
//! held by the Secret Service on Linux). The Electron launcher's `keys/<id>.key` (safeStorage) is read once
//! through `legacy.rs`, written in the new format, read back, and left in place (D7). A plain-text copy may
//! sit in the player's Documents (the recovery folder), as the Electron launcher keeps one.

pub mod legacy;
pub mod os;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::core::username::is_plausible_account_key;
use crate::platform::log;

/// Encrypts key files for one user on one PC.
pub trait Encryptor: Send + Sync {
    fn is_available(&self) -> bool;
    fn encrypt(&self, plain: &str) -> Option<Vec<u8>>;
    fn decrypt(&self, data: &[u8]) -> Option<String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStoreError {
    Unavailable,
    Invalid,
    Io,
}

/// One key per server. Private servers keep the plain host:port id; public servers get their own
/// namespace, so a public and a private server on the same address never share a key.
pub fn server_id(host: &str, port: u16, public: bool) -> String {
    let id = if public {
        format!("public:{host}:{port}")
    } else {
        format!("{host}:{port}")
    };
    Sha256::digest(id.as_bytes())
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySlot {
    pub host: String,
    pub port: u16,
    pub public: bool,
    /// Public mode: the certificate fingerprint the key belongs to
    pub fp: Option<String>,
}

impl KeySlot {
    fn id(&self) -> String {
        server_id(&self.host, self.port, self.public)
    }
}

/// What a key file holds after decryption: `DRK2\n<fingerprint or ->\n<key>`. The first format was the
/// key alone; such a key is not tied to any certificate.
const FORMAT: &str = "DRK2";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stored {
    key: String,
    fp: Option<String>,
}

fn parse_stored(text: &str) -> Option<Stored> {
    let parts: Vec<&str> = text.split('\n').collect();
    if parts.len() == 3 && parts[0] == FORMAT {
        let fp = (parts[1] != "-").then(|| parts[1].to_owned());
        if fp
            .as_deref()
            .is_some_and(|fp| !protocol::invite::is_valid_fingerprint(fp))
        {
            return None;
        }
        return is_plausible_account_key(parts[2]).then(|| Stored {
            key: parts[2].to_owned(),
            fp,
        });
    }
    is_plausible_account_key(text).then(|| Stored {
        key: text.to_owned(),
        fp: None,
    })
}

pub struct KeyStore {
    dir: PathBuf,
    crypto: Arc<dyn Encryptor>,
    /// The Electron launcher's safeStorage, for keys it saved
    legacy: Option<Arc<dyn Encryptor>>,
    recovery_dir: Option<PathBuf>,
}

impl KeyStore {
    pub fn new(
        dir: PathBuf,
        crypto: Arc<dyn Encryptor>,
        legacy: Option<Arc<dyn Encryptor>>,
        recovery_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            dir,
            crypto,
            legacy,
            recovery_dir,
        }
    }

    /// The encrypted key file of a slot (the offline save restore keeps a copy of it to roll back).
    pub fn file(&self, slot: &KeySlot) -> PathBuf {
        self.dir.join(format!("{}.key2", slot.id()))
    }

    fn legacy_file(&self, slot: &KeySlot) -> PathBuf {
        self.dir.join(format!("{}.key", slot.id()))
    }

    fn recovery_file(&self, slot: &KeySlot) -> Option<PathBuf> {
        self.recovery_dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.txt", slot.id())))
    }

    fn read(&self, slot: &KeySlot) -> Result<Option<Stored>, KeyStoreError> {
        if let Ok(data) = std::fs::read(self.file(slot)) {
            if !self.crypto.is_available() {
                return Err(KeyStoreError::Unavailable);
            }
            // Encrypted by another user or PC: no key
            return Ok(self.crypto.decrypt(&data).and_then(|text| parse_stored(&text)));
        }
        if let Ok(data) = std::fs::read(self.legacy_file(slot))
            && let Some(legacy) = self.legacy.as_ref().filter(|legacy| legacy.is_available())
            && let Some(stored) = legacy.decrypt(&data).and_then(|text| parse_stored(&text))
        {
            self.migrate(slot, &stored);
            return Ok(Some(stored));
        }
        let Some(recovery) = self.recovery_file(slot) else {
            return Ok(None);
        };
        Ok(std::fs::read_to_string(recovery)
            .ok()
            .and_then(|text| parse_stored(&text)))
    }

    /// Writes an Electron key in the new format and reads it back; the old file stays where it is.
    fn migrate(&self, slot: &KeySlot, stored: &Stored) {
        let text = format!("{FORMAT}\n{}\n{}", stored.fp.as_deref().unwrap_or("-"), stored.key);
        let written = self.write_encrypted(slot, &text).is_ok();
        let read_back = std::fs::read(self.file(slot))
            .ok()
            .and_then(|data| self.crypto.decrypt(&data));
        if written && read_back.as_deref() == Some(text.as_str()) {
            log::info("moved an account key from the Electron launcher's key storage");
        } else {
            let _ = std::fs::remove_file(self.file(slot));
            log::warn("an account key from the Electron launcher could not be stored again; the old file is kept");
        }
    }

    fn write_encrypted(&self, slot: &KeySlot, text: &str) -> Result<(), KeyStoreError> {
        let data = self.crypto.encrypt(text).ok_or(KeyStoreError::Unavailable)?;
        std::fs::create_dir_all(&self.dir).map_err(|_| KeyStoreError::Io)?;
        let target = self.file(slot);
        let mut temporary = target.clone().into_os_string();
        temporary.push(".tmp");
        std::fs::write(&temporary, data).map_err(|_| KeyStoreError::Io)?;
        std::fs::rename(&temporary, &target).map_err(|_| KeyStoreError::Io)
    }

    pub fn has(&self, slot: &KeySlot) -> bool {
        self.load(slot).ok().flatten().is_some()
    }

    pub fn save(&self, slot: &KeySlot, key: &str) -> Result<(), KeyStoreError> {
        if !is_plausible_account_key(key) {
            return Err(KeyStoreError::Invalid);
        }
        let fp = match (&slot.fp, slot.public) {
            (Some(fp), true) if protocol::invite::is_valid_fingerprint(fp) => fp.as_str(),
            (_, true) => return Err(KeyStoreError::Invalid),
            (_, false) => "-",
        };
        if !self.crypto.is_available() {
            return Err(KeyStoreError::Unavailable);
        }
        log::add_secret(key);
        let text = format!("{FORMAT}\n{fp}\n{key}");
        self.write_encrypted(slot, &text)?;
        if let Some(backup) = self.recovery_file(slot) {
            let written = backup.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
                && write_private(&backup, &text).is_ok();
            if !written {
                log::warn("Automatic Documents account backup could not be written");
            }
        }
        Ok(())
    }

    /// The key for this slot, or None. A public server's key is returned only for the exact certificate
    /// it was saved with.
    pub fn load(&self, slot: &KeySlot) -> Result<Option<String>, KeyStoreError> {
        let Some(stored) = self.read(slot)? else {
            return Ok(None);
        };
        if slot.public && (slot.fp.is_none() || stored.fp != slot.fp) {
            return Ok(None);
        }
        log::add_secret(&stored.key);
        if let Some(backup) = self.recovery_file(slot)
            && !backup.exists()
            && self.save(slot, &stored.key).is_err()
        {
            log::warn("Automatic Documents account backup unavailable");
        }
        Ok(Some(stored.key))
    }

    /// Public servers: whether a key is stored for this host:port, and which certificate it belongs to
    /// (`Some(None)`: a key from before keys were tied to certificates). None: no key stored.
    pub fn certificate_of(&self, slot: &KeySlot) -> Option<Option<String>> {
        self.read(slot).ok().flatten().map(|stored| stored.fp)
    }

    /// Moves this host:port's key to a new certificate. Only after the player confirmed the change.
    pub fn rebind(&self, slot: &KeySlot, fp: &str) -> Result<bool, KeyStoreError> {
        if !slot.public || !protocol::invite::is_valid_fingerprint(fp) {
            return Err(KeyStoreError::Invalid);
        }
        let Some(stored) = self.read(slot)? else {
            return Ok(false);
        };
        self.save(
            &KeySlot {
                fp: Some(fp.to_owned()),
                ..slot.clone()
            },
            &stored.key,
        )?;
        Ok(true)
    }

    pub fn remove(&self, slot: &KeySlot) {
        if let Ok(Some(stored)) = self.read(slot) {
            log::remove_secret(&stored.key);
        }
        let _ = std::fs::remove_file(self.file(slot));
        let _ = std::fs::remove_file(self.legacy_file(slot));
        if let Some(backup) = self.recovery_file(slot) {
            let _ = std::fs::remove_file(backup);
        }
    }
}

/// A file only its owner may read (where the file system knows owners).
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path)
}

pub struct BackupFile<'a> {
    pub key: &'a str,
    pub username: Option<&'a str>,
    pub server_name: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub fingerprint: Option<&'a str>,
    /// YYYY-MM-DD
    pub date: &'a str,
}

/// The key backup file the player saves (`backupFileText`).
pub fn backup_file_text(file: &BackupFile<'_>) -> String {
    let mut lines = vec![
        "Dauntless Revived account key".to_owned(),
        String::new(),
        format!("Server: {} ({}:{})", file.server_name, file.host, file.port),
    ];
    if let Some(fp) = file.fingerprint.filter(|fp| !fp.is_empty()) {
        lines.push(format!("Certificate: {fp}"));
    }
    lines.push(format!("Username: {}", file.username.unwrap_or("-")));
    lines.push(format!("Saved: {}", file.date));
    lines.push(format!("Key: {}", file.key));
    lines.push(String::new());
    lines.push("Keep this file private: anyone who has this key can play as you on this server.".into());
    lines.push(
        "To use it on another PC: Dauntless Revived Launcher > Join > Use an existing key > Load a backup file.".into(),
    );
    lines.push(String::new());
    lines.join("\r\n")
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;

    /// "enc:" + base64, as the TypeScript tests' encryptor.
    struct Fake;

    impl Encryptor for Fake {
        fn is_available(&self) -> bool {
            true
        }

        fn encrypt(&self, plain: &str) -> Option<Vec<u8>> {
            Some(format!("enc:{}", base64::engine::general_purpose::STANDARD.encode(plain)).into_bytes())
        }

        fn decrypt(&self, data: &[u8]) -> Option<String> {
            let text = std::str::from_utf8(data).ok()?.strip_prefix("enc:")?;
            String::from_utf8(base64::engine::general_purpose::STANDARD.decode(text).ok()?).ok()
        }
    }

    /// The Electron launcher's safeStorage, as another encryptor.
    struct Old;

    impl Encryptor for Old {
        fn is_available(&self) -> bool {
            true
        }

        fn encrypt(&self, plain: &str) -> Option<Vec<u8>> {
            Some(format!("old:{plain}").into_bytes())
        }

        fn decrypt(&self, data: &[u8]) -> Option<String> {
            Some(std::str::from_utf8(data).ok()?.strip_prefix("old:")?.to_owned())
        }
    }

    const KEY: &str = concat!("UUK_", "abababababababababababab", "abababababababababababab");

    fn store(dir: &Path) -> KeyStore {
        KeyStore::new(dir.to_path_buf(), Arc::new(Fake), Some(Arc::new(Old)), None)
    }

    fn public(fp: Option<&str>) -> KeySlot {
        KeySlot {
            host: "203.0.113.10".into(),
            port: 443,
            public: true,
            fp: fp.map(str::to_owned),
        }
    }

    fn fp(pair: &str) -> String {
        pair.repeat(32)
    }

    #[test]
    fn server_ids_are_the_typescript_ones() {
        // sha256("100.64.0.1:61000") and sha256("public:203.0.113.10:443"), first 24 hex
        let expected = |text: &str| {
            Sha256::digest(text.as_bytes())
                .iter()
                .take(12)
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert_eq!(server_id("100.64.0.1", 61000, false), expected("100.64.0.1:61000"));
        assert_eq!(
            server_id("203.0.113.10", 443, true),
            expected("public:203.0.113.10:443")
        );
        assert_eq!(server_id("x", 1, false).len(), 24);
    }

    #[test]
    fn a_public_key_loads_only_for_its_certificate_and_rebind_moves_it() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        let (a, b) = (fp("a1"), fp("b2"));
        keys.save(&public(Some(&a)), KEY).unwrap();
        assert_eq!(keys.load(&public(Some(&a))).unwrap().as_deref(), Some(KEY));
        assert!(keys.has(&public(Some(&a))));
        assert_eq!(
            keys.load(&public(Some(&b))).unwrap(),
            None,
            "another certificate never gets the key"
        );
        assert_eq!(keys.load(&public(None)).unwrap(), None);
        assert_eq!(keys.certificate_of(&public(Some(&b))), Some(Some(a.clone())));
        assert_eq!(keys.rebind(&public(Some(&a)), &b), Ok(true));
        assert_eq!(keys.load(&public(Some(&b))).unwrap().as_deref(), Some(KEY));
        assert_eq!(keys.load(&public(Some(&a))).unwrap(), None);
        assert_eq!(
            keys.rebind(&public(Some(&b)), "not-a-fingerprint"),
            Err(KeyStoreError::Invalid)
        );
        assert_eq!(
            store(tempfile::tempdir().unwrap().path()).rebind(&public(Some(&a)), &b),
            Ok(false),
            "nothing to move"
        );
        assert_eq!(keys.save(&public(None), KEY), Err(KeyStoreError::Invalid));
        assert_eq!(keys.save(&public(Some("ABC")), KEY), Err(KeyStoreError::Invalid));
        keys.remove(&public(Some(&b)));
        assert_eq!(
            keys.certificate_of(&public(Some(&a))),
            None,
            "remove deletes the key whatever certificate it belongs to"
        );
    }

    #[test]
    fn private_keys_and_the_first_file_format() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        let private = KeySlot {
            host: "100.64.0.1".into(),
            port: 61000,
            public: false,
            fp: None,
        };
        keys.save(&private, KEY).unwrap();
        assert_eq!(keys.load(&private).unwrap().as_deref(), Some(KEY));
        // The first format, the key alone, in a new-format file
        std::fs::write(
            dir.path()
                .join(format!("{}.key2", server_id("100.64.0.2", 61000, false))),
            Fake.encrypt(KEY).unwrap(),
        )
        .unwrap();
        assert_eq!(
            keys.load(&KeySlot {
                host: "100.64.0.2".into(),
                ..private.clone()
            })
            .unwrap()
            .as_deref(),
            Some(KEY)
        );
        std::fs::write(
            dir.path()
                .join(format!("{}.key2", server_id("203.0.113.10", 443, true))),
            Fake.encrypt(KEY).unwrap(),
        )
        .unwrap();
        assert_eq!(
            keys.load(&public(Some(&fp("a1")))).unwrap(),
            None,
            "an untied public key goes nowhere"
        );
        assert_eq!(keys.certificate_of(&public(Some(&fp("a1")))), Some(None));
    }

    #[test]
    fn an_electron_key_is_moved_once_and_its_file_kept() {
        let dir = tempfile::tempdir().unwrap();
        let a = fp("a1");
        let slot = public(Some(&a));
        let old = dir.path().join(format!("{}.key", slot.id()));
        std::fs::write(&old, Old.encrypt(&format!("DRK2\n{a}\n{KEY}")).unwrap()).unwrap();
        let keys = store(dir.path());
        assert_eq!(keys.load(&slot).unwrap().as_deref(), Some(KEY));
        let new = dir.path().join(format!("{}.key2", slot.id()));
        assert_eq!(
            Fake.decrypt(&std::fs::read(&new).unwrap()).unwrap(),
            format!("DRK2\n{a}\n{KEY}")
        );
        assert!(old.exists(), "the Electron file stays");
        // Without the old encryptor the new file still loads
        let after = KeyStore::new(dir.path().to_path_buf(), Arc::new(Fake), None, None);
        assert_eq!(after.load(&slot).unwrap().as_deref(), Some(KEY));
        // A key the old encryptor cannot read is not there, not an error: the import flow takes over
        let other = public(Some(&fp("b2")));
        std::fs::write(
            dir.path().join(format!(
                "{}.key",
                KeySlot {
                    host: "198.51.100.1".into(),
                    ..other.clone()
                }
                .id()
            )),
            b"garbage",
        )
        .unwrap();
        assert_eq!(
            keys.load(&KeySlot {
                host: "198.51.100.1".into(),
                ..other
            })
            .unwrap(),
            None
        );
    }

    #[test]
    fn the_documents_copy_survives_lost_key_storage() {
        let dir = tempfile::tempdir().unwrap();
        let (primary, recovery) = (dir.path().join("local"), dir.path().join("Documents"));
        let a = fp("a1");
        let keys = KeyStore::new(primary.clone(), Arc::new(Fake), None, Some(recovery.clone()));
        keys.save(&public(Some(&a)), KEY).unwrap();
        std::fs::remove_dir_all(&primary).unwrap();
        let restored = KeyStore::new(primary, Arc::new(Fake), None, Some(recovery));
        assert_eq!(restored.load(&public(Some(&fp("b2")))).unwrap(), None);
        assert_eq!(restored.load(&public(Some(&a))).unwrap().as_deref(), Some(KEY));
        restored.remove(&public(Some(&a)));
        assert_eq!(restored.load(&public(Some(&a))).unwrap(), None);
    }

    #[test]
    fn the_backup_file() {
        let text = backup_file_text(&BackupFile {
            key: KEY,
            username: None,
            server_name: "Hunts",
            host: "play.example.org",
            port: 443,
            fingerprint: Some("ab"),
            date: "2026-10-21",
        });
        assert!(text.starts_with("Dauntless Revived account key\r\n\r\nServer: Hunts (play.example.org:443)\r\nCertificate: ab\r\nUsername: -\r\nSaved: 2026-10-21\r\n"));
        assert!(text.ends_with("Load a backup file.\r\n"));
        assert_eq!(crate::core::username::extract_account_key(&text), Some(KEY));
    }
}
