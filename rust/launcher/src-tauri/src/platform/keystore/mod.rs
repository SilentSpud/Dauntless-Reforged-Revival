//! Account keys, one per server (row L27): `src/main/keystore.ts`. The key never goes to the page; it is
//! read here when a request or the game launch needs it.
//!
//! A public server's key is tied to the certificate fingerprint it was saved with. An invite for the same
//! host:port with another fingerprint (a rebuilt server, or someone in the middle) cannot load it: the
//! key moves to a new fingerprint only through `rebind`, after the player confirmed.
//!
//! Files: `keys/<id>.key2`, encrypted by this launcher (`os.rs`: DPAPI on Windows, AES-256-GCM with a key
//! held by the Secret Service or KWallet on Linux). The Electron launcher's `keys/<id>.key` (safeStorage)
//! is read once through `legacy.rs`, written in the new format, read back, and left in place for a
//! rollback to 1.5.x until 3.0.0 (D7, RD7). Both hold `RRK2` text or a bare key (`format.rs`, RD22). A
//! plain-text copy sits in the player's Documents (the recovery folder, `Paths::recovery_dir`), as the
//! Electron launcher keeps one. Keys of saved 1.4.4 servers stay on disk like any other (RD22).
//!
//! A key goes on the log's secret list (`log::add_secret`) as soon as it is read or saved, and leaves this
//! module only to the controller; no bridge type carries one.

pub mod format;
pub mod legacy;
pub mod os;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::account::is_plausible_account_key;
use protocol::invite::is_valid_fingerprint;
use sha2::{Digest, Sha256};

use self::format::{Stored, parse_stored};
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

    /// The encrypted key file of a slot.
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
            log::add_secret(&stored.key);
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
        let text = stored.text();
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
            (Some(fp), true) if is_valid_fingerprint(fp) => Some(fp.clone()),
            (_, true) => return Err(KeyStoreError::Invalid),
            (_, false) => None,
        };
        if !self.crypto.is_available() {
            return Err(KeyStoreError::Unavailable);
        }
        log::add_secret(key);
        let text = Stored {
            key: key.to_owned(),
            fp,
        }
        .text();
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
        if !slot.public || !is_valid_fingerprint(fp) {
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

    /// Forgets this server's key: this launcher's file, the Electron launcher's and the Documents copy.
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

/// The key backup file the player saves (`backupFileText`). Its English lines are the file's format,
/// which the import reads back, so they are not translated.
pub fn backup_file_text(file: &BackupFile<'_>) -> String {
    let mut lines = vec![
        "Dauntless Reforged Revival account key".to_owned(),
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
        "To use it on another PC: Dauntless Reforged Revival Launcher > Register > I already have a key > Load a backup file."
            .into(),
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

    const KEY: &str = concat!("RRK_", "abababababababababababab", "abababababababababababab");

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

    fn private(host: &str) -> KeySlot {
        KeySlot {
            host: host.into(),
            port: 61000,
            public: false,
            fp: None,
        }
    }

    fn fp(pair: &str) -> String {
        pair.repeat(32)
    }

    #[test]
    fn server_ids_are_the_typescript_ones() {
        // createHash("sha256").update(id).digest("hex").slice(0, 24), worked out by Node
        assert_eq!(server_id("100.64.0.1", 61000, false), "2f1974fb8b73463e94a88aed");
        assert_eq!(server_id("203.0.113.10", 443, true), "4513d283ab25060967c23596");
    }

    // keystore.test.ts: "a public key loads only for the certificate it was saved with"
    #[test]
    fn a_public_key_loads_only_for_the_certificate_it_was_saved_with() {
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
        assert!(!keys.has(&public(Some(&b))));
        assert_eq!(keys.load(&public(None)).unwrap(), None);
        assert_eq!(keys.certificate_of(&public(Some(&b))), Some(Some(a)));
    }

    // "rebind moves the key to a new certificate"
    #[test]
    fn rebind_moves_the_key_to_a_new_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        let (a, b) = (fp("a1"), fp("b2"));
        keys.save(&public(Some(&a)), KEY).unwrap();
        assert_eq!(keys.rebind(&public(Some(&a)), &b), Ok(true));
        assert_eq!(keys.load(&public(Some(&b))).unwrap().as_deref(), Some(KEY));
        assert_eq!(keys.load(&public(Some(&a))).unwrap(), None);
        assert_eq!(
            store(tempfile::tempdir().unwrap().path()).rebind(&public(Some(&a)), &b),
            Ok(false),
            "nothing to move"
        );
        assert_eq!(
            keys.rebind(&public(Some(&b)), "not-a-fingerprint"),
            Err(KeyStoreError::Invalid)
        );
        assert_eq!(keys.rebind(&private("100.64.0.1"), &b), Err(KeyStoreError::Invalid));
    }

    // "a public key cannot be saved without a fingerprint"
    #[test]
    fn a_public_key_cannot_be_saved_without_a_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        assert_eq!(keys.save(&public(None), KEY), Err(KeyStoreError::Invalid));
        assert_eq!(keys.save(&public(Some("ABC")), KEY), Err(KeyStoreError::Invalid));
        assert_eq!(
            keys.save(&public(Some(&fp("a1"))), "not a key"),
            Err(KeyStoreError::Invalid)
        );
    }

    // "private keys are not tied to a certificate; old key files still load for private servers only"
    #[test]
    fn private_keys_and_the_first_file_format() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        keys.save(&private("100.64.0.1"), KEY).unwrap();
        assert_eq!(keys.load(&private("100.64.0.1")).unwrap().as_deref(), Some(KEY));
        // The first format, the key alone, in a new-format file
        std::fs::write(keys.file(&private("100.64.0.2")), Fake.encrypt(KEY).unwrap()).unwrap();
        assert_eq!(keys.load(&private("100.64.0.2")).unwrap().as_deref(), Some(KEY));
        std::fs::write(keys.file(&public(None)), Fake.encrypt(KEY).unwrap()).unwrap();
        assert_eq!(
            keys.load(&public(Some(&fp("a1")))).unwrap(),
            None,
            "an untied public key goes nowhere"
        );
        assert_eq!(keys.certificate_of(&public(Some(&fp("a1")))), Some(None));
    }

    // "remove deletes the key whatever certificate it belongs to"
    #[test]
    fn remove_deletes_the_key_whatever_certificate_it_belongs_to() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        keys.save(&public(Some(&fp("a1"))), KEY).unwrap();
        std::fs::write(
            keys.legacy_file(&public(None)),
            Old.encrypt(&format!("RRK2\n{}\n{KEY}", fp("a1"))).unwrap(),
        )
        .unwrap();
        keys.remove(&public(Some(&fp("b2"))));
        assert_eq!(keys.certificate_of(&public(Some(&fp("a1")))), None);
        assert!(!keys.legacy_file(&public(None)).exists(), "the Electron file too");
    }

    // "Documents recovery survives lost local key storage and keeps certificate binding"
    #[test]
    fn the_documents_copy_survives_lost_key_storage_and_keeps_its_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let (primary, recovery) = (dir.path().join("local"), dir.path().join("Documents"));
        let a = fp("a1");
        let keys = KeyStore::new(primary.clone(), Arc::new(Fake), None, Some(recovery.clone()));
        keys.save(&public(Some(&a)), KEY).unwrap();
        assert_eq!(
            std::fs::read_to_string(recovery.join(format!("{}.txt", public(None).id()))).unwrap(),
            format!("RRK2\n{a}\n{KEY}"),
            "the Electron launcher's Documents file, byte for byte"
        );
        std::fs::remove_dir_all(&primary).unwrap();
        let restored = KeyStore::new(primary, Arc::new(Fake), None, Some(recovery));
        assert_eq!(restored.load(&public(Some(&fp("b2")))).unwrap(), None);
        assert_eq!(restored.load(&public(Some(&a))).unwrap().as_deref(), Some(KEY));
        restored.remove(&public(Some(&a)));
        assert_eq!(restored.load(&public(Some(&a))).unwrap(), None);
    }

    #[test]
    fn an_electron_key_is_moved_once_and_its_file_kept() {
        let dir = tempfile::tempdir().unwrap();
        let a = fp("a1");
        let slot = public(Some(&a));
        let keys = store(dir.path());
        let old = keys.legacy_file(&slot);
        std::fs::write(&old, Old.encrypt(&format!("RRK2\n{a}\n{KEY}")).unwrap()).unwrap();
        assert_eq!(keys.load(&slot).unwrap().as_deref(), Some(KEY));
        assert_eq!(
            Fake.decrypt(&std::fs::read(keys.file(&slot)).unwrap()).unwrap(),
            format!("RRK2\n{a}\n{KEY}")
        );
        assert!(old.exists(), "the Electron file stays for a rollback");
        // Without the old encryptor the new file still loads
        let after = KeyStore::new(dir.path().to_path_buf(), Arc::new(Fake), None, None);
        assert_eq!(after.load(&slot).unwrap().as_deref(), Some(KEY));
        // A bare Electron key (the first format) is moved for a private server
        let bare = private("100.64.0.9");
        std::fs::write(keys.legacy_file(&bare), Old.encrypt(KEY).unwrap()).unwrap();
        assert_eq!(keys.load(&bare).unwrap().as_deref(), Some(KEY));
        assert_eq!(
            Fake.decrypt(&std::fs::read(keys.file(&bare)).unwrap()).unwrap(),
            format!("RRK2\n-\n{KEY}")
        );
    }

    #[test]
    fn only_rrk2_and_bare_electron_keys_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        let a = fp("a1");
        let slot = public(Some(&a));
        // DR's tag never was on a Reforged PC (RD22)
        std::fs::write(
            keys.legacy_file(&slot),
            Old.encrypt(&format!("DRK2\n{a}\n{KEY}")).unwrap(),
        )
        .unwrap();
        assert_eq!(keys.load(&slot).unwrap(), None);
        assert!(!keys.file(&slot).exists(), "nothing written");
        // A file the old encryptor cannot read is no key, not an error: the import flow takes over
        std::fs::write(keys.legacy_file(&private("100.64.0.3")), b"garbage").unwrap();
        assert_eq!(keys.load(&private("100.64.0.3")).unwrap(), None);
    }

    #[test]
    fn a_read_key_is_hidden_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let keys = store(dir.path());
        let key = concat!("RRK_", "0f0f0f0f0f0f0f0f0f0f0f0f", "0f0f0f0f0f0f0f0f0f0f0f0f");
        std::fs::write(keys.legacy_file(&private("100.64.0.4")), Old.encrypt(key).unwrap()).unwrap();
        assert_eq!(keys.load(&private("100.64.0.4")).unwrap().as_deref(), Some(key));
        let line = log::redact(&format!("argument {} and {}", key, &key[4..]));
        assert!(!line.contains(&key[4..]), "{line}");
    }

    #[test]
    fn an_unavailable_store_saves_nothing() {
        struct Off;
        impl Encryptor for Off {
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
        let dir = tempfile::tempdir().unwrap();
        let keys = KeyStore::new(dir.path().to_path_buf(), Arc::new(Off), None, None);
        assert_eq!(keys.save(&private("100.64.0.1"), KEY), Err(KeyStoreError::Unavailable));
        std::fs::write(keys.file(&private("100.64.0.1")), b"x").unwrap();
        assert_eq!(keys.load(&private("100.64.0.1")), Err(KeyStoreError::Unavailable));
    }

    /// A profile the Electron launcher's own code wrote (`rust/launcher/test/electron-keystore`, with real
    /// safeStorage, as the same user on the same PC): every key reads back through `legacy.rs`, is moved to
    /// this launcher's files once, and the 1.4.4 server in its settings loads. Never a real profile.
    #[tokio::test]
    #[ignore = "needs REFORGED_ELECTRON_PROFILE, a profile made by test/electron-keystore (see its README)"]
    async fn an_electron_profile_reads_back() {
        use serde_json::Value;

        use crate::core::i18n::Language;
        use crate::platform::settings::{Platform, SettingsStore, is_revived};

        let profile = PathBuf::from(std::env::var_os("REFORGED_ELECTRON_PROFILE").expect("REFORGED_ELECTRON_PROFILE"));
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(profile.join("expected.json")).unwrap()).unwrap();
        let user_data = profile.join("User Data");
        let keys_dir = user_data.join("keys");
        let recovery = profile
            .join("Documents")
            .join(crate::platform::paths::RECOVERY_FOLDER)
            .join("Account Keys");
        let (crypto, legacy) = os::encryptors(&user_data).await;
        assert!(legacy.is_some(), "the Electron launcher's Local State key");
        let slots: Vec<(KeySlot, Option<String>, Option<String>, bool)> = expected["slots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|slot| {
                (
                    KeySlot {
                        host: slot["host"].as_str().unwrap().into(),
                        port: slot["port"].as_u64().unwrap() as u16,
                        public: slot["mode"] == "public",
                        fp: slot["fp"].as_str().map(str::to_owned),
                    },
                    slot["key"].as_str().map(str::to_owned),
                    slot["certificate"].as_str().map(str::to_owned),
                    slot["documents"] == true,
                )
            })
            .collect();
        assert!(slots.len() >= 8);

        let keys = KeyStore::new(keys_dir.clone(), crypto.clone(), legacy, None);
        for (slot, key, certificate, _) in &slots {
            assert_eq!(
                &keys.load(slot).unwrap(),
                key,
                "{}:{} {:?}",
                slot.host,
                slot.port,
                slot.fp
            );
            if slot.public {
                assert_eq!(keys.certificate_of(slot), Some(certificate.clone()), "{}", slot.host);
            }
            assert!(keys.legacy_file(slot).exists(), "the Electron file stays");
        }
        // Moved: the same keys without the Electron reader
        let moved = KeyStore::new(keys_dir, crypto.clone(), None, None);
        for (slot, key, _, _) in &slots {
            if key.is_some() {
                assert!(moved.file(slot).exists());
            }
            assert_eq!(&moved.load(slot).unwrap(), key, "{} after the move", slot.host);
        }
        // The Documents copies alone, as after reinstalling the system
        let empty = tempfile::tempdir().unwrap();
        let restored = KeyStore::new(empty.path().to_path_buf(), crypto, None, Some(recovery));
        for (slot, key, _, documents) in &slots {
            if *documents {
                assert_eq!(&restored.load(slot).unwrap(), key, "{} from Documents", slot.host);
            }
        }

        let platform = if expected["platform"] == "win32" {
            Platform::Windows
        } else {
            Platform::Linux
        };
        let settings = SettingsStore::load(&user_data, Language::En, platform);
        let saved = settings.get()["savedServers"].as_array().unwrap();
        assert_eq!(
            settings.get()["server"],
            Value::Null,
            "a 1.4.4 server is not the current one"
        );
        assert_eq!(saved.len(), 2);
        assert!(is_revived(saved[0].as_object().unwrap()));
        assert_eq!(saved[1]["game"], "reforged");
        assert_eq!(settings.get()["usernames"].as_object().unwrap().len(), 1);
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
        // backupFileText's output for the same values, from Node
        assert_eq!(
            text,
            "Dauntless Reforged Revival account key\r\n\r\nServer: Hunts (play.example.org:443)\r\nCertificate: ab\r\nUsername: -\r\nSaved: 2026-10-21\r\nKey: RRK_abababababababababababababababababababababababab\r\n\r\nKeep this file private: anyone who has this key can play as you on this server.\r\nTo use it on another PC: Dauntless Reforged Revival Launcher > Register > I already have a key > Load a backup file.\r\n"
        );
        assert_eq!(protocol::account::extract_account_key(&text), Some(KEY));
    }
}
