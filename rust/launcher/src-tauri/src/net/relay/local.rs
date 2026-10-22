//! What the relay reads from a local request before anything is forwarded: the Host and Origin headers,
//! the QoS path and the port override. Only `std` here, so `rust/fuzz` compiles this file as it is
//! (target `relay_input`).

use std::net::IpAddr;

/// The page the 1.14.7 client expects from a region's QoS URL.
pub const QOS_PONG: &str = "<!DOCTYPE html><html><body>pong</body></html>";

/// `/^\/qos(?:[/?]|$)/i` on the request target.
pub fn is_qos_path(target: &str) -> bool {
    let bytes = target.as_bytes();
    bytes.len() >= 4 && bytes[..4].eq_ignore_ascii_case(b"/qos") && matches!(bytes.get(4), None | Some(b'/' | b'?'))
}

/// The hidden override for the relay port (`REFORGED_RELAY_PORT`; tests and rehearsals on a PC where
/// 61100 is taken): 4 or 5 digits, 1024 to 65535.
pub fn relay_port_override(value: Option<&str>) -> Option<u16> {
    let text = value?.trim();
    let shaped =
        (4..=5).contains(&text.len()) && !text.starts_with('0') && text.bytes().all(|byte| byte.is_ascii_digit());
    text.parse::<u16>().ok().filter(|port| shaped && *port >= 1024)
}

pub fn is_loopback_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == "localhost" || name == "::1" {
        return true;
    }
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() == 4
        && parts[0] == "127"
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// "127.0.0.1:61100", "localhost", "[::1]:61100" -> the host part
pub fn host_part(value: &str) -> &str {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('[')
        && let Some((host, rest)) = inner.split_once(']')
        && (rest.is_empty()
            || rest
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())))
    {
        return host;
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => value,
    }
}

/// `/^(?:[a-z][a-z0-9+.-]*:\/\/)?(\[[^\]]+\]|[^/:]+)(?::\d+)?\/?$/i`: the host an Origin names.
pub fn origin_host(origin: &str) -> Option<&str> {
    let mut rest = origin.trim();
    if let Some((scheme, after)) = rest.split_once("://") {
        let mut chars = scheme.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
        if valid {
            rest = after;
        }
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, after) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        (host, after)
    } else {
        let end = rest.find([':', '/']).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        (&rest[..end], &rest[end..])
    };
    let port_ok = after.is_empty()
        || after
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    port_ok.then_some(host)
}

/// The Origin header as the relay sees it: absent, text, or bytes that are not text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin<'a> {
    Absent,
    Text(&'a str),
    Unreadable,
}

/// Why a request must be refused, or None. The game sends `Host: 127.0.0.1:<port>` and no browser
/// headers. A web page cannot avoid sending `Sec-Fetch-*` on plain requests, and a browser WebSocket
/// always carries the page's Origin; DNS rebinding shows up as a foreign Host. `host` is None when the
/// header is missing or not text.
pub fn request_problem(
    remote: IpAddr,
    host: Option<&str>,
    origin: Origin<'_>,
    fetch_metadata: bool,
    upgrade: bool,
) -> Option<&'static str> {
    let ip = match remote {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        ip => ip,
    };
    if !ip.is_loopback() {
        return Some("not_local");
    }
    if !host.is_some_and(|host| is_loopback_name(host_part(host))) {
        return Some("host");
    }
    if !upgrade && fetch_metadata {
        return Some("browser");
    }
    match origin {
        Origin::Absent => None,
        Origin::Text(origin) if origin_host(origin).is_some_and(is_loopback_name) => None,
        Origin::Text(_) | Origin::Unreadable => Some("origin"),
    }
}
