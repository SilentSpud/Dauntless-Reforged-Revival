//! Invite parsing (row L11): `protocol::invite` (RS1), which reads `src/shared/invite.ts`'s two invite
//! shapes answer for answer and refuses a `dauntless-revived://` invite (RD5). The host, port and
//! fingerprint decide where the account key is sent later, so the launcher has no copy of its own: the
//! rules here are the servers' rules, and both sides run the same table
//! (`crates/protocol/tests/cases/invite.json`).

#[allow(unused_imports, reason = "the controller (RA13) and the platform rows use the rest")]
pub use protocol::invite::{
    DEFAULT_PUBLIC_PORT, INVITE_SCHEME, Invite, InviteError, MAX_INVITE_LENGTH, ServerMode, clean_server_name,
    format_invite, is_loopback_host, is_magic_dns_name, is_private_mode_host, is_tailscale_ipv4, is_valid_fingerprint,
    is_valid_host, is_valid_invite_code, is_valid_ipv4, is_valid_port, is_valid_share_url, parse_invite,
};

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

    /// The servers' table, through the launcher's path: what the launcher refuses is what they refuse.
    #[test]
    fn the_shared_table() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/protocol/tests/cases/invite.json"
        );
        let cases: Cases = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(cases.accept.len() > 30 && cases.reject.len() > 150);
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
        let link = format_invite(&invite).unwrap();
        assert!(link.starts_with("reforged-revival://join?v=2&"));
        assert_eq!(parse_invite(&link), Ok(invite));
    }

    #[test]
    fn dauntless_revived_invites_are_refused() {
        let link = format!(
            "dauntless-revived://join?v=2&mode=public&host=play.example.org&fp={}&code=ABCD&name=X",
            "3f".repeat(32)
        );
        assert_eq!(parse_invite(&link), Err(InviteError::Revived));
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
