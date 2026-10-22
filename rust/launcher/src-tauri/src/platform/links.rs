//! Which links the launcher may open in the browser (row L35): `src/main/links.ts`. The page never passes
//! a URL: it names an `ExternalTarget`, the backend picks the URL and checks it here, then opens it from
//! Rust with the `open` crate (ShellExecuteW on Windows, xdg-open on Linux; tauri-plugin-opener would bring
//! zbus 5, whose parts are unmaintained). The page cannot open anything itself.

use crate::bridge::types::ExternalTarget;

pub const PROJECT_URL: &str = "https://github.com/mixutin/dauntless-revived";
pub const PROJECT_LICENSE_URL: &str = "https://github.com/mixutin/dauntless-revived/blob/dauntless-revived/LICENSE.txt";
pub const PROJECT_CONTRIBUTORS_URL: &str = "https://github.com/mixutin/dauntless-revived/graphs/contributors";
/// Undaunted, the project this launcher and the whole fork are a modified version of
pub const UPSTREAM_URL: &str = "https://github.com/SyST3MDeV/Undaunted";
pub const UPSTREAM_CONTRIBUTORS_URL: &str = "https://github.com/SyST3MDeV/Undaunted/graphs/contributors";
pub const TAILSCALE_DOWNLOAD_URL: &str = "https://tailscale.com/download";
pub const VC_REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";
const DIRECTX_URL: &str = "https://www.microsoft.com/en-us/download/details.aspx?id=35";
const EUGAMEHOST_URL: &str = "https://www.eugamehost.com/";
const EUGAMEHOST_GAME1_URL: &str =
    "https://www.eugamehost.com/clients/cart.php?a=add&pid=238&promocode=SIGNUP6MONTH&skipconfig=1";
const EUGAMEHOST_GAME2_URL: &str =
    "https://www.eugamehost.com/clients/cart.php?a=add&pid=239&promocode=SIGNUP6MONTH&skipconfig=1";
const EUGAMEHOST_GAME3_URL: &str =
    "https://www.eugamehost.com/clients/cart.php?a=add&pid=240&promocode=SIGNUP6MONTH&skipconfig=1";
const EUGAMEHOST_GAME5_URL: &str =
    "https://www.eugamehost.com/clients/cart.php?a=add&pid=242&promocode=SIGNUP6MONTH&skipconfig=1";
const EUGAMEHOST_5800X_URL: &str = "https://www.eugamehost.com/clients/cart.php?a=add&pid=500";
const DISCORD_URL: &str = "https://discord.gg/dauntlessrevived";
const PATREON_URL: &str = "https://patreon.com/DauntlessRevived";

/// The URL of a target that always opens one fixed page; None for the two that come from the server
/// side (the Tailscale share link of a private invite, the host's own source link).
pub fn fixed_link_url(target: ExternalTarget) -> Option<&'static str> {
    use ExternalTarget::*;
    Some(match target {
        TailscaleDownload => TAILSCALE_DOWNLOAD_URL,
        VcRedist => VC_REDIST_URL,
        DirectxRuntime => DIRECTX_URL,
        ProjectSource => PROJECT_URL,
        ProjectLicense => PROJECT_LICENSE_URL,
        ProjectContributors => PROJECT_CONTRIBUTORS_URL,
        UpstreamSource => UPSTREAM_URL,
        UpstreamContributors => UPSTREAM_CONTRIBUTORS_URL,
        Eugamehost => EUGAMEHOST_URL,
        EugamehostGame1 => EUGAMEHOST_GAME1_URL,
        EugamehostGame2 => EUGAMEHOST_GAME2_URL,
        EugamehostGame3 => EUGAMEHOST_GAME3_URL,
        EugamehostGame5 => EUGAMEHOST_GAME5_URL,
        Eugamehost5800x => EUGAMEHOST_5800X_URL,
        Discord => DISCORD_URL,
        Patreon => PATREON_URL,
        TailscaleShare | ServerSource => return None,
    })
}

/// Plain https without user info; an explicit port only where the caller allows one.
fn parse(url: &str, allow_port: bool) -> Option<url::Url> {
    if crate::core::text::js_length(url) > 1024 {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    let clean = parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && (allow_port || parsed.port().is_none());
    clean.then_some(parsed)
}

pub fn is_allowed_static_url(url: &str) -> bool {
    let Some(parsed) = parse(url, false) else { return false };
    match parsed.host_str() {
        Some("www.microsoft.com") => url == DIRECTX_URL,
        Some("tailscale.com" | "login.tailscale.com") => true,
        Some("aka.ms") => parsed.path().starts_with("/vs/"),
        // On github.com only these exact pages: the project's and upstream Undaunted's
        Some("github.com") => [
            PROJECT_URL,
            PROJECT_LICENSE_URL,
            PROJECT_CONTRIBUTORS_URL,
            UPSTREAM_URL,
            UPSTREAM_CONTRIBUTORS_URL,
        ]
        .contains(&url),
        Some("www.eugamehost.com") => [
            EUGAMEHOST_URL,
            EUGAMEHOST_GAME1_URL,
            EUGAMEHOST_GAME2_URL,
            EUGAMEHOST_GAME3_URL,
            EUGAMEHOST_GAME5_URL,
            EUGAMEHOST_5800X_URL,
        ]
        .contains(&url),
        Some("discord.gg") => url == DISCORD_URL,
        Some("patreon.com") => url == PATREON_URL,
        _ => false,
    }
}

/// The host's own source link (ServerStatus.sourceUrl) is allowed as itself while it is plain https,
/// with an explicit port too: the status check keeps such a link, and its Source button must work.
pub fn is_allowed_external_url(url: &str, host_source_url: Option<&str>) -> bool {
    is_allowed_static_url(url) || host_source_url.is_some_and(|source| url == source && parse(url, true).is_some())
}

/// Opens a checked URL in the player's browser.
pub fn open(url: &str, host_source_url: Option<&str>) -> bool {
    is_allowed_external_url(url, host_source_url) && open::that_detached(url).is_ok()
}

/// Opens the game folder in the file manager.
pub fn open_folder(dir: &std::path::Path) -> bool {
    dir.is_dir() && open::that_detached(dir).is_ok()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn the_shared_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/links.json");
        let cases: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for (name, url) in cases["fixed"].as_object().unwrap() {
            let target: ExternalTarget = serde_json::from_value(Value::from(name.as_str())).unwrap();
            assert_eq!(fixed_link_url(target), url.as_str(), "{name}");
        }
        for case in cases["names"].as_array().unwrap() {
            let target = serde_json::from_value::<ExternalTarget>(case["name"].clone()).ok();
            assert_eq!(target.and_then(fixed_link_url), case["url"].as_str(), "{case}");
        }
        let hosts: Vec<Option<&str>> = cases["hosts"].as_array().unwrap().iter().map(Value::as_str).collect();
        for case in cases["urls"].as_array().unwrap() {
            let url = case["url"].as_str().unwrap();
            assert_eq!(is_allowed_static_url(url), case["static"].as_bool().unwrap(), "{url}");
            for (host, expected) in hosts.iter().zip(case["external"].as_array().unwrap()) {
                assert_eq!(
                    is_allowed_external_url(url, *host),
                    expected.as_bool().unwrap(),
                    "{url} with {host:?}"
                );
            }
        }
    }
}
