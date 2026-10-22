//! Where everything lives (`legacy/ReforgedHost/src/paths.ts`). The repository holds the code;
//! `.workspace/` (git-ignored) holds what is the host's own: the pinned upstream source with the
//! patches applied, the SDK generated from their game, game data, builds, configuration with
//! secrets, certificates, the database and logs.
//!
//! The layout is the Node kit's, so a host set up with it keeps working. RH5 moves the code side
//! (the services, the patches) to the server release; `.workspace/` stays where it is.

use std::path::{Path, PathBuf};

use crate::error::HostError;

pub const RUNTIME_FILE_NAMES: [&str; 2] = ["MysticParadox.dll", "winmm.dll"];
pub const GAME_CHANGELIST: u32 = 647472;
pub const GAME_VERSION_TEXT: &str = "dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472";
pub const PINNED_EXE_SHA256: &str = "f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b";

/// `Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe`, with this platform's separator.
pub fn game_exe_relative() -> PathBuf {
    ["Archon", "Binaries", "Win64", "Dauntless-Win64-Shipping.exe"]
        .iter()
        .collect()
}

pub struct Ports {
    pub backend: u16,
    pub director: u16,
    pub content: u16,
    pub allowlist: u16,
    pub public_gateway: u16,
    pub private_gateway: u16,
    pub relay: u16,
    pub mongo: u16,
    /// The game servers' UDP ports: Ramsgate on `game_last`, the Training Grounds on the one below,
    /// hunts below that. Up to 6 hunts they are 8770-8777, as always; more hunts reach further down
    /// ([`game_port_range`]).
    pub game_first: u16,
    pub game_last: u16,
    pub game_lowest: u16,
}

pub const PORTS: Ports = Ports {
    backend: 61000,
    director: 61001,
    content: 61002,
    allowlist: 61005,
    public_gateway: 443,
    private_gateway: 61000,
    relay: 61100,
    mongo: 27017,
    game_first: 8770,
    game_last: 8777,
    game_lowest: 8766,
};

/// Hunts at once: 1 to `MAX_HUNTS` (the ports from `game_lowest` to `game_last`).
pub const MAX_HUNTS: u32 = (PORTS.game_last - PORTS.game_lowest - 1) as u32;

/// The UDP ports the game servers use with this many hunts at once: 8770-8777 up to 6 (unchanged, so
/// a firewall opened for them keeps working), one more port below 8770 for each hunt above 6
/// (8766-8777 at 10). Returns `(first, last)`.
pub fn game_port_range(max_hunts: u32) -> (u16, u16) {
    let hunts = max_hunts.clamp(1, MAX_HUNTS) as u16;
    (PORTS.game_first.min(PORTS.game_last - 1 - hunts), PORTS.game_last)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub repo: PathBuf,
    /// The project's server preset (RH4), committed with the project.
    pub preset: PathBuf,
    pub host_kit: PathBuf,
    pub upstream_pin: PathBuf,
    pub patches_dir: PathBuf,
    pub gateway_dir: PathBuf,
    pub content_dir: PathBuf,
    pub workspace: PathBuf,
    pub upstream: PathBuf,
    pub marker: PathBuf,
    pub backend_dir: PathBuf,
    pub director_dir: PathBuf,
    pub runtime_source: PathBuf,
    pub sdk_header: PathBuf,
    pub sdk_dir: PathBuf,
    pub runtime_dll: PathBuf,
    pub loader_dir: PathBuf,
    pub loader_dll: PathBuf,
    pub exporter_dll: PathBuf,
    pub backend_game_data: PathBuf,
    pub director_game_data: PathBuf,
    pub raw_game_data: PathBuf,
    pub exports: PathBuf,
    pub store_captures: PathBuf,
    pub captures_pin: PathBuf,
    pub runtime_publish: PathBuf,
    pub config: PathBuf,
    pub host_json: PathBuf,
    pub tls: PathBuf,
    pub cert_file: PathBuf,
    pub key_file: PathBuf,
    pub mongo_data: PathBuf,
    pub logs: PathBuf,
    pub gameserver_logs: PathBuf,
    pub branding: PathBuf,
    pub news_file: PathBuf,
    /// Linux: what runs the Windows game servers (`wine`) and the systemd units.
    pub bin: PathBuf,
    pub gameserver_wrapper: PathBuf,
    pub wine_prefix: PathBuf,
    pub proton_data: PathBuf,
    pub systemd: PathBuf,
}

impl Paths {
    /// `root`: the Dauntless Reforged Revival checkout (the folder with `legacy/ReforgedHost/` and `upstream/`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let workspace = root.join(".workspace");
        let upstream = workspace.join("Mystic-Paradox");
        let runtime_source = upstream.join("ParadoxRuntime");
        let config = workspace.join("config");
        let tls = workspace.join("tls");
        let logs = workspace.join("logs");
        Self {
            preset: root.join("preset"),
            host_kit: root.join("legacy").join("ReforgedHost"),
            upstream_pin: root.join("upstream").join("mystic-paradox.json"),
            patches_dir: root.join("upstream"),
            gateway_dir: root.join("legacy").join("ReforgedGateway"),
            content_dir: root.join("legacy").join("ReforgedContent"),
            marker: upstream.join(".reforged-patches.json"),
            backend_dir: upstream.join("ParadoxBackend"),
            director_dir: upstream.join("ParadoxDirector"),
            sdk_header: runtime_source.join("SDK.hpp"),
            sdk_dir: runtime_source.join("SDK"),
            runtime_dll: runtime_source.join("x64").join("Release").join("MysticParadox.dll"),
            loader_dir: upstream.join("tools").join("RuntimeLoader"),
            loader_dll: join(&upstream, &["tools", "RuntimeLoader", "target", "release", "winmm.dll"]),
            exporter_dll: join(
                &upstream,
                &["tools", "CatalogExporter", "x64", "Release", "CatalogExporter.dll"],
            ),
            backend_game_data: upstream.join("ParadoxBackend").join("game-data"),
            director_game_data: upstream.join("ParadoxDirector").join("game-data"),
            raw_game_data: join(&upstream, &["ParadoxBackend", "game-data", "raw"]),
            exports: upstream.join("Items_Analysis"),
            store_captures: upstream.join("DauntlessEndpointDocumentation"),
            captures_pin: root.join("upstream").join("store-captures.json"),
            runtime_publish: workspace.join("runtime"),
            host_json: config.join("host.json"),
            cert_file: tls.join("gateway-cert.pem"),
            key_file: tls.join("gateway-key.pem"),
            mongo_data: workspace.join("mongodb"),
            gameserver_logs: logs.join("gameservers"),
            branding: workspace.join("branding"),
            news_file: config.join("news.json"),
            bin: workspace.join("bin"),
            gameserver_wrapper: workspace.join("bin").join("reforged-gameserver"),
            wine_prefix: workspace.join("wine"),
            proton_data: workspace.join("proton"),
            systemd: config.join("systemd"),
            repo: root,
            workspace,
            upstream,
            runtime_source,
            config,
            tls,
            logs,
        }
    }
}

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base.to_path_buf(), |path, part| path.join(part))
}

/// The repository root: the first folder at or above `start` (at most 8 levels) that holds
/// `upstream/mystic-paradox.json` and `legacy/ReforgedHost/`.
pub fn find_repo_root(start: &Path) -> Result<PathBuf, HostError> {
    let mut dir = start;
    for _ in 0..8 {
        if dir.join("upstream").join("mystic-paradox.json").exists() && dir.join("legacy").join("ReforgedHost").exists()
        {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(up) => dir = up,
            None => break,
        }
    }
    Err(HostError::message(format!(
        "Could not find the Dauntless Reforged Revival folder (with upstream/ and legacy/ReforgedHost/) above {}",
        start.display()
    )))
}

/// The root for this run: searched from the binary's folder (a build inside the checkout, as the
/// Node kit searched from its `dist/`), then from the working directory.
pub fn locate_repo_root() -> Result<PathBuf, HostError> {
    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .map(|dir| find_repo_root(&dir));
    match from_exe {
        Some(Ok(root)) => Ok(root),
        _ => {
            let cwd = std::env::current_dir().map_err(|e| HostError::io(".", e))?;
            find_repo_root(&cwd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_udp_8770_8777_up_to_6_hunts_and_reaches_one_port_further_down_per_hunt_above() {
        assert_eq!(game_port_range(3), (8770, 8777));
        assert_eq!(game_port_range(6), (8770, 8777));
        assert_eq!(game_port_range(8), (8768, 8777));
        assert_eq!(game_port_range(10), (8766, 8777));
        assert_eq!(game_port_range(0), (8770, 8777), "Math.trunc(0) || 1");
        assert_eq!(game_port_range(50), (8766, 8777), "capped at MAX_HUNTS");
        assert_eq!(MAX_HUNTS, 10);
    }

    #[test]
    fn lays_out_the_workspace_as_the_node_kit() {
        let root = PathBuf::from("rr");
        let p = Paths::new(&root);
        let ws = root.join(".workspace");
        assert_eq!(p.host_json, ws.join("config").join("host.json"));
        assert_eq!(p.cert_file, ws.join("tls").join("gateway-cert.pem"));
        assert_eq!(p.backend_dir, ws.join("Mystic-Paradox").join("ParadoxBackend"));
        assert_eq!(
            p.runtime_dll,
            ws.join("Mystic-Paradox")
                .join("ParadoxRuntime")
                .join("x64")
                .join("Release")
                .join("MysticParadox.dll")
        );
        assert_eq!(p.gameserver_wrapper, ws.join("bin").join("reforged-gameserver"));
        assert_eq!(p.systemd, ws.join("config").join("systemd"));
        assert_eq!(p.host_kit, root.join("legacy").join("ReforgedHost"));
        assert_eq!(p.upstream_pin, root.join("upstream").join("mystic-paradox.json"));
        assert!(game_exe_relative().ends_with("Dauntless-Win64-Shipping.exe"));
        assert!(game_exe_relative().starts_with("Archon"));
    }

    #[test]
    fn finds_the_checkout_above_a_folder_and_names_the_start_when_there_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Reforged");
        std::fs::create_dir_all(root.join("upstream")).unwrap();
        std::fs::create_dir_all(root.join("legacy").join("ReforgedHost").join("dist")).unwrap();
        std::fs::write(root.join("upstream").join("mystic-paradox.json"), "{}").unwrap();
        let deep = root.join("legacy").join("ReforgedHost").join("dist");
        assert_eq!(find_repo_root(&deep).unwrap(), root);
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let error = find_repo_root(&elsewhere).unwrap_err().to_string();
        assert!(
            error.starts_with("Could not find the Dauntless Reforged Revival folder"),
            "{error}"
        );
    }
}
