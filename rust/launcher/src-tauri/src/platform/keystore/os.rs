//! This launcher's key file encryption (row L27): DPAPI for the user on Windows; on Linux AES-256-GCM
//! with a random key the desktop's Secret Service keeps (gnome-keyring, or KWallet on Plasma 6). Plus
//! what the Electron launcher's files need to be read: its `Local State` key (Windows) or its Secret
//! Service password (Linux).

use std::path::Path;
use std::sync::Arc;

use super::Encryptor;
use super::legacy::ElectronStorage;

/// "dr1" + a 12-byte nonce + ciphertext and tag.
#[cfg_attr(windows, allow(dead_code, reason = "Windows uses DPAPI for the files themselves"))]
struct SealedKey {
    key: [u8; 32],
}

#[cfg_attr(windows, allow(dead_code, reason = "Windows uses DPAPI for the files themselves"))]
impl SealedKey {
    const PREFIX: &'static [u8] = b"dr1";
}

impl Encryptor for SealedKey {
    fn is_available(&self) -> bool {
        true
    }

    fn encrypt(&self, plain: &str) -> Option<Vec<u8>> {
        use aes_gcm::aead::{Aead, KeyInit};
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&self.key).ok()?;
        let nonce: [u8; 12] = rand::random();
        let sealed = cipher
            .encrypt(&aes_gcm::Nonce::try_from(&nonce[..]).ok()?, plain.as_bytes())
            .ok()?;
        Some([Self::PREFIX, &nonce, &sealed].concat())
    }

    fn decrypt(&self, data: &[u8]) -> Option<String> {
        use aes_gcm::aead::{Aead, KeyInit};
        let rest = data.strip_prefix(Self::PREFIX)?;
        if rest.len() < 12 + 16 {
            return None;
        }
        let (nonce, sealed) = rest.split_at(12);
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&self.key).ok()?;
        String::from_utf8(cipher.decrypt(&aes_gcm::Nonce::try_from(nonce).ok()?, sealed).ok()?).ok()
    }
}

/// No key storage on this desktop: keys cannot be saved ("unavailable").
#[cfg_attr(windows, expect(dead_code, reason = "Linux only"))]
struct Unavailable;

impl Encryptor for Unavailable {
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

/// The encryptor for new key files, and the reader for the Electron launcher's (if any can be read).
pub async fn encryptors(user_data: &Path) -> (Arc<dyn Encryptor>, Option<Arc<dyn Encryptor>>) {
    #[cfg(windows)]
    {
        let legacy = std::fs::read_to_string(user_data.join("Local State"))
            .ok()
            .and_then(|state| super::legacy::windows_master_key(&state, dpapi::unprotect))
            .map(|master| Arc::new(ElectronStorage::Windows { master }) as Arc<dyn Encryptor>);
        (Arc::new(dpapi::Dpapi), legacy)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = user_data;
        let current: Arc<dyn Encryptor> = match secret_service::launcher_key().await {
            Some(key) => Arc::new(SealedKey { key }),
            None => Arc::new(Unavailable),
        };
        let legacy = Arc::new(ElectronStorage::Linux {
            secret: secret_service::electron_password().await,
        }) as Arc<dyn Encryptor>;
        (current, Some(legacy))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = user_data;
        (Arc::new(Unavailable), None)
    }
}

#[cfg(windows)]
mod dpapi {
    //! `CryptProtectData` / `CryptUnprotectData` for the current user, as Electron's safeStorage does on
    //! Windows for its own key.
    #![allow(
        unsafe_code,
        reason = "DPAPI is a C API; each call is wrapped and its buffer freed here"
    )]

    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use super::super::Encryptor;

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr().cast_mut(),
        }
    }

    /// Copies a blob DPAPI allocated, then frees it.
    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: DPAPI filled `out` with `cbData` bytes at `pbData`, allocated with LocalAlloc
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: the buffer is DPAPI's and freed once, here
        unsafe {
            let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        }
        bytes
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` points at `data`, which outlives the call; `out` is written by the call
        unsafe { CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
        Some(take(out))
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let input = blob(data);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as above
        unsafe { CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
        Some(take(out))
    }

    pub struct Dpapi;

    impl Encryptor for Dpapi {
        fn is_available(&self) -> bool {
            true
        }

        fn encrypt(&self, plain: &str) -> Option<Vec<u8>> {
            protect(plain.as_bytes())
        }

        fn decrypt(&self, data: &[u8]) -> Option<String> {
            String::from_utf8(unprotect(data)?).ok()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_round_trip_for_this_user() {
            let sealed = Dpapi.encrypt("DRK2\n-\nUUK_x").unwrap();
            assert_ne!(sealed, b"DRK2\n-\nUUK_x");
            assert_eq!(Dpapi.decrypt(&sealed).as_deref(), Some("DRK2\n-\nUUK_x"));
            assert_eq!(Dpapi.decrypt(b"not a blob"), None);
        }
    }
}

#[cfg(target_os = "linux")]
mod secret_service {
    //! The desktop's Secret Service over D-Bus (gnome-keyring; KWallet answers it too on Plasma 6), through
    //! libdbus (`dbus-secret-service`; zbus 5's parts are unmaintained). Its calls block, so they run on
    //! the blocking pool.

    use std::collections::HashMap;

    use dbus_secret_service::{EncryptionType, SecretService};

    const APPLICATION: &str = "io.github.mixutin.dauntlessrevived";

    fn key_now() -> Option<[u8; 32]> {
        let service = SecretService::connect(EncryptionType::Dh).ok()?;
        let attributes = HashMap::from([("application", APPLICATION), ("purpose", "account-key-files")]);
        let found = service.search_items(attributes.clone()).ok()?;
        for item in found.unlocked.iter().chain(found.locked.iter()) {
            let _ = item.unlock();
            if let Ok(secret) = item.get_secret()
                && let Ok(key) = <[u8; 32]>::try_from(secret.as_slice())
            {
                return Some(key);
            }
        }
        let collection = service.get_default_collection().ok()?;
        let _ = collection.unlock();
        let key: [u8; 32] = rand::random();
        collection
            .create_item(
                "Dauntless Revived Launcher account keys",
                attributes,
                &key,
                true,
                "application/octet-stream",
            )
            .ok()?;
        Some(key)
    }

    fn password_now() -> Option<Vec<u8>> {
        let service = SecretService::connect(EncryptionType::Dh).ok()?;
        let attributes = HashMap::from([
            ("xdg:schema", "chrome_libsecret_os_crypt_password_v2"),
            ("application", "Dauntless Revived Launcher"),
        ]);
        let found = service.search_items(attributes).ok()?;
        for item in found.unlocked.iter().chain(found.locked.iter()) {
            let _ = item.unlock();
            if let Ok(secret) = item.get_secret() {
                return Some(secret);
            }
        }
        None
    }

    /// This launcher's key for its key files, made the first time.
    pub async fn launcher_key() -> Option<[u8; 32]> {
        tokio::task::spawn_blocking(key_now).await.ok().flatten()
    }

    /// The password Electron's safeStorage keeps for the old launcher. Found by its attributes, never by
    /// its label ("Chromium Safe Storage" is the same for every Electron app).
    pub async fn electron_password() -> Option<Vec<u8>> {
        tokio::task::spawn_blocking(password_now).await.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_files_open_only_with_their_key() {
        let sealed = SealedKey { key: [1; 32] };
        let data = sealed.encrypt("DRK2\n-\nUUK_x").unwrap();
        assert!(data.starts_with(b"dr1"));
        assert_eq!(sealed.decrypt(&data).as_deref(), Some("DRK2\n-\nUUK_x"));
        assert_eq!(SealedKey { key: [2; 32] }.decrypt(&data), None);
        assert_ne!(
            sealed.encrypt("same").unwrap(),
            sealed.encrypt("same").unwrap(),
            "a fresh nonce each time"
        );
    }
}
