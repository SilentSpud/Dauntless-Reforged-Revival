//! Invite parsing (row L11): `src/shared/invite.ts`, answer for answer. The host, port and fingerprint
//! decide where the account key is sent later, so anything that is not exactly one of the two invite
//! shapes is refused, and this copy is the one that decides (the page's copy only gives feedback).
//! Both run the same table, `test/cases/invite.json`.
//!
//! The string rules are JavaScript's, not Rust's: lengths in UTF-16 units, `trim` and `\s` with
//! ECMAScript's white space, `decodeURIComponent`'s strict UTF-8, and `new URL` for the share link.

use super::text::{is_js_space, is_unsafe_char, js_length, js_trim};
use protocol::invite::{DEFAULT_PUBLIC_PORT, Invite, ServerMode, is_valid_fingerprint};

const MAX_INVITE_LENGTH: usize = 2048;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    Empty,
    TooLong,
    NotInvite,
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
    let mut name = String::with_capacity(raw.len());
    let mut in_space = false;
    for c in js_trim(raw).chars() {
        if is_js_space(c) {
            if !in_space {
                name.push(' ');
            }
            in_space = true;
        } else {
            name.push(c);
            in_space = false;
        }
    }
    (js_length(&name) >= 1 && name.chars().count() <= 64).then_some(name)
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

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
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

pub fn parse_invite(input: &str) -> Result<Invite, InviteError> {
    let text = js_trim(input);
    if text.is_empty() {
        return Err(InviteError::Empty);
    }
    if js_length(text) > MAX_INVITE_LENGTH {
        return Err(InviteError::TooLong);
    }
    let rest = strip_prefix_ignore_case(text, "dauntless-revived://join").ok_or(InviteError::NotInvite)?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let query = rest.strip_prefix('?').ok_or(InviteError::NotInvite)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Cases {
        accept: Vec<Accept>,
        reject: Vec<Reject>,
    }

    #[derive(Deserialize)]
    struct Accept {
        input: String,
        invite: Invite,
    }

    #[derive(Deserialize)]
    struct Reject {
        input: String,
        error: String,
    }

    fn cases() -> Cases {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/invite.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_shared_table() {
        let cases = cases();
        assert!(cases.accept.len() > 30 && cases.reject.len() > 100);
        for case in cases.accept {
            assert_eq!(parse_invite(&case.input), Ok(case.invite), "{:?}", case.input);
        }
        for case in cases.reject {
            assert_eq!(
                parse_invite(&case.input).map_err(InviteError::as_str),
                Err(case.error.as_str()),
                "{:?}",
                case.input
            );
        }
    }

    #[test]
    fn invites_the_servers_build_parse_back() {
        let invite = Invite {
            mode: ServerMode::Public,
            host: "play.example.org".into(),
            port: 8443,
            code: "XXXX-YYYY".into(),
            name: "Aether & Ember".into(),
            share: None,
            fp: Some("3f".repeat(32)),
        };
        assert_eq!(
            parse_invite(&protocol::invite::format_invite(&invite).unwrap()),
            Ok(invite)
        );
    }

    #[test]
    fn host_rules() {
        assert!(is_private_mode_host("100.64.0.1"));
        assert!(is_private_mode_host("my-pc.tail-scale.ts.net"));
        assert!(is_private_mode_host("127.0.0.1"));
        assert!(is_private_mode_host("localhost"));
        assert!(!is_private_mode_host("203.0.113.9"));
        assert!(!is_private_mode_host("10.0.0.5"));
        assert!(!is_private_mode_host("example.com"));
        assert!(!is_private_mode_host("ts.net"));
        assert!(!is_valid_host("MyPC"));
        assert!(!is_valid_host("a_b"));
        assert!(is_tailscale_ipv4("100.127.255.255"));
        assert!(!is_tailscale_ipv4("100.128.0.1"));
        assert!(!is_valid_fingerprint(&"3F".repeat(32)));
    }
}
