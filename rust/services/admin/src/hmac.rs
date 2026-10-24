//! HMAC-SHA256 (RFC 2104) for the backend's manager-operator signature, and a constant-time compare
//! for the API key. Written out on `sha2` rather than adding a crate for twenty lines.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        for (slot, byte) in block.iter_mut().zip(Sha256::digest(key).iter()) {
            *slot = *byte;
        }
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().to_vec()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Compares the SHA-256 of both, so neither the length nor the first differing byte shows in the time
/// taken (as the backend's `IsLocalAdminRequest` does).
pub fn same_secret(given: &str, expected: &str) -> bool {
    let a = Sha256::digest(given.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rfc_4231() {
        // Test cases 2 and 6 (a key longer than the block)
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn compares_whole_secrets() {
        assert!(same_secret("k".repeat(40).as_str(), "k".repeat(40).as_str()));
        assert!(!same_secret("k".repeat(39).as_str(), "k".repeat(40).as_str()));
        assert!(!same_secret("", "k"));
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
