//! Invite strings, as in `legacy/UndauntedLauncher/src/shared/invite.ts`. Two versions exist:
//!
//! - v=1, private mode (the host's PC, reached over Tailscale, plain HTTP):
//!   `dauntless-revived://join?v=1&host=…&port=61000&code=…&name=…[&share=…]`
//! - v=2, public mode (a server on a public IP, reached through its TLS gateway):
//!   `dauntless-revived://join?v=2&mode=public&host=…&port=…&fp=…&code=…&name=…`
//!
//! Parsing stays in the launcher (L11), because the host, port and fingerprint decide where the
//! account key is sent. This module holds the shape both sides share and the builder that the
//! servers and the Discord bot use to hand invites out.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

pub const INVITE_SCHEME: &str = "dauntless-revived";
pub const DEFAULT_PUBLIC_PORT: u16 = 443;

/// What JavaScript's `encodeURIComponent` leaves as it is: letters, digits and `-_.!~*'()`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub mode: ServerMode,
    pub host: String,
    pub port: u16,
    pub code: String,
    pub name: String,
    /// Private mode only: the Tailscale share link.
    pub share: Option<String>,
    /// Public mode only: SHA-256 of the server's TLS certificate (DER), 64 lower-case hex characters.
    pub fp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// A public invite without a valid certificate fingerprint.
    Fingerprint,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fingerprint => f.write_str("public invites need a certificate fingerprint"),
        }
    }
}

impl std::error::Error for FormatError {}

pub fn is_valid_fingerprint(fp: &str) -> bool {
    fp.len() == 64 && fp.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn encode(text: &str) -> impl std::fmt::Display + '_ {
    utf8_percent_encode(text, URI_COMPONENT)
}

/// Builds the invite string, byte for byte as `formatInvite` in the launcher does.
pub fn format_invite(invite: &Invite) -> Result<String, FormatError> {
    let mut out = format!("{INVITE_SCHEME}://join?");
    match invite.mode {
        ServerMode::Public => {
            let fp = invite.fp.as_deref().filter(|fp| is_valid_fingerprint(fp));
            let fp = fp.ok_or(FormatError::Fingerprint)?;
            out += &format!(
                "v=2&mode=public&host={}&port={}&fp={fp}",
                encode(&invite.host),
                invite.port
            );
        }
        ServerMode::Private => {
            out += &format!("v=1&host={}&port={}", encode(&invite.host), invite.port);
        }
    }
    out += &format!("&code={}&name={}", encode(&invite.code), encode(&invite.name));
    if let (ServerMode::Private, Some(share)) = (invite.mode, invite.share.as_deref())
        && !share.is_empty()
    {
        out += &format!("&share={}", encode(share));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn private() -> Invite {
        Invite {
            mode: ServerMode::Private,
            host: "100.101.102.103".into(),
            port: 61000,
            code: "abcd-1234".into(),
            name: "Ramsgate Hunters' (EU) ✓".into(),
            share: Some("https://login.tailscale.com/admin/invite/AbC?x=1&y=2".into()),
            fp: None,
        }
    }

    fn public() -> Invite {
        Invite {
            mode: ServerMode::Public,
            host: "play.example.org".into(),
            port: 443,
            share: None,
            fp: Some(FP.into()),
            ..private()
        }
    }

    // Expected strings are the output of the launcher's formatInvite for the same input.
    #[test]
    fn formats_private_invites_like_the_launcher() {
        assert_eq!(
            format_invite(&private()).unwrap(),
            "dauntless-revived://join?v=1&host=100.101.102.103&port=61000&code=abcd-1234\
             &name=Ramsgate%20Hunters'%20(EU)%20%E2%9C%93\
             &share=https%3A%2F%2Flogin.tailscale.com%2Fadmin%2Finvite%2FAbC%3Fx%3D1%26y%3D2"
        );
    }

    #[test]
    fn formats_public_invites_like_the_launcher() {
        assert_eq!(
            format_invite(&public()).unwrap(),
            format!(
                "dauntless-revived://join?v=2&mode=public&host=play.example.org&port=443&fp={FP}\
                 &code=abcd-1234&name=Ramsgate%20Hunters'%20(EU)%20%E2%9C%93"
            )
        );
    }

    #[test]
    fn public_invites_need_a_fingerprint() {
        for fp in [None, Some(""), Some(&FP[1..]), Some(&*FP.to_uppercase())] {
            let invite = Invite {
                fp: fp.map(Into::into),
                ..public()
            };
            assert_eq!(format_invite(&invite), Err(FormatError::Fingerprint));
        }
    }

    #[test]
    fn share_is_left_out_of_public_invites() {
        let invite = Invite {
            share: Some("https://login.tailscale.com/x".into()),
            ..public()
        };
        assert!(!format_invite(&invite).unwrap().contains("share="));
    }

    #[test]
    fn json_matches_the_typescript_shape() {
        let json = serde_json::to_value(public()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "mode": "public", "host": "play.example.org", "port": 443, "code": "abcd-1234",
                "name": "Ramsgate Hunters' (EU) ✓", "share": null, "fp": FP,
            })
        );
        assert_eq!(serde_json::from_value::<Invite>(json).unwrap(), public());
    }
}
