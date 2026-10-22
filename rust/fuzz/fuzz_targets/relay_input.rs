//! What the launcher's local relay reads before it forwards anything (MIGRATION_PLAN.md RA6, RX3, RD21): a
//! local request's peer address, Host and Origin headers and request target, the port override, and the
//! TCP table the Windows owner check reads. The relay's own files, compiled in here, since the launcher
//! crate needs a webview to build. HTTP framing itself is hyper's.
#![no_main]
#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[path = "../../launcher/src-tauri/src/net/relay/local.rs"]
mod local;

#[path = "../../launcher/src-tauri/src/net/relay_owner/table.rs"]
mod table;

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    v6: bool,
    ip: [u8; 16],
    host: Option<&'a str>,
    origin: Option<Option<&'a str>>,
    fetch_metadata: bool,
    upgrade: bool,
    target: &'a str,
    port_override: &'a str,
    local_port: u16,
    remote_port: u16,
    table: &'a str,
}

fuzz_target!(|input: Input<'_>| {
    let ip = if input.v6 {
        IpAddr::V6(Ipv6Addr::from(input.ip))
    } else {
        IpAddr::V4(Ipv4Addr::new(input.ip[0], input.ip[1], input.ip[2], input.ip[3]))
    };
    let origin = match input.origin {
        None => local::Origin::Absent,
        Some(Some(text)) => local::Origin::Text(text),
        Some(None) => local::Origin::Unreadable,
    };
    let problem = local::request_problem(ip, input.host, origin, input.fetch_metadata, input.upgrade);
    if problem.is_none() {
        // A request that passes comes from this PC, names this PC, and carries no web page's marks
        let mapped = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            ip => ip,
        };
        assert!(mapped.is_loopback());
        let host = input.host.expect("a Host header");
        assert!(local::is_loopback_name(local::host_part(host)));
        assert!(input.upgrade || !input.fetch_metadata);
        match origin {
            local::Origin::Absent => {}
            local::Origin::Text(text) => {
                assert!(local::origin_host(text).is_some_and(local::is_loopback_name))
            }
            local::Origin::Unreadable => panic!("an unreadable Origin passed"),
        }
    }
    if local::is_qos_path(input.target) {
        assert!(input.target.len() >= 4 && input.target[..4].eq_ignore_ascii_case("/qos"));
    }
    if let Some(port) = local::relay_port_override(Some(input.port_override)) {
        assert!(port >= 1024);
    }

    // The owner check passes only for the one client row with exactly the game's pid
    let local = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), input.local_port);
    let remote = SocketAddr::new(ip, input.remote_port);
    if let Some(owner) = table::peer_pid(input.table, local, remote)
        && owner >= 0
    {
        let client = table::endpoint(remote).expect("a loopback client");
        let server = table::endpoint(local).expect("a loopback server");
        let rows = input
            .table
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .filter(|row| row.len() == 5 && row[0] == "TCP" && row[1] == client && row[2] == server)
            .count();
        assert_eq!(rows, 1);
    }
});
