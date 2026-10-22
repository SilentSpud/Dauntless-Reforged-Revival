//! Reading the keys the Electron launcher saved with safeStorage (D7, rows L2 and L27), on crates the
//! launcher has anyway. Read only: new keys are written in this launcher's own format (`os.rs`).
//!
//! - Windows: `userData/Local State` holds `os_crypt.encrypted_key`, base64 of "DPAPI" + a DPAPI blob of
//!   the 32-byte AES key. A key file is "v10" + a 12-byte nonce + AES-256-GCM ciphertext and tag.
//! - Linux: a key file is "v11" (the password is the Secret Service item Chromium stores for the app) or
//!   "v10" (the fixed password "peanuts"), with AES-128-CBC, PKCS#7, an IV of 16 spaces, and the key
//!   PBKDF2-HMAC-SHA1(password, "saltysalt", 1 round, 16 bytes).

use aes_gcm::aead::{Aead, KeyInit};
use base64::Engine as _;
use serde_json::Value;

use super::Encryptor;

/// The AES key in `Local State`, unwrapped with `unprotect` (DPAPI's `CryptUnprotectData`).
#[cfg_attr(not(windows), allow(dead_code, reason = "Windows only"))]
pub fn windows_master_key(local_state: &str, unprotect: impl Fn(&[u8]) -> Option<Vec<u8>>) -> Option<[u8; 32]> {
    let state: Value = serde_json::from_str(local_state).ok()?;
    let encoded = state.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let wrapped = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let key = unprotect(wrapped.strip_prefix(b"DPAPI")?)?;
    key.try_into().ok()
}

pub fn decrypt_windows(blob: &[u8], master: &[u8; 32]) -> Option<String> {
    let rest = blob.strip_prefix(b"v10")?;
    if rest.len() < 12 + 16 {
        return None;
    }
    let (nonce, sealed) = rest.split_at(12);
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(master).ok()?;
    let nonce = aes_gcm::Nonce::try_from(nonce).ok()?;
    String::from_utf8(cipher.decrypt(&nonce, sealed).ok()?).ok()
}

fn linux_key(password: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", 1, &mut key);
    key
}

/// `secret`: the Secret Service item's password, needed for "v11" files.
pub fn decrypt_linux(blob: &[u8], secret: Option<&[u8]>) -> Option<String> {
    use cbc::cipher::{BlockModeDecrypt, KeyIvInit};
    let (password, sealed): (&[u8], &[u8]) = match blob.get(..3)? {
        b"v11" => (secret?, &blob[3..]),
        b"v10" => (b"peanuts", &blob[3..]),
        _ => return None,
    };
    let decryptor = cbc::Decryptor::<aes::Aes128>::new(&linux_key(password).into(), &[b' '; 16].into());
    let plain = decryptor
        .decrypt_padded_vec::<cbc::cipher::block_padding::Pkcs7>(sealed)
        .ok()?;
    String::from_utf8(plain).ok()
}

/// The Electron launcher's safeStorage, for reading only.
pub enum ElectronStorage {
    #[cfg_attr(not(windows), allow(dead_code, reason = "Windows only"))]
    Windows { master: [u8; 32] },
    #[cfg_attr(windows, allow(dead_code, reason = "Linux only"))]
    Linux { secret: Option<Vec<u8>> },
}

impl Encryptor for ElectronStorage {
    fn is_available(&self) -> bool {
        true
    }

    fn encrypt(&self, _: &str) -> Option<Vec<u8>> {
        None
    }

    fn decrypt(&self, data: &[u8]) -> Option<String> {
        match self {
            Self::Windows { master } => decrypt_windows(data, master),
            Self::Linux { secret } => decrypt_linux(data, secret.as_deref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_key_file() {
        let master = [7u8; 32];
        let wrapped = [b"DPAPI".as_slice(), b"protected:", &master].concat();
        let local_state = serde_json::json!({ "os_crypt": { "encrypted_key": base64::engine::general_purpose::STANDARD.encode(&wrapped) } }).to_string();
        let unprotect = |data: &[u8]| data.strip_prefix(b"protected:").map(<[u8]>::to_vec);
        assert_eq!(windows_master_key(&local_state, unprotect), Some(master));
        assert_eq!(windows_master_key("{}", unprotect), None);

        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&master).unwrap();
        let nonce = [3u8; 12];
        let sealed = cipher
            .encrypt(
                &aes_gcm::Nonce::try_from(&nonce[..]).unwrap(),
                b"DRK2\n-\nUUK_x".as_slice(),
            )
            .unwrap();
        let blob = [b"v10".as_slice(), &nonce, &sealed].concat();
        assert_eq!(decrypt_windows(&blob, &master).as_deref(), Some("DRK2\n-\nUUK_x"));
        assert_eq!(decrypt_windows(&blob, &[8u8; 32]), None, "another user's key");
        assert_eq!(decrypt_windows(&blob[..blob.len() - 1], &master), None, "a damaged tag");
    }

    fn linux_blob(version: &[u8], password: &[u8], plain: &[u8]) -> Vec<u8> {
        use cbc::cipher::{BlockModeEncrypt, KeyIvInit};
        let encryptor = cbc::Encryptor::<aes::Aes128>::new(&linux_key(password).into(), &[b' '; 16].into());
        [
            version,
            &encryptor.encrypt_padded_vec::<cbc::cipher::block_padding::Pkcs7>(plain),
        ]
        .concat()
    }

    #[test]
    fn linux_key_files() {
        let v11 = linux_blob(b"v11", b"keyring-secret", b"DRK2\n-\nUUK_y");
        assert_eq!(
            decrypt_linux(&v11, Some(b"keyring-secret")).as_deref(),
            Some("DRK2\n-\nUUK_y")
        );
        assert_eq!(decrypt_linux(&v11, None), None);
        assert_eq!(decrypt_linux(&v11, Some(b"other")), None);
        let v10 = linux_blob(b"v10", b"peanuts", b"UUK_z");
        assert_eq!(decrypt_linux(&v10, None).as_deref(), Some("UUK_z"));
        assert_eq!(decrypt_linux(b"v12abc", None), None);
        // Known answer: Chromium's key for "peanuts"
        assert_eq!(
            linux_key(b"peanuts")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "fd621fe5a2b402539dfa147ca9272778"
        );
    }
}
