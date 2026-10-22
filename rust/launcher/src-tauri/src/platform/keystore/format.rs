//! What the key store reads from disk before and after decryption, with no crypto: the key file text
//! (`RRK2`, or a bare key) and the Electron launcher's framing (`Local State`, `v10`/`v11` blobs). Built
//! on `protocol` and `serde_json` only, so the fuzz target (`rust/fuzz`, `launcher_files`) compiles this
//! file as it is. The decryption itself is in `legacy.rs` and `os.rs`.

use protocol::account::is_plausible_account_key;
use protocol::invite::is_valid_fingerprint;
use serde_json::Value;

/// What a key file holds after decryption: `RRK2\n<fingerprint or ->\n<key>`, as Reforged's Electron
/// launcher writes it (and this one, in its own files). The first format was the key alone; such a key is
/// not tied to any certificate. DR's `DRK2` was never written on a Reforged PC and is not read (RD22).
pub const FORMAT: &str = "RRK2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub key: String,
    /// The certificate the key belongs to; None for a private server's key or a bare key
    pub fp: Option<String>,
}

impl Stored {
    /// The text a key file holds.
    pub fn text(&self) -> String {
        format!("{FORMAT}\n{}\n{}", self.fp.as_deref().unwrap_or("-"), self.key)
    }
}

/// A decrypted key file (or a Documents copy). None: not a key this launcher can use.
pub fn parse_stored(text: &str) -> Option<Stored> {
    let parts: Vec<&str> = text.split('\n').collect();
    if parts.len() == 3 && parts[0] == FORMAT {
        let fp = (parts[1] != "-").then(|| parts[1].to_owned());
        if fp.as_deref().is_some_and(|fp| !is_valid_fingerprint(fp)) {
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

/// `Local State`'s `os_crypt.encrypted_key`: base64 of "DPAPI" + a DPAPI blob of the AES key.
pub fn local_state_key(local_state: &str) -> Option<String> {
    let state: Value = serde_json::from_str(local_state).ok()?;
    Some(state.get("os_crypt")?.get("encrypted_key")?.as_str()?.to_owned())
}

/// After base64: the DPAPI blob behind the "DPAPI" tag.
pub fn dpapi_blob(wrapped: &[u8]) -> Option<&[u8]> {
    wrapped.strip_prefix(b"DPAPI").filter(|blob| !blob.is_empty())
}

/// A Windows key file: "v10" + a 12-byte nonce + AES-256-GCM ciphertext and its 16-byte tag.
pub fn windows_blob(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = blob.strip_prefix(b"v10")?;
    (rest.len() >= 12 + 16).then(|| rest.split_at(12))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPassword {
    /// "v10": the fixed password "peanuts"
    Fixed,
    /// "v11": the password the Secret Service keeps for the app
    Keyring,
}

/// A Linux key file: "v10" or "v11" + AES-128-CBC blocks.
pub fn linux_blob(blob: &[u8]) -> Option<(LinuxPassword, &[u8])> {
    let password = match blob.get(..3)? {
        b"v10" => LinuxPassword::Fixed,
        b"v11" => LinuxPassword::Keyring,
        _ => return None,
    };
    let sealed = &blob[3..];
    (!sealed.is_empty() && sealed.len().is_multiple_of(16)).then_some((password, sealed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = concat!("RRK_", "abababababababababababab", "abababababababababababab");

    fn fp() -> String {
        "a1".repeat(32)
    }

    #[test]
    fn rrk2_and_bare_keys_only() {
        assert_eq!(
            parse_stored(&format!("RRK2\n{}\n{KEY}", fp())),
            Some(Stored {
                key: KEY.into(),
                fp: Some(fp())
            })
        );
        assert_eq!(
            parse_stored(&format!("RRK2\n-\n{KEY}")),
            Some(Stored {
                key: KEY.into(),
                fp: None
            })
        );
        assert_eq!(
            parse_stored(KEY),
            Some(Stored {
                key: KEY.into(),
                fp: None
            }),
            "the first format: the key alone"
        );
        for bad in [
            format!("DRK2\n{}\n{KEY}", fp()),
            format!("RRK2\nABC\n{KEY}"),
            format!("RRK2\n{}\n{KEY}\n", fp()),
            "RRK2\n-\nkey with spaces".into(),
            format!("RRK2\n-\n{KEY}\r"),
            format!("{KEY}\n"),
            String::new(),
            "short".into(),
        ] {
            assert_eq!(parse_stored(&bad), None, "{bad:?}");
        }
        let stored = parse_stored(&format!("RRK2\n{}\n{KEY}", fp())).unwrap();
        assert_eq!(parse_stored(&stored.text()), Some(stored));
    }

    #[test]
    fn electron_framing() {
        assert_eq!(
            local_state_key(r#"{"os_crypt":{"encrypted_key":"RFBBUEk="}}"#).as_deref(),
            Some("RFBBUEk=")
        );
        for bad in ["{}", r#"{"os_crypt":{"encrypted_key":1}}"#, "not json", ""] {
            assert_eq!(local_state_key(bad), None);
        }
        assert_eq!(dpapi_blob(b"DPAPIxyz"), Some(b"xyz".as_slice()));
        assert_eq!(dpapi_blob(b"DPAPI"), None);
        assert_eq!(dpapi_blob(b"dpapixyz"), None);
        let blob = [b"v10".as_slice(), &[0; 12], &[1; 16]].concat();
        assert_eq!(windows_blob(&blob), Some((&[0u8; 12][..], &[1u8; 16][..])));
        assert_eq!(windows_blob(&blob[..blob.len() - 1]), None);
        assert_eq!(windows_blob(b"v11"), None);
        assert_eq!(
            linux_blob(&[b"v11".as_slice(), &[0; 32]].concat()),
            Some((LinuxPassword::Keyring, &[0u8; 32][..]))
        );
        assert_eq!(linux_blob(&[b"v10".as_slice(), &[0; 17]].concat()), None);
        assert_eq!(linux_blob(b"v10"), None);
        assert_eq!(linux_blob(b"v1"), None);
    }
}
