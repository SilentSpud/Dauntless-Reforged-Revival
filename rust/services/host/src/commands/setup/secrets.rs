//! New secrets (the Node kit's `newSecrets()` in `hostconfig.ts`): six 32-byte hex keys and a
//! 4096-bit RSA pair for the backend's tokens, PKCS#8 and SPKI PEM, each base64 as one line.
//! RSA with aws-lc-rs (D10). [`crate::hostconfig::load_or_create_secrets`] takes this as its `make`;
//! `hostconfig::new_secrets` (RH1's stub) can call it once the integration pass wires it.

use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::KeyPair as _;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::error::HostError;
use crate::hostconfig::Secrets;

fn crypto_failed(what: &str) -> HostError {
    HostError::message(format!("could not make {what} (aws-lc)"))
}

fn hex(rng: &SystemRandom) -> Result<String, HostError> {
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes).map_err(|_| crypto_failed("a random key"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// PEM as Node's `export({ format: "pem" })` writes it: 64 base64 characters a line, a final newline.
pub fn pem(label: &str, der: &[u8]) -> String {
    let body = STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out + &format!("-----END {label}-----\n")
}

pub fn new_secrets() -> Result<Secrets, HostError> {
    let rng = SystemRandom::new();
    let pair = KeyPair::generate(KeySize::Rsa4096).map_err(|_| crypto_failed("the RSA key pair"))?;
    let private = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&pair)
        .map_err(|_| crypto_failed("the private key's PKCS#8"))?;
    let public = AsDer::<aws_lc_rs::encoding::PublicKeyX509Der>::as_der(pair.public_key())
        .map_err(|_| crypto_failed("the public key's SPKI"))?;
    Ok(Secrets {
        gateway_secret: hex(&rng)?,
        allowlist_secret: hex(&rng)?,
        admin_key: hex(&rng)?,
        gameserver_key: hex(&rng)?,
        api_key_hash_secret: hex(&rng)?,
        metrics_token: hex(&rng)?,
        jwt_private_key_b64: STANDARD.encode(pem("PRIVATE KEY", private.as_ref())),
        jwt_public_key_b64: STANDARD.encode(pem("PUBLIC KEY", public.as_ref())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // config.test.ts: "include a 4096-bit RSA pair for the backend's tokens, base64 PEM".
    #[test]
    fn include_a_4096_bit_rsa_pair_for_the_backends_tokens_base64_pem() {
        let s = new_secrets().unwrap();
        let private = String::from_utf8(STANDARD.decode(&s.jwt_private_key_b64).unwrap()).unwrap();
        let public = String::from_utf8(STANDARD.decode(&s.jwt_public_key_b64).unwrap()).unwrap();
        assert!(private.starts_with("-----BEGIN PRIVATE KEY-----\n"), "{private}");
        assert!(public.starts_with("-----BEGIN PUBLIC KEY-----\n"), "{public}");
        assert!(public.ends_with("-----END PUBLIC KEY-----\n"));
        assert!(public.lines().all(|l| l.len() <= 64));
        assert!(
            s.admin_key.len() == 64
                && s.admin_key
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(s.gateway_secret, s.allowlist_secret);
        // 4096 bits: the SPKI of a 4096-bit RSA key is 550 bytes.
        let der = STANDARD
            .decode(public.lines().filter(|l| !l.starts_with("-----")).collect::<String>())
            .unwrap();
        assert_eq!(der.len(), 550);
    }
}
