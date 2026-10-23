//! Invite strings, as `legacy/ReforgedLauncher/src/shared/invite.ts` reads and writes them. Two versions:
//!
//! - v=1, private mode (the host's PC, reached over Tailscale, plain HTTP):
//!   `reforged-revival://join?v=1&host=…&port=61000&code=…&name=…[&share=…]`. The host must be a Tailscale
//!   address (100.64.0.0/10), a `*.ts.net` name or loopback: v1 must never point at the internet.
//! - v=2, public mode (a server on a public IP, reached through its TLS gateway):
//!   `reforged-revival://join?v=2&mode=public&host=…&port=…&fp=…&code=…&name=…`
//!
//! The host, port and fingerprint decide where the account key is sent later, so anything that is not
//! exactly one of these shapes is refused. Dauntless Revived's `dauntless-revived://` invites name servers
//! for Dauntless 1.4.4, which Reforged does not play (RD5): they are refused with [`InviteError::Revived`],
//! which the page words, never read as an [`Invite`].
//!
//! The string rules are JavaScript's, not Rust's: lengths in UTF-16 units, `trim` and `\s` with
//! ECMAScript's white space, `decodeURIComponent`'s strict UTF-8, and `new URL` for the share link.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::text::{collapse_spaces, is_unsafe_char, js_length, js_trim};

pub const INVITE_SCHEME: &str = "reforged-revival";
/// Dauntless Revived's scheme, recognised only to be refused (RD5).
pub const REVIVED_INVITE_SCHEME: &str = "dauntless-revived";
pub const DEFAULT_PUBLIC_PORT: u16 = 443;
pub const MAX_INVITE_LENGTH: usize = 2048;

const V1_ALLOWED: &[&str] = &["v", "host", "port", "code", "name", "share"];
const V1_REQUIRED: &[(&str, InviteError)] = &[
    ("host", InviteError::Host),
    ("port", InviteError::Port),
    ("code", InviteError::Code),
    ("name", InviteError::Name),
];
const V2_ALLOWED: &[&str] = &["v", "mode", "host", "port", "fp", "code", "name"];
const V2_REQUIRED: &[(&str, InviteError)] = &[
    ("mode", InviteError::Mode),
    ("host", InviteError::Host),
    ("fp", InviteError::Fp),
    ("code", InviteError::Code),
    ("name", InviteError::Name),
];

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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// Why an invite was refused. The JSON form is the TypeScript's `InviteError` name, plus `revived`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum InviteError {
    Empty,
    TooLong,
    NotInvite,
    /// A `dauntless-revived://join?…` invite: a server for Dauntless 1.4.4, which Reforged does not play
    /// (RD5). Its fields are not read.
    Revived,
    Version,
    Params,
    Mode,
    Host,
    Port,
    Fp,
    Code,
    Name,
    Share,
}

impl InviteError {
    /// The name the page shows in the error's detail, as the TypeScript returns it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLong => "too_long",
            Self::NotInvite => "not_invite",
            Self::Revived => "revived",
            Self::Version => "version",
            Self::Params => "params",
            Self::Mode => "mode",
            Self::Host => "host",
            Self::Port => "port",
            Self::Fp => "fp",
            Self::Code => "code",
            Self::Name => "name",
            Self::Share => "share",
        }
    }
}

impl std::fmt::Display for InviteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for InviteError {}

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

/// SHA-256 of the certificate's DER bytes, as 64 lower-case hex characters.
pub fn is_valid_fingerprint(fp: &str) -> bool {
    fp.len() == 64 && fp.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn is_valid_ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            let digits = part.len() <= 3 && !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            digits && (*part == "0" || !part.starts_with('0')) && part.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

/// IPv4, or a DNS name made of `[a-z0-9.-]`. Names a resolver could read as a number ("1.2.3",
/// "0x7f.1", "host.123") are refused.
pub fn is_valid_host(host: &str) -> bool {
    if host.is_empty() || js_length(host) > 253 {
        return false;
    }
    if host.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.') {
        return is_valid_ipv4(host);
    }
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-')
    {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels
        .iter()
        .any(|label| label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-'))
    {
        return false;
    }
    let last = labels.last().copied().unwrap_or_default();
    !last.bytes().all(|byte| byte.is_ascii_digit()) && !last.to_ascii_lowercase().starts_with("0x")
}

pub fn is_valid_port(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=5).contains(&bytes.len())
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
        && text.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
}

pub fn is_valid_invite_code(code: &str) -> bool {
    (4..=64).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub fn is_valid_share_url(text: &str) -> bool {
    if js_length(text) > 512 {
        return false;
    }
    let Ok(url) = url::Url::parse(text) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str() == Some("login.tailscale.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && text.to_lowercase().starts_with("https://login.tailscale.com/")
}

/// Server names are shown to the friend, always as text (never as HTML).
pub fn clean_server_name(raw: &str) -> Option<String> {
    // Control characters, and bidirectional overrides that could disguise the name
    if raw.chars().any(is_unsafe_char) {
        return None;
    }
    let name = collapse_spaces(js_trim(raw).chars());
    (js_length(&name) >= 1 && name.chars().count() <= 64).then_some(name)
}

pub fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts[0] == "127"
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Tailscale hands out addresses from 100.64.0.0/10.
pub fn is_tailscale_ipv4(host: &str) -> bool {
    if !is_valid_ipv4(host) {
        return false;
    }
    let mut parts = host.split('.').map(|part| part.parse::<u16>().unwrap_or(0));
    parts.next() == Some(100) && parts.next().is_some_and(|b| (64..=127).contains(&b))
}

/// A MagicDNS name: `<machine>.<tailnet>.ts.net`.
pub fn is_magic_dns_name(host: &str) -> bool {
    is_valid_host(host) && host.ends_with(".ts.net") && host.split('.').count() >= 3
}

/// Where private mode (plain HTTP, invite v1) may connect: a Tailscale address or MagicDNS name, or
/// this PC itself (tests, and a host playing on the server PC).
pub fn is_private_mode_host(host: &str) -> bool {
    is_tailscale_ipv4(host) || is_magic_dns_name(host) || is_loopback_host(host)
}

/// `decodeURIComponent(raw.replace(/\+/g, " "))`: every `%` starts two hex digits, and the bytes
/// they make must be UTF-8 (no surrogates, no overlong forms).
fn decode_component(raw: &str) -> Option<String> {
    let raw = raw.replace('+', " ");
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `/^<scheme>:\/\/join\/?\?/i`: the query after the prefix, or None.
fn strip_invite_prefix<'a>(text: &'a str, scheme: &str) -> Option<&'a str> {
    let prefix = format!("{scheme}://join");
    let head = text.get(..prefix.len())?;
    let rest = head.eq_ignore_ascii_case(&prefix).then(|| &text[prefix.len()..])?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    rest.strip_prefix('?')
}

/// Reads an invite as `parseInvite` does, but refuses a `dauntless-revived://` one (RD5).
pub fn parse_invite(input: &str) -> Result<Invite, InviteError> {
    let text = js_trim(input);
    if text.is_empty() {
        return Err(InviteError::Empty);
    }
    if js_length(text) > MAX_INVITE_LENGTH {
        return Err(InviteError::TooLong);
    }
    if strip_invite_prefix(text, REVIVED_INVITE_SCHEME).is_some() {
        return Err(InviteError::Revived);
    }
    let query = strip_invite_prefix(text, INVITE_SCHEME).ok_or(InviteError::NotInvite)?;
    if query.contains('#') {
        return Err(InviteError::Params);
    }

    // Parsed by hand, so that duplicates and unknown keys can be refused
    let mut values: Vec<(&str, String)> = Vec::new();
    for pair in query.split('&') {
        let Some((key, raw)) = pair.split_once('=') else {
            return Err(InviteError::Params);
        };
        if key.is_empty() || !V2_ALLOWED.contains(&key) && !V1_ALLOWED.contains(&key) {
            return Err(InviteError::Params);
        }
        if values.iter().any(|(seen, _)| *seen == key) {
            return Err(InviteError::Params);
        }
        values.push((key, decode_component(raw).ok_or(InviteError::Params)?));
    }
    let get = |key: &str| {
        values
            .iter()
            .find(|(seen, _)| *seen == key)
            .map(|(_, value)| value.as_str())
    };

    let version = get("v");
    if version != Some("1") && version != Some("2") {
        return Err(InviteError::Version);
    }
    let private = version == Some("1");
    let (allowed, required) = if private {
        (V1_ALLOWED, V1_REQUIRED)
    } else {
        (V2_ALLOWED, V2_REQUIRED)
    };
    if values.iter().any(|(key, _)| !allowed.contains(key)) {
        return Err(InviteError::Params);
    }
    if let Some((_, error)) = required.iter().find(|(key, _)| get(key).is_none()) {
        return Err(*error);
    }
    if !private && get("mode") != Some("public") {
        return Err(InviteError::Mode);
    }

    let host = get("host").unwrap_or_default().to_lowercase();
    if !is_valid_host(&host) {
        return Err(InviteError::Host);
    }
    // v1 is plain HTTP, so it may only point into the tailnet; a server on the internet needs a v2
    // (public, pinned TLS) invite
    if private && !is_private_mode_host(&host) {
        return Err(InviteError::Host);
    }

    let mut port = DEFAULT_PUBLIC_PORT;
    if let Some(text) = get("port") {
        if !is_valid_port(text) {
            return Err(InviteError::Port);
        }
        port = text.parse().map_err(|_| InviteError::Port)?;
    }

    let mut fp = None;
    if !private {
        let raw = get("fp").unwrap_or_default();
        if !is_valid_fingerprint(raw) {
            return Err(InviteError::Fp);
        }
        fp = Some(raw.to_owned());
    }

    let code = get("code").unwrap_or_default();
    if !is_valid_invite_code(code) {
        return Err(InviteError::Code);
    }

    let name = clean_server_name(get("name").unwrap_or_default()).ok_or(InviteError::Name)?;

    let mut share = None;
    if let Some(raw) = get("share") {
        if !is_valid_share_url(raw) {
            return Err(InviteError::Share);
        }
        share = Some(raw.to_owned());
    }

    Ok(Invite {
        mode: if private {
            ServerMode::Private
        } else {
            ServerMode::Public
        },
        host,
        port,
        code: code.to_owned(),
        name,
        share,
        fp,
    })
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
            "reforged-revival://join?v=1&host=100.101.102.103&port=61000&code=abcd-1234\
             &name=Ramsgate%20Hunters'%20(EU)%20%E2%9C%93\
             &share=https%3A%2F%2Flogin.tailscale.com%2Fadmin%2Finvite%2FAbC%3Fx%3D1%26y%3D2"
        );
    }

    #[test]
    fn formats_public_invites_like_the_launcher() {
        assert_eq!(
            format_invite(&public()).unwrap(),
            format!(
                "reforged-revival://join?v=2&mode=public&host=play.example.org&port=443&fp={FP}\
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

    #[test]
    fn errors_serialise_as_the_page_names_them() {
        for error in [
            InviteError::TooLong,
            InviteError::NotInvite,
            InviteError::Revived,
            InviteError::Fp,
        ] {
            assert_eq!(serde_json::to_value(error).unwrap(), serde_json::json!(error.as_str()));
        }
    }
}
