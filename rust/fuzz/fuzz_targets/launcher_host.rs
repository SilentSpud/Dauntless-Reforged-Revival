//! What the launcher's Host page reads from text it did not write (MIGRATION_PLAN.md RA12, RX3): the
//! output of `reforged-host` (its `Error:` lines, the `--json` lines of `invite` and `update`, the
//! published runtime's version, the pack it wrote), the workspace's `host.toml` (or a Node kit's
//! `host.json`) and `vswhere`'s answers. The invite's link is a secret the page copies, and the
//! multipliers go back to the host kit as arguments. The launcher's own parser file, compiled in here, since the launcher crate needs a webview
//! to build.
#![no_main]
#![allow(dead_code)]

use libfuzzer_sys::fuzz_target;

#[path = "../../launcher/src-tauri/src/platform/hostkit/parse.rs"]
mod parse;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();

    for line in &lines {
        let clean = parse::clean_line(line);
        assert!(!clean.contains('\r') && clean.chars().count() <= 400);
    }
    if let Some(reason) = parse::failure_line(&lines) {
        assert!(reason.chars().count() <= 300);
    }
    if let Some(invite) = parse::invite_line(&lines) {
        // Only a reforged-revival link, short, on one line, reaches the page's copy button
        assert!(invite.link.starts_with("reforged-revival://"));
        assert!(invite.link.len() <= 4096 && !invite.link.chars().any(char::is_control));
        if let Some(name) = &invite.for_name {
            assert!(!name.is_empty() && name.chars().count() <= 40 && !name.chars().any(char::is_control));
        }
    }
    let json = parse::last_json(&lines);
    if let Some(json) = &json {
        if let Some(check) = parse::update_check(json) {
            assert!(check.head.chars().count() <= 40 && !check.head.chars().any(char::is_control));
        }
        if let Some(done) = parse::update_done(json) {
            assert!(done.to.chars().count() <= 40 && !done.to.chars().any(char::is_control));
        }
    }
    let _ = parse::update_failure(json.as_ref(), lines.first().map(String::as_str));
    if let Some(version) = parse::published_version(&lines) {
        assert!(
            version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        );
    }
    if let Some(pack) = parse::pack_written(&lines) {
        // Shown as a file name only
        assert!(!pack.contains(['\\', '/']) && pack.chars().count() <= 120);
    }

    // host.toml and a Node kit's host.json
    for config in [parse::server_config(&text), parse::server_config_toml(&text)]
        .into_iter()
        .flatten()
    {
        if let Some((xp, rewards)) = config.multipliers {
            // What goes back to `reforged-host multipliers` as arguments
            for n in [xp, rewards] {
                assert!(n.is_finite() && (1.0..=parse::MAX_MULTIPLIER).contains(&n));
            }
            assert!(config.mode == parse::ServerMode::Private);
        }
        assert!(config.name.as_ref().is_none_or(|name| name.chars().count() <= 80));
        assert!(config.host.as_ref().is_none_or(|host| host.chars().count() <= 255));
    }
    if let Some((name, _)) = parse::vswhere_install(&text) {
        assert!(name.chars().count() <= 80);
    }
    let _ = parse::vswhere_msbuild(&text);
    let _ = parse::cut(&text, 300);
});
