//! `legacy/ReforgedLauncher/test/invite.test.ts`, case for case. The one change is its last test: a
//! `dauntless-revived://` invite is refused as `revived` instead of read as a 1.4.4 server (RD5). Its
//! "rejects non-strings" has no counterpart: the parser takes `&str`, and the bridge refuses a non-string
//! argument before it reaches Rust (RA2).

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use protocol::invite::{
    Invite, InviteError, ServerMode, format_invite, is_private_mode_host, is_tailscale_ipv4, is_valid_fingerprint,
    is_valid_host, parse_invite,
};

fn fp() -> String {
    "3f".repeat(32)
}

const GOOD: &str =
    "reforged-revival://join?v=1&host=100.101.102.103&port=61000&code=ABCD-1234&name=Alex%27s%20Ramsgate";
const RLO: char = '\u{202e}';

fn good_v2() -> String {
    format!(
        "reforged-revival://join?v=2&mode=public&host=203.0.113.10&port=443&fp={}&code=ABCD-EFGH-JKLM&name=Friday%20Hunts",
        fp()
    )
}

/// `encodeURIComponent`
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out += &format!("%{byte:02X}");
        }
    }
    out
}

fn invite(
    mode: ServerMode,
    host: &str,
    port: u16,
    code: &str,
    name: &str,
    share: Option<&str>,
    fp: Option<&str>,
) -> Invite {
    Invite {
        mode,
        host: host.into(),
        port,
        code: code.into(),
        name: name.into(),
        share: share.map(Into::into),
        fp: fp.map(Into::into),
    }
}

// ------------------------------------------------------------------ v1 (private, Tailscale)

#[test]
fn v1_parses_a_valid_invite() {
    assert_eq!(
        parse_invite(GOOD),
        Ok(invite(
            ServerMode::Private,
            "100.101.102.103",
            61000,
            "ABCD-1234",
            "Alex's Ramsgate",
            None,
            None
        ))
    );
}

#[test]
fn v1_accepts_surrounding_whitespace_a_magic_dns_name_a_share_link_and_an_upper_case_scheme() {
    let share = encode("https://login.tailscale.com/admin/invite/abcDEF123");
    let r = parse_invite(&format!(
        "  REFORGED-REVIVAL://join/?v=1&host=HostPC.tail1234.ts.net&port=61000&code=abcd&name=Friends+Night&share={share}\n"
    ))
    .unwrap();
    assert_eq!(r.mode, ServerMode::Private);
    assert_eq!(r.host, "hostpc.tail1234.ts.net");
    assert_eq!(r.name, "Friends Night");
    assert_eq!(
        r.share.as_deref(),
        Some("https://login.tailscale.com/admin/invite/abcDEF123")
    );
}

#[test]
fn v1_round_trips_through_format_invite() {
    let inv = invite(
        ServerMode::Private,
        "100.64.0.1",
        61000,
        "Code-42",
        "Ember & Aether",
        Some("https://login.tailscale.com/x"),
        None,
    );
    assert_eq!(parse_invite(&format_invite(&inv).unwrap()), Ok(inv));
}

// ------------------------------------------------------------------ v2 (public, TLS gateway)

#[test]
fn v2_parses_a_valid_public_invite() {
    assert_eq!(
        parse_invite(&good_v2()),
        Ok(invite(
            ServerMode::Public,
            "203.0.113.10",
            443,
            "ABCD-EFGH-JKLM",
            "Friday Hunts",
            None,
            Some(&fp())
        ))
    );
}

#[test]
fn v2_the_port_defaults_to_443_a_dns_name_and_any_parameter_order_work() {
    let r = parse_invite(&format!(
        "reforged-revival://join?name=Hunts&code=abcd&fp={}&host=Play.Example.ORG&mode=public&v=2",
        fp()
    ))
    .unwrap();
    assert_eq!(r.port, 443);
    assert_eq!(r.host, "play.example.org");
    assert_eq!(r.mode, ServerMode::Public);
}

#[test]
fn v2_a_custom_gateway_port() {
    let r = parse_invite(&format!(
        "reforged-revival://join?v=2&mode=public&host=198.51.100.7&port=8443&fp={}&code=abcd&name=x",
        fp()
    ))
    .unwrap();
    assert_eq!(r.port, 8443);
}

#[test]
fn v2_round_trips_through_format_invite() {
    let inv = invite(
        ServerMode::Public,
        "play.example.org",
        8443,
        "XXXX-YYYY",
        "Aether & Ember",
        None,
        Some(&fp()),
    );
    assert_eq!(parse_invite(&format_invite(&inv).unwrap()), Ok(inv.clone()));
    assert!(format_invite(&Invite { fp: None, ..inv }).is_err());
}

/// `build2`: the good v2 parameters with some replaced (`Some`) or left out (`None`), plus new ones
/// appended in order.
fn build2(over: &[(&str, Option<&str>)]) -> String {
    let fp = fp();
    let mut params: Vec<(String, Option<String>)> = [
        ("v", "2"),
        ("mode", "public"),
        ("host", "203.0.113.10"),
        ("port", "443"),
        ("fp", fp.as_str()),
        ("code", "abcd"),
        ("name", "x"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), Some((*v).to_owned())))
    .collect();
    for (key, value) in over {
        match params.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.map(Into::into),
            None => params.push(((*key).to_owned(), value.map(Into::into))),
        }
    }
    let joined: Vec<String> = params
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{k}={v}")))
        .collect();
    format!("reforged-revival://join?{}", joined.join("&"))
}

// ------------------------------------------------------------------ malformed

fn bad() -> Vec<(String, InviteError)> {
    use InviteError::*;
    let fp = fp();
    let p = "reforged-revival://join?";
    let base1 = "v=1&host=100.64.0.1&port=61000&code=abcd";
    let mut cases: Vec<(String, InviteError)> = vec![
        ("".into(), Empty),
        ("   ".into(), Empty),
        ("hello".into(), NotInvite),
        ("https://example.com/join?v=1".into(), NotInvite),
        (
            "reforged-revival://register?v=1&host=1.2.3.4&port=1&code=abcd&name=x".into(),
            NotInvite,
        ),
        (
            "reforged-revival:join?v=1&host=1.2.3.4&port=1&code=abcd&name=x".into(),
            NotInvite,
        ),
        (format!("{p}v=3&host=100.64.0.1&port=61000&code=abcd&name=x"), Version),
        (format!("{p}v=01&host=100.64.0.1&port=61000&code=abcd&name=x"), Version),
        (format!("{p}host=100.64.0.1&port=61000&code=abcd&name=x"), Version),
        (format!("{p}v=1&port=61000&code=abcd&name=x"), Host),
        (format!("{p}v=1&host=100.64.0.1&code=abcd&name=x"), Port),
        (format!("{p}v=1&host=100.64.0.1&port=61000&name=x"), Code),
        (format!("{p}v=1&host=100.64.0.1&port=61000&code=abcd"), Name),
        (format!("{p}{base1}&name=x&extra=1"), Params),
        (
            format!("{p}v=1&v=1&host=100.64.0.1&port=61000&code=abcd&name=x"),
            Params,
        ),
        (format!("{p}{base1}&name=x#frag"), Params),
        (format!("{p}{base1}&name=%E0%A4%A"), Params),
        (format!("{p}{base1}&name=x&"), Params),
        (format!("{p}{base1}&=x"), Params),
        // v1 does not take the v2 fields, and v2 does not take share
        (format!("{p}{base1}&name=x&fp={fp}"), Params),
        (
            format!("{p}v=1&mode=public&host=100.64.0.1&port=61000&code=abcd&name=x"),
            Params,
        ),
    ];
    for host in [
        "256.1.1.1",
        "1.2.3",
        "01.2.3.4",
        "evil.com%2F%40x",
        "user%40host",
        "host.123",
        "0x7f.1",
        "-bad-.ts.net",
        "a..b",
        "%5B%3A%3A1%5D",
        // v1 is plain HTTP: only Tailscale addresses, *.ts.net names and loopback
        "203.0.113.9",
        "evil.example.com",
        "192.168.1.10",
        "100.128.0.1",
        "ts.net",
        "hostpc.ts.net.evil.com",
        "hostpc",
    ] {
        cases.push((format!("{p}v=1&host={host}&port=61000&code=abcd&name=x"), Host));
    }
    for port in ["0", "65536", "061000", "61000x"] {
        cases.push((format!("{p}v=1&host=100.64.0.1&port={port}&code=abcd&name=x"), Port));
    }
    for code in ["abc".to_owned(), "ab%20cd".to_owned(), "a".repeat(65)] {
        cases.push((format!("{p}v=1&host=100.64.0.1&port=61000&code={code}&name=x"), Code));
    }
    for name in [
        "%20%20".to_owned(),
        "a%0Ab".to_owned(),
        format!("{}evil", encode(&RLO.to_string())),
        "n".repeat(65),
    ] {
        cases.push((format!("{p}{base1}&name={name}"), Name));
    }
    for share in [
        "http%3A%2F%2Flogin.tailscale.com%2Fx",
        "https%3A%2F%2Fevil.com%2F",
        "https%3A%2F%2Flogin.tailscale.com.evil.com%2F",
        "https%3A%2F%2Fa%40login.tailscale.com%2F",
        "https%3A%2F%2Flogin.tailscale.com%3A8443%2F",
    ] {
        cases.push((format!("{p}{base1}&name=x&share={share}"), Share));
    }
    cases.push((format!("{p}{}", "x".repeat(3000)), TooLong));
    // v2
    let fp_colons: String = fp
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":");
    let share = encode("https://login.tailscale.com/x");
    let v2: Vec<(String, InviteError)> = vec![
        (build2(&[("mode", None)]), Mode),
        (build2(&[("mode", Some("private"))]), Mode),
        (build2(&[("mode", Some("PUBLIC"))]), Mode),
        (build2(&[("mode", Some(""))]), Mode),
        (build2(&[("fp", None)]), Fp),
        (build2(&[("fp", Some(&fp.to_uppercase()))]), Fp),
        (build2(&[("fp", Some(&fp[..63]))]), Fp),
        (build2(&[("fp", Some(&format!("{fp}a")))]), Fp),
        (build2(&[("fp", Some(&format!("{}zz", &fp[..62])))]), Fp),
        (build2(&[("fp", Some(&fp_colons))]), Fp),
        (build2(&[("fp", Some(&format!("sha256%2F{fp}")))]), Fp),
        (build2(&[("host", None)]), Host),
        (build2(&[("host", Some("203.0.113.999"))]), Host),
        (build2(&[("host", Some("exa_mple.com"))]), Host),
        (build2(&[("host", Some("https%3A%2F%2Fexample.com"))]), Host),
        (build2(&[("port", Some("0"))]), Port),
        (build2(&[("port", Some("443.0"))]), Port),
        (build2(&[("port", Some(""))]), Port),
        (build2(&[("code", None)]), Code),
        (build2(&[("code", Some("a+b+c+d"))]), Code),
        (build2(&[("name", None)]), Name),
        (build2(&[("name", Some("%09tab"))]), Name),
        (build2(&[("share", Some(&share))]), Params),
        (build2(&[("extra", Some("1"))]), Params),
        (format!("{}&fp={fp}", build2(&[])), Params),
    ];
    cases.extend(v2);
    cases
}

#[test]
fn rejects_malformed_invites() {
    let cases = bad();
    assert_eq!(cases.len(), 80, "every case of invite.test.ts's table");
    for (input, error) in cases {
        assert_eq!(parse_invite(&input), Err(error), "{input:?}");
    }
}

#[test]
fn v1_loopback_is_allowed() {
    for host in ["127.0.0.1", "localhost"] {
        assert!(
            parse_invite(&format!(
                "reforged-revival://join?v=1&host={host}&port=61000&code=abcd&name=x"
            ))
            .is_ok(),
            "{host}"
        );
    }
}

#[test]
fn v2_still_takes_any_public_address_or_dns_name() {
    for host in ["203.0.113.9", "evil.example.com", "hostpc.tail1234.ts.net"] {
        assert!(parse_invite(&build2(&[("host", Some(host))])).is_ok(), "{host}");
    }
}

#[test]
fn private_mode_host_rule() {
    assert!(is_private_mode_host("100.64.0.1"));
    assert!(is_private_mode_host("my-pc.tail-scale.ts.net"));
    assert!(is_private_mode_host("127.0.0.1"));
    assert!(is_private_mode_host("localhost"));
    assert!(!is_private_mode_host("203.0.113.9"));
    assert!(!is_private_mode_host("10.0.0.5"));
    assert!(!is_private_mode_host("example.com"));
    assert!(!is_private_mode_host("ts.net"));
}

#[test]
fn host_and_fingerprint_rules() {
    assert!(is_valid_host("100.64.0.1"));
    assert!(is_valid_host("localhost"));
    assert!(is_valid_host("my-pc.tail-scale.ts.net"));
    assert!(!is_valid_host("MyPC")); // upper case is lowered by the parser, not here
    assert!(!is_valid_host("a_b"));
    assert!(is_tailscale_ipv4("100.64.0.1"));
    assert!(is_tailscale_ipv4("100.127.255.255"));
    assert!(!is_tailscale_ipv4("100.128.0.1"));
    assert!(!is_tailscale_ipv4("192.168.1.1"));
    assert!(is_valid_fingerprint(&fp()));
    assert!(!is_valid_fingerprint(&fp().to_uppercase()));
    assert!(!is_valid_fingerprint(""));
}

#[test]
fn dauntless_revived_invites_are_refused_as_1_4_4_servers() {
    let fp = "ab".repeat(32);
    for input in [
        "dauntless-revived://join?v=1&host=100.88.12.40&port=61000&code=ABCD-1234&name=Revived%20Fridays".to_owned(),
        format!(
            "dauntless-revived://join?v=2&mode=public&host=revived.example.net&port=443&fp={fp}&code=XYZ-99&name=Open"
        ),
        // Its fields are not read: a refusal TypeScript gave for the host is now the edition's
        "dauntless-revived://join?v=1&host=203.0.113.7&port=61000&code=ABCD-1234&name=X".to_owned(),
        "  DAUNTLESS-REVIVED://join/?v=9\n".to_owned(),
    ] {
        assert_eq!(parse_invite(&input), Err(InviteError::Revived), "{input:?}");
    }
    // Only the invite prefix: anything else from that scheme is not an invite at all
    assert_eq!(
        parse_invite("dauntless-revived://register?v=1"),
        Err(InviteError::NotInvite)
    );
    assert_eq!(
        parse_invite(&format!("dauntless-revived://join?{}", "x".repeat(3000))),
        Err(InviteError::TooLong)
    );
    let ours = parse_invite("reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=Code-42&name=Ours");
    assert!(ours.is_ok());
    // And formatInvite writes Reforged's scheme only
    let inv = invite(
        ServerMode::Private,
        "100.88.12.40",
        61000,
        "ABCD-1234",
        "Revived Fridays",
        None,
        None,
    );
    assert!(format_invite(&inv).unwrap().starts_with("reforged-revival://join?"));
}
