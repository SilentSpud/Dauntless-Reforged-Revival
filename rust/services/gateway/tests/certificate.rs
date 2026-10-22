#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

//! An existing server keeps its certificate (section 6: every invite's `fp`). `fixtures/node-kit`
//! holds a certificate and key made by the Node `tools/make-cert.js` and a public invite made from
//! them by the Node host kit (`formatInvite` with `certificateFingerprint`); the key is a test key
//! and guards nothing. Made with, from `legacy/`:
//!
//!   node ReforgedGateway/tools/make-cert.js --host 127.0.0.1 --host play.example.org --out <fixtures>
//!   (ReforgedHost: npx tsc, then formatInvite({ mode: "public", host: "play.example.org", port: 443,
//!   code: "ABCD-EFGH-JKMN", name: "Parity Test", fp: certificateFingerprint(<cert>), share: null }))

mod common;

use std::path::{Path, PathBuf};

use common::*;
use gateway::cert;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/node-kit")
}

fn node_cert() -> TestCert {
    // A copy, so nothing a test does can touch the fixture
    let dir = tempfile::tempdir().unwrap();
    let (cert_file, key_file) = (dir.path().join("gateway-cert.pem"), dir.path().join("gateway-key.pem"));
    std::fs::copy(fixtures().join("gateway-cert.pem"), &cert_file).unwrap();
    std::fs::copy(fixtures().join("gateway-key.pem"), &key_file).unwrap();
    let info = cert::load(&cert_file, &key_file).unwrap().info;
    TestCert {
        cert_pem: std::fs::read_to_string(&cert_file).unwrap(),
        dir,
        cert_file,
        key_file,
        info,
    }
}

fn invite_fp() -> String {
    let link = std::fs::read_to_string(fixtures().join("invite.txt")).unwrap();
    protocol::invite::parse_invite(link.trim()).unwrap().fp.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_node_kits_certificate_loads_and_its_invite_still_pins_the_gateway() {
    let cert = node_cert();
    let fp = invite_fp();
    assert_eq!(cert.info.fingerprint, fp);
    assert_eq!(cert.info.subject_alt_name, "IP Address:127.0.0.1, DNS:play.example.org");
    let upstream = start_upstream("backend", None).await;
    let ports = Ports {
        backend: upstream.port,
        content: dead_port().await,
        ws: dead_port().await,
    };
    let running = start_gateway(&cert, test_config(&cert, &ports, &[]), None).await;
    assert_eq!(running.gateway.certificate().unwrap().fingerprint, fp);
    // The launcher pins the invite's fp before it sends a byte
    let options = Options {
        pin: Some(&fp),
        ..Options::default()
    };
    let reply = https_request(running.port, "/party/invites", options).await.unwrap();
    assert_eq!((reply.status, reply.fingerprint.as_str()), (200, fp.as_str()));
    let wrong = "0".repeat(64);
    let refused = Options {
        pin: Some(&wrong),
        ..Options::default()
    };
    assert!(https_request(running.port, "/party/invites", refused).await.is_err());
    running.close().await;
}

#[test]
fn make_cert_reads_the_fingerprint_and_never_replaces_a_certificate_without_force() {
    let cert = node_cert();
    let before = (
        std::fs::read(&cert.cert_file).unwrap(),
        std::fs::read(&cert.key_file).unwrap(),
    );
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let out = cert.dir.path().to_string_lossy().into_owned();
    // An install that already has its pair: refused, both files untouched
    assert_eq!(cert::make_cert_main(&args(&["--host", "127.0.0.1", "--out", &out])), 2);
    let cert_file = cert.cert_file.to_string_lossy().into_owned();
    let key_file = cert.key_file.to_string_lossy().into_owned();
    assert_eq!(
        cert::make_cert_main(&args(&[
            "--host",
            "127.0.0.1",
            "--cert",
            &cert_file,
            "--key",
            &key_file
        ])),
        2
    );
    assert_eq!(
        (
            std::fs::read(&cert.cert_file).unwrap(),
            std::fs::read(&cert.key_file).unwrap()
        ),
        before
    );
    assert_eq!(cert::make_cert_main(&args(&["--fingerprint", &cert_file])), 0);

    // A new install: a pair that loads, with the Reforged subject
    let fresh = tempfile::tempdir().unwrap();
    let dir = fresh.path().join("tls").to_string_lossy().into_owned();
    assert_eq!(
        cert::make_cert_main(&args(&["--host", "203.0.113.7", "--out", &dir, "--json"])),
        0
    );
    let made = cert::load(
        &fresh.path().join("tls/gateway-cert.pem"),
        &fresh.path().join("tls/gateway-key.pem"),
    )
    .unwrap();
    assert_ne!(made.info.fingerprint, cert.info.fingerprint);
    let (_, parsed) = x509_parser::parse_x509_certificate(made.chain[0].as_ref()).unwrap();
    let organization = parsed.subject().iter_organization().next().unwrap().as_str().unwrap();
    assert_eq!(organization, "Dauntless Reforged Revival server");
}
