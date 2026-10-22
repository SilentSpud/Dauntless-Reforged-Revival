//! The allowlist helper's untrusted input (MIGRATION_PLAN.md RS4, RX3; docs/security/allowlist.md):
//! the `/allow` body and its `Content-Length`, the saved state file, and the settings that reach a
//! firewall script. None of it may panic, and nothing the checks accept may change the shape of the
//! nftables script or the Windows rule: an accepted address is exactly one element, an accepted
//! port range, table or interface name is one token.
#![no_main]

use gateway::allowlist::firewall::{RulePlan, WindowsChange, plan, windows_change};
use gateway::allowlist::helper::{allow_body, declared_length};
use gateway::allowlist::nft::{NftSettings, check_interface, check_nft_table, nft_script};
use gateway::allowlist::ports::check_ports;
use gateway::allowlist::state::AllowlistState;
use gateway::ip::{AllowlistCheck, check_allowlist_ip};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// The lines every script has, whatever the addresses: 3 table lines, 2 sets of 3, 8 chain lines.
const SCRIPT_LINES: usize = 17;

fn token(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.@-".contains(&b))
}

fn script_holds(ips: &[String], ports: &str, settings: &NftSettings) {
    let Ok(rule) = plan(ips, ports) else {
        return;
    };
    let Ok(script) = nft_script(&rule, settings) else {
        return;
    };
    let element_line = |line: &&str| line.starts_with("\t\telements = { ");
    let elements = script.lines().filter(element_line).count();
    let v4 = ips.iter().any(|ip| !ip.contains(':'));
    let v6 = ips.iter().any(|ip| ip.contains(':'));
    assert_eq!(script.lines().count(), SCRIPT_LINES + elements);
    assert_eq!(elements, usize::from(v4) + usize::from(v6));
    for line in script.lines().filter(element_line) {
        let inside = line.trim().trim_start_matches("elements = { ").trim_end_matches(" }");
        assert!(
            inside.split(", ").all(|ip| ips.iter().any(|given| given == ip)),
            "{line}"
        );
    }
    if let WindowsChange::Open(rule) = windows_change(&rule) {
        assert_eq!(rule.remote_addresses.split(',').count(), ips.len());
    }
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    // The request: Content-Length as Node reads it, and the body's ip
    let _ = declared_length(Some(&text));
    let mut ips = Vec::new();
    if let Some(ip) = allow_body(data) {
        for allow_private in [false, true] {
            if let AllowlistCheck::Ok(parsed) = check_allowlist_ip(ip.as_ref().and_then(Value::as_str), allow_private) {
                // What the helper keeps is canonical, so it always passes the script's own check
                assert!(plan(std::slice::from_ref(&parsed.canonical), "8770-8777").is_ok());
                ips.push(parsed.canonical);
            }
        }
    }
    for part in text.split(['\n', ' ', ',']) {
        if let AllowlistCheck::Ok(parsed) = check_allowlist_ip(Some(part), true) {
            ips.push(parsed.canonical);
        }
    }

    // The saved state file: whatever it holds, only allowed addresses come back, at most the limit
    if let Ok(saved) = serde_json::from_slice::<Value>(data) {
        let mut state = AllowlistState::new(600_000, 4);
        let loaded = state.load(&saved["entries"], 1_800_000_000_000, |_| true);
        assert!(loaded <= 4 && state.len() <= 4);
        for ip in state.list() {
            assert!(plan(std::slice::from_ref(&ip), "8777").is_ok(), "{ip}");
        }
    }

    // Settings that reach a script: each accepted one is a single token
    let ports = check_ports(&text).ok();
    if let Some(ports) = &ports {
        assert!(ports.bytes().all(|b| b.is_ascii_digit() || b == b'-'));
    }
    let mut settings = NftSettings {
        binary: "/usr/sbin/nft".into(),
        table: "reforged_revival".into(),
        interface: None,
    };
    if let Ok(table) = check_nft_table(&text) {
        assert!(token(&table));
        settings.table = table;
    }
    if let Ok(interface) = check_interface(&text) {
        assert!(token(&interface));
        settings.interface = Some(interface);
    }
    script_holds(&ips, ports.as_deref().unwrap_or("8770-8777"), &settings);
    // A plan built by hand still goes through the same checks
    let _ = nft_script(
        &RulePlan {
            ips: vec![text.to_string()],
            ports: text.to_string(),
        },
        &settings,
    )
    .map(|script| assert!(!script.contains(&*text) || token(&text)));
});
