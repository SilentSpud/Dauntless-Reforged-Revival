//! Which links the launcher may open in the browser (row L35): Reforged's `src/main/links.ts`. The page
//! never passes a URL: it names an `ExternalTarget`, the backend picks the URL and checks it here, then
//! opens it from Rust with the `open` crate (ShellExecuteW on Windows, xdg-open on Linux;
//! tauri-plugin-opener would bring zbus 5, whose parts are unmaintained). The page cannot open anything
//! itself.
//!
//! RA2 moved the table to Reforged's targets with the bridge's `ExternalTarget`; RA11 owns the rest.

use crate::bridge::types::ExternalTarget;

pub const PROJECT_URL: &str = "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival";
pub const PROJECT_LICENSE_URL: &str =
    "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/blob/main/LICENSE.txt";
const PROJECT_ISSUES_URL: &str = "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/issues";
const PROJECT_DOCS_PLAYERS_URL: &str =
    "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/blob/main/docs/players.md";
const PROJECT_DOCS_HOSTING_URL: &str =
    "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/blob/main/docs/hosting.md";
const PROJECT_DOCS_SDK_URL: &str =
    "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/blob/main/docs/sdk-and-game-data.md";
// The projects this one is built from
const REVIVED_URL: &str = "https://github.com/mixutin/dauntless-revived";
const MYSTIC_URL: &str = "https://github.com/pranav158/Mystic-Paradox";
const UNDAUNTED_URL: &str = "https://github.com/SyST3MDeV/Undaunted";
const INSTALLING_GUIDE_URL: &str = "https://github.com/pranav158/Mystic-Paradox/blob/main/docs/INSTALLING_DAUNTLESS.md";
const LEGENDARY_URL: &str = "https://github.com/legendary-gl/legendary/releases/latest";
const DUMPER7_URL: &str = "https://github.com/Encryqed/Dumper-7";
const VISUAL_STUDIO_URL: &str = "https://visualstudio.microsoft.com/downloads/";
const RUST_URL: &str = "https://www.rust-lang.org/tools/install";
const NODEJS_URL: &str = "https://nodejs.org/en/download";
const MONGODB_URL: &str = "https://www.mongodb.com/try/download/community";
pub const TAILSCALE_DOWNLOAD_URL: &str = "https://tailscale.com/download";
// Direct downloads for the Host page's Requirements tab: each starts the official installer, or the
// source archive (Dumper-7 is built from source)
const GIT_URL: &str = "https://git-scm.com/download/win";
const VISUAL_STUDIO_INSTALLER_URL: &str = "https://aka.ms/vs/17/release/vs_community.exe";
const RUST_INSTALLER_URL: &str = "https://win.rustup.rs/x86_64";
const DUMPER7_ZIP_URL: &str = "https://github.com/Encryqed/Dumper-7/archive/refs/heads/main.zip";
pub const VC_REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";

/// The URL of a target that always opens one fixed page; None for the two that come from the server
/// side (the Tailscale share link of a private invite, the host's own source link).
pub fn fixed_link_url(target: ExternalTarget) -> Option<&'static str> {
    use ExternalTarget::*;
    Some(match target {
        TailscaleDownload => TAILSCALE_DOWNLOAD_URL,
        VcRedist => VC_REDIST_URL,
        ProjectSource => PROJECT_URL,
        ProjectLicense => PROJECT_LICENSE_URL,
        ProjectIssues => PROJECT_ISSUES_URL,
        ProjectDocsPlayers => PROJECT_DOCS_PLAYERS_URL,
        ProjectDocsHosting => PROJECT_DOCS_HOSTING_URL,
        ProjectDocsSdk => PROJECT_DOCS_SDK_URL,
        InstallingGuide => INSTALLING_GUIDE_URL,
        Legendary => LEGENDARY_URL,
        Dumper7 => DUMPER7_URL,
        VisualStudio => VISUAL_STUDIO_URL,
        Rust => RUST_URL,
        Nodejs => NODEJS_URL,
        Mongodb => MONGODB_URL,
        Git => GIT_URL,
        VisualStudioInstaller => VISUAL_STUDIO_INSTALLER_URL,
        RustInstaller => RUST_INSTALLER_URL,
        Dumper7Zip => DUMPER7_ZIP_URL,
        RevivedSource => REVIVED_URL,
        MysticSource => MYSTIC_URL,
        UndauntedSource => UNDAUNTED_URL,
        TailscaleShare | ServerSource => return None,
    })
}

const FIXED_URLS: [&str; 22] = [
    TAILSCALE_DOWNLOAD_URL,
    VC_REDIST_URL,
    PROJECT_URL,
    PROJECT_LICENSE_URL,
    PROJECT_ISSUES_URL,
    PROJECT_DOCS_PLAYERS_URL,
    PROJECT_DOCS_HOSTING_URL,
    PROJECT_DOCS_SDK_URL,
    INSTALLING_GUIDE_URL,
    LEGENDARY_URL,
    DUMPER7_URL,
    VISUAL_STUDIO_URL,
    RUST_URL,
    NODEJS_URL,
    MONGODB_URL,
    GIT_URL,
    VISUAL_STUDIO_INSTALLER_URL,
    RUST_INSTALLER_URL,
    DUMPER7_ZIP_URL,
    REVIVED_URL,
    MYSTIC_URL,
    UNDAUNTED_URL,
];

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

/// A fixed link, or a private invite's Tailscale share link.
pub fn is_allowed_static_url(url: &str) -> bool {
    let Some(parsed) = parse(url, false) else { return false };
    FIXED_URLS.contains(&url) || parsed.host_str() == Some("login.tailscale.com")
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
    use super::*;
    use crate::bridge::spec::EXTERNAL_TARGETS;

    /// `links.test.ts`: "every fixed link is plain https"
    #[test]
    fn every_fixed_link_is_plain_https() {
        let mut fixed = 0;
        for name in EXTERNAL_TARGETS {
            let target: ExternalTarget = serde_json::from_value(serde_json::Value::from(*name)).unwrap();
            let Some(url) = fixed_link_url(target) else {
                assert!(["tailscale_share", "server_source"].contains(name), "{name}");
                continue;
            };
            fixed += 1;
            let parsed = url::Url::parse(url).unwrap();
            assert_eq!(parsed.scheme(), "https", "{name}");
            assert!(parsed.username().is_empty() && parsed.password().is_none(), "{name}");
            assert!(is_allowed_static_url(url), "{name}");
        }
        assert_eq!(fixed, FIXED_URLS.len());
    }

    /// `links.test.ts`: "external URLs: fixed ones, Tailscale share links and the host's own https source
    /// link only"
    #[test]
    fn external_urls() {
        assert!(is_allowed_static_url("https://login.tailscale.com/admin/invite/abc"));
        assert!(!is_allowed_static_url("http://login.tailscale.com/admin/invite/abc"));
        assert!(!is_allowed_static_url("https://evil.example/"));
        let source = Some("https://git.example/me/server");
        assert!(is_allowed_external_url("https://git.example/me/server", source));
        assert!(!is_allowed_external_url("https://git.example/me/other", source));
        assert!(!is_allowed_external_url(
            "javascript:alert(1)",
            Some("javascript:alert(1)")
        ));
        assert!(!is_allowed_external_url(
            "https://user:pw@git.example/",
            Some("https://user:pw@git.example/")
        ));
        assert!(is_allowed_external_url(
            "https://git.example:8443/me",
            Some("https://git.example:8443/me")
        ));
        assert!(!is_allowed_static_url(
            "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival/x"
        ));
    }
}
