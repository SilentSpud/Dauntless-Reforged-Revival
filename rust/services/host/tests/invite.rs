//! `invite` (MIGRATION_PLAN.md RH3): the Node kit's invite tests (`kit.test.ts`, `ports.test.ts`) and
//! the command against a fake backend. That the links equal the Node kit's for the same inputs is
//! `tests/setup.rs`'s `makes_the_node_kits_invites`.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::invite::{ServerMode as InviteMode, parse_invite};
use serde_json::{Value, json};

use host::cli::InviteArgs;
use host::hostconfig::{DEFAULT_MONGO_URI, HostConfig, Secrets, load_or_create_secrets, write_host_config};
use host::invite::{CodeOptions, certificate_fingerprint, create_invite_code, invite_for, served_fingerprint};
use host::paths::Paths;

fn config(changes: Value) -> HostConfig {
    let mut doc = json!({
        "version": 1, "mode": "private", "host": "100.101.102.103", "gameAddress": "100.101.102.103",
        "name": "Friday Hunts & Co", "gameDir": null, "registration": "INVITECODE",
        "shareUrl": "https://login.tailscale.com/admin/invite/abcDEF123",
        "mongo": { "uri": DEFAULT_MONGO_URI, "manage": true, "mongod": null }, "maxHuntServers": 3
    });
    for (k, v) in changes.as_object().unwrap() {
        doc[k] = v.clone();
    }
    HostConfig::from_json(&doc).unwrap()
}

#[test]
fn are_links_the_launcher_accepts_private_and_public() {
    let private = parse_invite(&invite_for(&config(json!({})), "ABCD-EFGH-JKMN", None).unwrap()).unwrap();
    assert_eq!(private.mode, InviteMode::Private);
    assert_eq!(
        (
            private.host.as_str(),
            private.port,
            private.code.as_str(),
            private.name.as_str()
        ),
        ("100.101.102.103", 61000, "ABCD-EFGH-JKMN", "Friday Hunts & Co")
    );
    assert_eq!(
        private.share.as_deref(),
        Some("https://login.tailscale.com/admin/invite/abcDEF123")
    );
    assert_eq!(private.fp, None);
    let fp = "ab".repeat(32);
    let public = config(
        json!({ "mode": "public", "host": "hunts.example.net", "gameAddress": "203.0.113.7", "shareUrl": null }),
    );
    let public = parse_invite(&invite_for(&public, "ABCD-EFGH-JKMN", Some(&fp)).unwrap()).unwrap();
    assert_eq!(public.mode, InviteMode::Public);
    assert_eq!((public.host.as_str(), public.port), ("hunts.example.net", 443));
    assert_eq!((public.share, public.fp), (None, Some(fp)));
}

/// What the fake backend was sent: each request line and body.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A backend that answers the admin routes, recording what it was sent.
fn fake_backend(answer: fn(&str, &str) -> (u16, String)) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let (head, body) = loop {
                let n = stream.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
                if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
                    let length: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    while raw.len() < end + 4 + length {
                        let n = stream.read(&mut buf).unwrap();
                        raw.extend_from_slice(&buf[..n]);
                    }
                    break (
                        head,
                        String::from_utf8_lossy(&raw[end + 4..end + 4 + length]).into_owned(),
                    );
                }
            };
            let key = head
                .lines()
                .find_map(|l| l.strip_prefix("x-reforged-admin-key: "))
                .unwrap_or_default()
                .to_owned();
            record
                .lock()
                .unwrap()
                .push((head.lines().next().unwrap().to_owned(), body.clone()));
            let (status, reply) = answer(&key, &body);
            let text = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            stream.write_all(text.as_bytes()).unwrap();
        }
    });
    (port, seen)
}

fn key_check(key: &str, _body: &str) -> (u16, String) {
    if key == "k".repeat(64) {
        (200, r#"{"code":"WXYZ-2345-6789"}"#.into())
    } else {
        (403, String::new())
    }
}

#[test]
fn get_their_code_from_the_backends_local_admin_route_with_the_admin_key() {
    let (port, seen) = fake_backend(key_check);
    let options = CodeOptions {
        uses: 2.0,
        infinite: false,
        note: Some("Alex".into()),
    };
    assert_eq!(
        create_invite_code(&"k".repeat(64), &options, port).unwrap(),
        "WXYZ-2345-6789"
    );
    let (line, body) = seen.lock().unwrap()[0].clone();
    assert_eq!(line, "POST /reforged/api/CreateInvite HTTP/1.1");
    assert_eq!(
        body, r#"{"uses":2,"infinite":false,"name":"Alex"}"#,
        "the Node kit's body, key for key"
    );
    let refused = create_invite_code(
        &"wrong".repeat(10),
        &CodeOptions {
            uses: 1.0,
            ..Default::default()
        },
        port,
    );
    assert!(refused.unwrap_err().to_string().contains("admin key"));

    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let nobody = closed.local_addr().unwrap().port();
    drop(closed);
    let error = create_invite_code(
        &"k".repeat(64),
        &CodeOptions {
            uses: 1.0,
            ..Default::default()
        },
        nobody,
    )
    .unwrap_err();
    assert!(error.to_string().contains("not running"), "{error}");
}

fn fast_secrets() -> Result<Secrets, host::HostError> {
    Ok(Secrets {
        gateway_secret: "a".repeat(64),
        allowlist_secret: "b".repeat(64),
        admin_key: "k".repeat(64),
        gameserver_key: "d".repeat(64),
        api_key_hash_secret: "e".repeat(64),
        metrics_token: "f".repeat(64),
        jwt_private_key_b64: "g".repeat(64),
        jwt_public_key_b64: "h".repeat(64),
        admin_api_key: "i".repeat(64),
        operator_secret: "j".repeat(64),
    })
}

fn invite(p: &Paths, args: InviteArgs, port: u16) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    host::commands::invite::run_with(p, &args, port, &mut |l| lines.push(l.to_owned()))
        .map(|_| lines)
        .map_err(|e| e.to_string())
}

#[test]
fn the_command_prints_the_link_and_the_launchers_json_line() {
    let dir = tempfile::tempdir().unwrap();
    let p = Paths::new(dir.path());
    let args = |json: bool| InviteArgs {
        uses: "3".into(),
        for_whom: Some("Alex".into()),
        json,
        ..Default::default()
    };
    assert_eq!(
        invite(&p, args(false), 1).unwrap_err(),
        "not set up yet: run reforged-host setup first"
    );
    write_host_config(&p.host_json, &config(json!({}))).unwrap();
    load_or_create_secrets(&p.config, fast_secrets).unwrap();
    let (port, seen) = fake_backend(key_check);
    let link = invite_for(&config(json!({})), "WXYZ-2345-6789", None).unwrap();

    let lines = invite(&p, args(false), port).unwrap();
    assert_eq!(
        lines,
        [
            "\nInvite for Alex (3 uses):\n".to_owned(),
            format!("  {link}\n"),
            "Send the whole line. They paste it into the Dauntless Reforged Revival launcher (or click it once the launcher is installed).".into(),
            "They also need Tailscale, and access to this PC: share it from https://login.tailscale.com/admin/machines.".into(),
        ]
    );
    assert_eq!(
        seen.lock().unwrap()[0].1,
        r#"{"uses":3,"infinite":false,"name":"Alex"}"#
    );
    let lines = invite(&p, args(true), port).unwrap();
    assert_eq!(
        lines,
        [format!(
            r#"{{"link":{},"for":"Alex","uses":3,"shareUrl":"https://login.tailscale.com/admin/invite/abcDEF123"}}"#,
            serde_json::to_string(&link).unwrap()
        )]
    );
    let unlimited = InviteArgs {
        unlimited: true,
        json: true,
        ..Default::default()
    };
    let line = invite(&p, unlimited, port).unwrap().remove(0);
    assert!(
        line.ends_with(r#","for":null,"uses":null,"shareUrl":"https://login.tailscale.com/admin/invite/abcDEF123"}"#),
        "{line}"
    );
    assert_eq!(seen.lock().unwrap()[2].1, r#"{"uses":1,"infinite":true}"#);
}

#[test]
fn a_public_invite_pins_the_certificate_the_port_serves() {
    let cert = rcgen::generate_simple_self_signed(vec!["test".into()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cert.pem");
    std::fs::write(&file, cert.cert.pem()).unwrap();
    let ours = certificate_fingerprint(&file).unwrap();
    assert_eq!(ours.len(), 64);

    let chain = vec![cert.cert.der().clone()];
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let server = Arc::new(server);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut socket, _)) = listener.accept() {
            let mut tls = rustls::ServerConnection::new(server).unwrap();
            while tls.is_handshaking() {
                if tls.complete_io(&mut socket).is_err() {
                    break;
                }
            }
        }
    });
    assert_eq!(
        served_fingerprint(port, "127.0.0.1", Duration::from_secs(4)),
        Some(ours)
    );
}
