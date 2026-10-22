//! Strict IP address handling (`src/ip.ts`), shared by the gateway (peer addresses, rate-limit keys)
//! and the allowlist helper (what may go into the firewall rule). Only single addresses: no ranges,
//! no CIDR, no zone ids, no leading zeros, no surrounding white space.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpClass {
    Public,
    /// RFC 1918, carrier-grade NAT / Tailscale (100.64.0.0/10), IPv6 ULA, benchmarking
    Private,
    Loopback,
    LinkLocal,
    /// 0.0.0.0/8 and ::
    Unspecified,
    Multicast,
    /// 240.0.0.0/4 (with 255.255.255.255), 192.0.0.0/24, IPv6 outside 2000::/3, NAT64, discard
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIp {
    pub version: u8,
    pub canonical: String,
    pub class: IpClass,
    /// Four bytes for IPv4, sixteen for IPv6
    pub bytes: Vec<u8>,
}

/// `25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9][0-9]|[0-9]`: no leading zeros.
fn v4_part(part: &str) -> Option<u8> {
    let valid = !part.is_empty()
        && part.len() <= 3
        && part.bytes().all(|b| b.is_ascii_digit())
        && (part.len() == 1 || !part.starts_with('0'));
    if !valid {
        return None;
    }
    part.parse::<u16>().ok().filter(|n| *n <= 255).map(|n| n as u8)
}

fn parse_v4_bytes(text: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        bytes[i] = v4_part(part)?;
    }
    Some(bytes)
}

fn in_v4(value: u32, base: [u8; 4], bits: u32) -> bool {
    let base = u32::from_be_bytes(base);
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    value & mask == base & mask
}

fn classify_v4(bytes: [u8; 4]) -> IpClass {
    let value = u32::from_be_bytes(bytes);
    if in_v4(value, [0, 0, 0, 0], 8) {
        IpClass::Unspecified
    } else if in_v4(value, [127, 0, 0, 0], 8) {
        IpClass::Loopback
    } else if in_v4(value, [169, 254, 0, 0], 16) {
        IpClass::LinkLocal
    } else if in_v4(value, [224, 0, 0, 0], 4) {
        IpClass::Multicast
    } else if in_v4(value, [240, 0, 0, 0], 4) || in_v4(value, [192, 0, 0, 0], 24) {
        IpClass::Reserved
    } else if in_v4(value, [10, 0, 0, 0], 8)
        || in_v4(value, [172, 16, 0, 0], 12)
        || in_v4(value, [192, 168, 0, 0], 16)
        || in_v4(value, [100, 64, 0, 0], 10)
        || in_v4(value, [198, 18, 0, 0], 15)
    {
        IpClass::Private
    } else {
        IpClass::Public
    }
}

/// Eight 16-bit words; a dotted IPv4 tail is allowed in the last 32 bits.
fn parse_v6_words(text: &str) -> Option<[u16; 8]> {
    if text.len() < 2 || text.len() > 45 || !text.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') {
        return None;
    }
    let mut body = text.to_owned();
    let mut tail: Vec<u16> = Vec::new();
    let last_colon = body.rfind(':')?;
    let last_part = &body[last_colon + 1..];
    if last_part.contains('.') {
        let v4 = parse_v4_bytes(last_part)?;
        tail = vec![
            u16::from(v4[0]) << 8 | u16::from(v4[1]),
            u16::from(v4[2]) << 8 | u16::from(v4[3]),
        ];
        body.truncate(last_colon + 1);
        // "::1.2.3.4" leaves "::", "a::b:1.2.3.4" leaves "a::b:"; a lone trailing colon goes
        if body.ends_with(':') && !body.ends_with("::") {
            body.pop();
        }
    }
    let halves: Vec<&str> = body.split("::").collect();
    if halves.len() > 2 {
        return None;
    }
    let groups = |part: &str| -> Option<Vec<u16>> {
        if part.is_empty() {
            return Some(Vec::new());
        }
        part.split(':')
            .map(|g| {
                ((1..=4).contains(&g.len()) && g.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| u16::from_str_radix(g, 16).ok())
                    .flatten()
            })
            .collect()
    };
    let head = groups(halves[0])?;
    let words: Vec<u16> = if halves.len() == 1 {
        let words: Vec<u16> = head.into_iter().chain(tail).collect();
        if words.len() != 8 {
            return None;
        }
        words
    } else {
        let after = groups(halves[1])?;
        let known = head.len() + after.len() + tail.len();
        // "::" stands for at least one zero word
        if known > 7 {
            return None;
        }
        head.into_iter()
            .chain(std::iter::repeat_n(0, 8 - known))
            .chain(after)
            .chain(tail)
            .collect()
    };
    words.try_into().ok()
}

/// RFC 5952: lower case, no leading zeros, the longest run (the first on a tie) of two or more zero
/// words becomes "::".
pub fn format_v6(words: &[u16; 8]) -> String {
    let (mut best_start, mut best_length) = (0usize, 0usize);
    let mut index = 0;
    while index < 8 {
        if words[index] != 0 {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < 8 && words[end] == 0 {
            end += 1;
        }
        if end - index > best_length {
            best_start = index;
            best_length = end - index;
        }
        index = end;
    }
    let hex: Vec<String> = words.iter().map(|w| format!("{w:x}")).collect();
    if best_length < 2 {
        return hex.join(":");
    }
    format!(
        "{}::{}",
        hex[..best_start].join(":"),
        hex[best_start + best_length..].join(":")
    )
}

fn classify_v6(words: &[u16; 8]) -> IpClass {
    if words.iter().all(|w| *w == 0) {
        IpClass::Unspecified
    } else if words[..7].iter().all(|w| *w == 0) && words[7] == 1 {
        IpClass::Loopback
    } else if words[0] & 0xff00 == 0xff00 {
        IpClass::Multicast
    } else if words[0] & 0xffc0 == 0xfe80 {
        IpClass::LinkLocal
    } else if words[0] & 0xfe00 == 0xfc00 || words[0] & 0xffc0 == 0xfec0 {
        IpClass::Private
    } else if words[0] & 0xe000 != 0x2000 {
        // Global unicast is 2000::/3; NAT64 prefixes stand for IPv4 hosts and are never a real peer
        IpClass::Reserved
    } else {
        IpClass::Public
    }
}

/// `ParseIp`.
pub fn parse_ip(text: &str) -> Option<ParsedIp> {
    if text.is_empty() || text.len() > 45 {
        return None;
    }
    if let Some(v4) = parse_v4_bytes(text) {
        return Some(ParsedIp {
            version: 4,
            canonical: v4.map(|b| b.to_string()).join("."),
            class: classify_v4(v4),
            bytes: v4.to_vec(),
        });
    }
    let words = parse_v6_words(text)?;
    // ::ffff:a.b.c.d is an IPv4 peer seen through a dual-stack socket: that address
    if words[..5].iter().all(|w| *w == 0) && words[5] == 0xffff {
        let bytes = [
            (words[6] >> 8) as u8,
            words[6] as u8,
            (words[7] >> 8) as u8,
            words[7] as u8,
        ];
        return Some(ParsedIp {
            version: 4,
            canonical: bytes.map(|b| b.to_string()).join("."),
            class: classify_v4(bytes),
            bytes: bytes.to_vec(),
        });
    }
    Some(ParsedIp {
        version: 6,
        canonical: format_v6(&words),
        class: classify_v6(&words),
        bytes: words.iter().flat_map(|w| w.to_be_bytes()).collect(),
    })
}

/// A socket's remote address as the gateway reports it: canonical, IPv4-mapped addresses unwrapped.
pub fn peer_address(raw: Option<&str>) -> String {
    raw.and_then(parse_ip)
        .map_or_else(|| "unknown".to_owned(), |p| p.canonical)
}

/// Rate limits count per IPv4 address, and per /64 for IPv6.
pub fn rate_limit_key(address: &str) -> String {
    let Some(parsed) = parse_ip(address) else {
        return address.to_owned();
    };
    if parsed.version == 4 {
        return parsed.canonical;
    }
    let mut words = [0u16; 8];
    for (word, pair) in words.iter_mut().zip(parsed.bytes.chunks(2)).take(4) {
        *word = u16::from(pair[0]) << 8 | u16::from(pair[1]);
    }
    format!("{}/64", format_v6(&words))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowlistCheck {
    Ok(ParsedIp),
    Refused(&'static str),
}

/// `CheckAllowlistIp`: what may go into the firewall rule, exactly one routable address. Private,
/// loopback and link-local addresses only with `allow_private` (tests and LAN parties). `None` is a
/// value that is not a string.
pub fn check_allowlist_ip(text: Option<&str>, allow_private: bool) -> AllowlistCheck {
    let Some(text) = text else {
        return AllowlistCheck::Refused("not_a_string");
    };
    let named = matches!(
        text.to_ascii_lowercase().as_str(),
        "any" | "localsubnet" | "dns" | "dhcp" | "wins" | "defaultgateway" | "internet" | "intranet" | "localhost"
    );
    if text.contains(['/', ',', '-', '*']) || named {
        return AllowlistCheck::Refused("range_not_allowed");
    }
    let Some(parsed) = parse_ip(text) else {
        return AllowlistCheck::Refused("not_an_ip");
    };
    match parsed.class {
        IpClass::Public => AllowlistCheck::Ok(parsed),
        IpClass::Private | IpClass::Loopback | IpClass::LinkLocal => {
            if allow_private {
                AllowlistCheck::Ok(parsed)
            } else {
                AllowlistCheck::Refused("private")
            }
        }
        IpClass::Unspecified => AllowlistCheck::Refused("unspecified"),
        IpClass::Multicast => AllowlistCheck::Refused("multicast"),
        IpClass::Reserved => AllowlistCheck::Refused("reserved"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(text: &str) -> Option<String> {
        parse_ip(text).map(|p| p.canonical)
    }

    #[test]
    fn dotted_ipv4_without_leading_zeros() {
        assert_eq!(canonical("203.0.113.7").as_deref(), Some("203.0.113.7"));
        assert_eq!(parse_ip("0.0.0.0").unwrap().class, IpClass::Unspecified);
        assert_eq!(parse_ip("255.255.255.255").unwrap().class, IpClass::Reserved);
        for bad in [
            "010.0.0.1",
            "1.2.3",
            "1.2.3.4.5",
            "256.1.1.1",
            "1.2.3.-1",
            " 1.2.3.4",
            "1.2.3.4 ",
            "1.2.3.4\n",
            "0x7f.0.0.1",
            "1.2.3.4/32",
            "",
            "1..2.3",
        ] {
            assert_eq!(parse_ip(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn ipv6_is_canonical_and_ipv4_mapped_unwrapped() {
        assert_eq!(
            canonical("2001:0DB8:0000:0000:0000:0000:0000:0001").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(canonical("2001:db8:0:0:1:0:0:1").as_deref(), Some("2001:db8::1:0:0:1"));
        assert_eq!(canonical("2001:db8::").as_deref(), Some("2001:db8::"));
        assert_eq!(canonical("::").as_deref(), Some("::"));
        assert_eq!(canonical("::1").as_deref(), Some("::1"));
        assert_eq!(
            canonical("2001:db8:1:2:3:4:5:6").as_deref(),
            Some("2001:db8:1:2:3:4:5:6")
        );
        let mapped = parse_ip("::ffff:203.0.113.9").unwrap();
        assert_eq!((mapped.version, mapped.canonical.as_str()), (4, "203.0.113.9"));
        assert_eq!(canonical("::ffff:cb00:7109").as_deref(), Some("203.0.113.9"));
        for bad in [
            "fe80::1%eth0",
            "fe80::1%1",
            "2001:db8::/64",
            "2001:db8:::1",
            ":1::",
            "1:2:3:4:5:6:7:8:9",
            "1:2:3:4:5:6:7:8::",
            "g::1",
            "2001:db8::1 ",
            "[2001:db8::1]",
            "::ffff:010.0.0.1",
        ] {
            assert_eq!(parse_ip(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_special_ranges() {
        use IpClass::*;
        for (text, class) in [
            ("8.8.8.8", Public),
            ("203.0.113.7", Public),
            ("100.63.255.255", Public),
            ("100.128.0.0", Public),
            ("10.1.2.3", Private),
            ("172.16.0.1", Private),
            ("172.31.255.255", Private),
            ("172.32.0.1", Public),
            ("192.168.1.1", Private),
            ("100.64.0.1", Private),
            ("100.127.255.255", Private),
            ("198.18.0.1", Private),
            ("127.0.0.1", Loopback),
            ("127.255.0.9", Loopback),
            ("169.254.1.1", LinkLocal),
            ("0.1.2.3", Unspecified),
            ("224.0.0.1", Multicast),
            ("239.255.255.250", Multicast),
            ("240.0.0.1", Reserved),
            ("192.0.0.8", Reserved),
            ("2001:db8::1", Public),
            ("2a00:1450:4001::1", Public),
            ("::1", Loopback),
            ("fe80::1", LinkLocal),
            ("fd7a:115c:a1e0::1", Private),
            ("fc00::1", Private),
            ("fec0::1", Private),
            ("ff02::1", Multicast),
            ("::", Unspecified),
            ("::1.2.3.4", Reserved),
            ("64:ff9b::808:808", Reserved),
            ("100::1", Reserved),
        ] {
            assert_eq!(parse_ip(text).unwrap().class, class, "{text}");
        }
    }

    #[test]
    fn peers_and_rate_limit_keys() {
        assert_eq!(peer_address(Some("::ffff:127.0.0.1")), "127.0.0.1");
        assert_eq!(peer_address(Some("2001:DB8::5")), "2001:db8::5");
        assert_eq!(peer_address(None), "unknown");
        assert_eq!(rate_limit_key("203.0.113.7"), "203.0.113.7");
        assert_eq!(rate_limit_key("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64");
        assert_eq!(rate_limit_key("2001:db8:1:2:bbbb::9"), "2001:db8:1:2::/64");
        assert_ne!(rate_limit_key("2001:db8:1:3::1"), rate_limit_key("2001:db8:1:2::1"));
    }

    #[test]
    fn allowlist_addresses() {
        for (given, canonical) in [
            ("203.0.113.7", "203.0.113.7"),
            ("8.8.4.4", "8.8.4.4"),
            ("2001:DB8::0:1", "2001:db8::1"),
            ("::ffff:198.51.100.2", "198.51.100.2"),
        ] {
            match check_allowlist_ip(Some(given), false) {
                AllowlistCheck::Ok(ip) => assert_eq!(ip.canonical, canonical),
                other => panic!("{given}: {other:?}"),
            }
        }
        for (given, reason) in [
            ("0.0.0.0", "unspecified"),
            ("::", "unspecified"),
            ("0.0.0.0/0", "range_not_allowed"),
            ("203.0.113.0/24", "range_not_allowed"),
            ("203.0.113.1-203.0.113.9", "range_not_allowed"),
            ("203.0.113.1,203.0.113.2", "range_not_allowed"),
            ("*", "range_not_allowed"),
            ("Any", "range_not_allowed"),
            ("LocalSubnet", "range_not_allowed"),
            ("Internet", "range_not_allowed"),
            ("localhost", "range_not_allowed"),
            ("2001:db8::/32", "range_not_allowed"),
            ("255.255.255.255", "reserved"),
            ("224.0.0.251", "multicast"),
            ("ff02::fb", "multicast"),
            ("hello", "not_an_ip"),
            ("1.2.3.4;calc", "not_an_ip"),
            ("1.2.3.4; Remove-NetFirewallRule", "range_not_allowed"),
            ("1.2.3.4'", "not_an_ip"),
            ("", "not_an_ip"),
            ("1.2.3.4$(whoami)", "not_an_ip"),
            ("1.2.3.4`n", "not_an_ip"),
            (" 1.2.3.4", "not_an_ip"),
            ("fe80::1%12", "not_an_ip"),
            ("1.2.3.04", "not_an_ip"),
        ] {
            for allow_private in [false, true] {
                assert_eq!(
                    check_allowlist_ip(Some(given), allow_private),
                    AllowlistCheck::Refused(reason),
                    "{given}"
                );
            }
        }
        assert_eq!(check_allowlist_ip(None, true), AllowlistCheck::Refused("not_a_string"));
        for given in [
            "10.0.0.5",
            "192.168.1.20",
            "172.20.1.1",
            "100.101.102.103",
            "127.0.0.1",
            "169.254.3.3",
            "::1",
            "fe80::2",
            "fd00::5",
        ] {
            assert_eq!(
                check_allowlist_ip(Some(given), false),
                AllowlistCheck::Refused("private"),
                "{given}"
            );
            assert!(
                matches!(check_allowlist_ip(Some(given), true), AllowlistCheck::Ok(_)),
                "{given}"
            );
        }
    }
}
