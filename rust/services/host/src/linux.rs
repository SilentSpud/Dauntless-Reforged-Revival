//! Linux hosts (`legacy/ReforgedHost/src/linux.ts`): privileges, the systemd units, the ufw script
//! and the Wine prefix.
//!
//! What a Linux server needs beyond the Windows host kit, on the server release (RD8, RH5):
//! - the launcher the Director starts for each game server, `.workspace/bin/reforged-gameserver`,
//!   which runs the hidden `reforged-host gameserver` ([`crate::wine`])
//! - privileges: a public server listens on TCP 443 and its allowlist changes nftables
//! - systemd units that give each part only the privilege it needs, and run the server at boot:
//!   `reforged-host start`, and in public mode `reforged-host allowlist`
//! - `firewall.sh`: lets a host firewall (ufw) pass the server's traffic

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::HostError;
use crate::hostconfig::{HostConfig, LinuxOptions, ServerMode, game_ports_text, gateway_port};
use crate::paths::{PORTS, Paths};
use crate::run::{RunOptions, run};
use crate::wine::{dll_overrides, find_on_path, find_wine, wine_prefix_ready};

pub const UNIT_SERVER: &str = "reforged.service";
pub const UNIT_ALLOWLIST: &str = "reforged-allowlist.service";

pub const CAP_NET_BIND_SERVICE: u32 = 10;
pub const CAP_NET_ADMIN: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Wine,
    Proton,
}

/// What runs the game servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRuntime {
    pub kind: RuntimeKind,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinePrefix {
    pub ok: bool,
    pub created: bool,
    pub detail: Option<String>,
}

// ------------------------------------------------------------------ privileges

fn read_text(file: &str) -> Option<String> {
    fs::read_to_string(file).ok()
}

/// `CapEff` from `/proc/<pid>/status`: the capabilities this process has now.
pub fn effective_capabilities(status_text: &str) -> u64 {
    status_text
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        .unwrap_or(0)
}

pub fn has_capability(bit: u32, status_text: Option<&str>) -> bool {
    status_text.is_some_and(|s| effective_capabilities(s) & (1u64 << bit) != 0)
}

/// The effective user id, from `Uid:` in `/proc/self/status` (real, effective, saved, filesystem).
fn effective_uid(status_text: &str) -> Option<u32> {
    status_text
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1))
        .and_then(|id| id.parse().ok())
}

pub fn is_root() -> Result<bool, HostError> {
    if !cfg!(unix) {
        return Ok(false);
    }
    if let Some(uid) = read_text("/proc/self/status").as_deref().and_then(effective_uid) {
        return Ok(uid == 0);
    }
    Ok(crate::run::capture("id", &["-u"], None)?.is_some_and(|id| id.trim() == "0"))
}

/// Whether a process can listen on `port`: root, `CAP_NET_BIND_SERVICE`, or a system that lets
/// everyone (`net.ipv4.ip_unprivileged_port_start`).
pub fn can_bind_port_with(port: u16, root: bool, status: Option<&str>, unprivileged_start: Option<&str>) -> bool {
    if port >= 1024 || root {
        return true;
    }
    let start = unprivileged_start
        .map(str::trim)
        .unwrap_or("1024")
        .parse::<u32>()
        .unwrap_or(1024);
    if start <= u32::from(port) {
        return true;
    }
    has_capability(CAP_NET_BIND_SERVICE, status)
}

pub fn can_bind_port(port: u16) -> Result<bool, HostError> {
    Ok(can_bind_port_with(
        port,
        is_root()?,
        read_text("/proc/self/status").as_deref(),
        read_text("/proc/sys/net/ipv4/ip_unprivileged_port_start").as_deref(),
    ))
}

pub fn can_change_firewall() -> Result<bool, HostError> {
    if is_root()? {
        return Ok(true);
    }
    Ok(has_capability(CAP_NET_ADMIN, read_text("/proc/self/status").as_deref()))
}

// ------------------------------------------------------------------ the game-server launcher

/// A POSIX shell word: single-quoted, with `'` written as `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The script the Director runs for each game server: `reforged-host gameserver` with its arguments.
pub fn gameserver_wrapper_script(host_bin: &Path) -> String {
    [
        "#!/bin/sh".to_owned(),
        "# Dauntless Reforged Revival: the Director starts this for each game server. Written by `reforged-host setup`.".to_owned(),
        "# It runs Dauntless-Win64-Shipping.exe through Wine or Proton (rust/services/host/src/wine.rs).".to_owned(),
        format!("exec {} gameserver \"$@\"", sh_quote(&host_bin.to_string_lossy())),
        String::new(),
    ]
    .join("\n")
}

/// This `reforged-host`: the one in the server release's `bin/`, or the running one.
fn host_binary(paths: &Paths) -> PathBuf {
    let name = format!("reforged-host{}", std::env::consts::EXE_SUFFIX);
    let release = paths.repo.join("bin").join(&name);
    if release.is_file() {
        return release;
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(name))
}

#[cfg(unix)]
fn set_mode(file: &Path, mode: u32) -> Result<(), HostError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(mode)).map_err(|e| HostError::io(file, e))
}

#[cfg(not(unix))]
fn set_mode(_file: &Path, _mode: u32) -> Result<(), HostError> {
    Ok(())
}

/// Writes `.workspace/bin/reforged-gameserver`; returns its path.
pub fn write_gameserver_wrapper(paths: &Paths) -> Result<PathBuf, HostError> {
    fs::create_dir_all(&paths.bin).map_err(|e| HostError::io(&paths.bin, e))?;
    let target = &paths.gameserver_wrapper;
    let tmp = PathBuf::from(format!("{}.tmp", target.display()));
    fs::write(&tmp, gameserver_wrapper_script(&host_binary(paths))).map_err(|e| HostError::io(&tmp, e))?;
    set_mode(&tmp, 0o755)?;
    fs::rename(&tmp, target).map_err(|e| HostError::io(target, e))?;
    Ok(target.clone())
}

/// What runs the game servers, as setup and doctor describe it. `None` when nothing was found.
pub fn game_runtime(options: &LinuxOptions) -> Result<Option<GameRuntime>, HostError> {
    if let Some(proton) = &options.proton {
        return Ok(Some(GameRuntime {
            kind: RuntimeKind::Proton,
            path: proton.clone(),
        }));
    }
    let wine = match &options.wine {
        Some(wine) => Some(wine.clone()),
        None => find_wine(&path_var())?,
    };
    Ok(wine.map(|path| GameRuntime {
        kind: RuntimeKind::Wine,
        path,
    }))
}

fn path_var() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// The first start of Wine makes its prefix (a minute or more): done once here, so it does not eat
/// into the first game server's readiness timeout. Proton makes its own on its first run.
pub fn ensure_wine_prefix(paths: &Paths, options: &LinuxOptions, log_file: &Path) -> Result<WinePrefix, HostError> {
    if options.proton.is_some() || wine_prefix_ready(&paths.wine_prefix) {
        return Ok(WinePrefix {
            ok: true,
            created: false,
            detail: None,
        });
    }
    let wine = match &options.wine {
        Some(wine) => Some(wine.clone()),
        None => find_wine(&path_var())?,
    };
    let Some(wine) = wine else {
        return Ok(WinePrefix {
            ok: false,
            created: false,
            detail: Some("Wine was not found (apt install wine)".into()),
        });
    };
    fs::create_dir_all(&paths.wine_prefix).map_err(|e| HostError::io(&paths.wine_prefix, e))?;
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k != "WINEPREFIX" && k != "WINEDEBUG" && k != "WINEDLLOVERRIDES")
        .collect();
    env.push(("WINEPREFIX".into(), paths.wine_prefix.to_string_lossy().into_owned()));
    env.push(("WINEDEBUG".into(), "-all".into()));
    env.push((
        "WINEDLLOVERRIDES".into(),
        dll_overrides(std::env::var("WINEDLLOVERRIDES").ok().as_deref()),
    ));
    let no_display = std::env::var("DISPLAY").unwrap_or_default().trim().is_empty();
    let xvfb = if options.xvfb && no_display {
        find_on_path(&["xvfb-run"])?
    } else {
        None
    };
    let (command, args): (String, Vec<&str>) = match &xvfb {
        Some(xvfb) => (xvfb.clone(), vec!["-a", wine.as_str(), "wineboot", "--init"]),
        None => (wine.clone(), vec!["wineboot", "--init"]),
    };
    let options = RunOptions {
        cwd: paths.workspace.clone(),
        env: Some(env),
        log_file: Some(log_file.to_path_buf()),
        quiet: true,
    };
    let r = run(&command, &args, &options)?;
    // wineboot returns before wineserver has written everything: wait for it.
    if let Some(server) = find_on_path(&["wineserver"])? {
        run(&server, &["-w"], &options)?;
    }
    if !wine_prefix_ready(&paths.wine_prefix) {
        let last = r.last_line.map(|l| format!(": {l}")).unwrap_or_default();
        return Ok(WinePrefix {
            ok: false,
            created: false,
            detail: Some(format!(
                "wineboot did not make the prefix (exit {}{last}; see {})",
                r.code,
                log_file.display()
            )),
        });
    }
    Ok(WinePrefix {
        ok: true,
        created: true,
        detail: None,
    })
}

// ------------------------------------------------------------------ systemd

fn no_line_break(s: &str) -> Result<(), HostError> {
    if s.contains(['\n', '\r']) {
        return Err(HostError::message("a path in a systemd unit cannot hold a line break"));
    }
    Ok(())
}

/// One word of an `ExecStart=` line: double-quoted, with systemd's specifiers (`%`) and variables
/// (`$`) escaped.
pub fn unit_word(s: &str) -> Result<String, HostError> {
    no_line_break(s)?;
    Ok(format!(
        "\"{}\"",
        s.replace('\\', r"\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    ))
}

/// A path setting (`WorkingDirectory=`): taken as is, only specifiers (`%`) expanded.
pub fn unit_path(s: &str) -> Result<String, HostError> {
    no_line_break(s)?;
    Ok(s.replace('%', "%%"))
}

/// Who the units run as, and the programs they start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitContext {
    pub user: String,
    pub host_bin: PathBuf,
}

fn user_name() -> Result<String, HostError> {
    if let Some(name) = crate::run::capture("id", &["-un"], None)? {
        let name = name.trim();
        if !name.is_empty() {
            return Ok(name.to_owned());
        }
    }
    Ok(std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default())
}

pub fn unit_context(paths: &Paths) -> Result<UnitContext, HostError> {
    Ok(UnitContext {
        user: user_name()?,
        host_bin: host_binary(paths),
    })
}

/// The units for this server, by file name, in the order they are written. Both run as the user who
/// ran setup, never as root:
/// - `reforged-allowlist` (public): the allowlist helper, with `CAP_NET_ADMIN` only (nftables)
/// - `reforged`: `reforged-host start` (MongoDB, backend, Director and its game servers, content
///   server, gateway), with `CAP_NET_BIND_SERVICE` only in public mode on a port below 1024
pub fn systemd_units(c: &HostConfig, p: &Paths, ctx: &UnitContext) -> Result<Vec<(String, String)>, HostError> {
    if ctx.user == "root" || ctx.user.is_empty() {
        return Err(HostError::message(
            "run setup as the user the server should run as, not as root (the units run as that user)",
        ));
    }
    let public = c.mode == ServerMode::Public;
    let mut files = Vec::new();
    let allowlist_env = p.config.join("allowlist.env");
    let workspace = unit_path(&p.workspace.to_string_lossy())?;
    let host = unit_word(&ctx.host_bin.to_string_lossy())?;
    let env_file = unit_word(&allowlist_env.to_string_lossy())?;
    if public {
        files.push((
            UNIT_ALLOWLIST.to_owned(),
            [
                "# Dauntless Reforged Revival: the game-port allowlist (nftables). Written by `reforged-host setup`."
                    .to_owned(),
                "[Unit]".to_owned(),
                "Description=Dauntless Reforged Revival game-port allowlist (nftables)".to_owned(),
                "After=network-online.target".to_owned(),
                "Wants=network-online.target".to_owned(),
                format!("Before={UNIT_SERVER}"),
                String::new(),
                "[Service]".to_owned(),
                "Type=simple".to_owned(),
                format!("User={}", ctx.user),
                format!("WorkingDirectory={workspace}"),
                format!("ExecStart={host} \"allowlist\" \"--env-file\" {env_file}"),
                "# Whatever happened, the game ports end closed.".to_owned(),
                format!("ExecStopPost={host} \"allowlist\" \"--env-file\" {env_file} \"--close-ports\""),
                "Restart=on-failure".to_owned(),
                "RestartSec=3".to_owned(),
                "TimeoutStopSec=30".to_owned(),
                "AmbientCapabilities=CAP_NET_ADMIN".to_owned(),
                "CapabilityBoundingSet=CAP_NET_ADMIN".to_owned(),
                "NoNewPrivileges=true".to_owned(),
                String::new(),
                "[Install]".to_owned(),
                "WantedBy=multi-user.target".to_owned(),
                String::new(),
            ]
            .join("\n"),
        ));
    }
    let mut server = vec![
        "# Dauntless Reforged Revival: the whole server (reforged-host start). Written by `reforged-host setup`."
            .to_owned(),
        "[Unit]".to_owned(),
        "Description=Dauntless Reforged Revival server".to_owned(),
        format!(
            "After=network-online.target{}",
            if public {
                format!(" {UNIT_ALLOWLIST}")
            } else {
                String::new()
            }
        ),
        "Wants=network-online.target".to_owned(),
    ];
    if public {
        server.push(format!("Requires={UNIT_ALLOWLIST}"));
    }
    server.extend([
        String::new(),
        "[Service]".to_owned(),
        "Type=simple".to_owned(),
        format!("User={}", ctx.user),
        format!("WorkingDirectory={}", unit_path(&p.repo.to_string_lossy())?),
    ]);
    if public {
        server.push("# The allowlist is its own unit (above): start only waits for it.".to_owned());
        server.push("Environment=REFORGED_ALLOWLIST=external".to_owned());
    }
    server.extend([
        format!("ExecStart={} \"start\"", unit_word(&ctx.host_bin.to_string_lossy())?),
        "# SIGTERM to start only: it stops the gateway first and every game server with the Director.".to_owned(),
        "KillMode=mixed".to_owned(),
        "KillSignal=SIGTERM".to_owned(),
        "TimeoutStopSec=120".to_owned(),
        "Restart=on-failure".to_owned(),
        "RestartSec=10".to_owned(),
    ]);
    // A port below 1024 (443) needs CAP_NET_BIND_SERVICE; nothing else does.
    if public && gateway_port(c) < 1024 {
        server.push("AmbientCapabilities=CAP_NET_BIND_SERVICE".to_owned());
        server.push("CapabilityBoundingSet=CAP_NET_BIND_SERVICE".to_owned());
    } else {
        server.push("CapabilityBoundingSet=".to_owned());
    }
    server.extend([
        "NoNewPrivileges=true".to_owned(),
        "LimitNOFILE=65536".to_owned(),
        String::new(),
        "[Install]".to_owned(),
        "WantedBy=multi-user.target".to_owned(),
        String::new(),
    ]);
    files.push((UNIT_SERVER.to_owned(), server.join("\n")));

    let units = if public {
        format!("{UNIT_ALLOWLIST} {UNIT_SERVER}")
    } else {
        UNIT_SERVER.to_owned()
    };
    let mut install = vec![
        "#!/bin/sh".to_owned(),
        "# Dauntless Reforged Revival: installs this server's systemd units and starts them. Written by `reforged-host setup`.".to_owned(),
        "# Run once (and again after setup changes them):  sudo sh install.sh".to_owned(),
        "set -eu".to_owned(),
        "[ \"$(id -u)\" -eq 0 ] || { echo \"run it with sudo: sudo sh $0\" >&2; exit 1; }".to_owned(),
        "here=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)".to_owned(),
    ];
    if !public {
        install.push(format!(
            "if [ -f /etc/systemd/system/{UNIT_ALLOWLIST} ]; then systemctl disable --now {UNIT_ALLOWLIST} || true; rm -f /etc/systemd/system/{UNIT_ALLOWLIST}; fi"
        ));
    }
    install.extend([
        format!("for unit in {units}; do"),
        "    install -m 0644 \"$here/$unit\" \"/etc/systemd/system/$unit\"".to_owned(),
        "done".to_owned(),
        "systemctl daemon-reload".to_owned(),
        format!("systemctl enable {units}"),
        format!("systemctl restart {units}"),
        format!("echo \"Started. Status: systemctl status {UNIT_SERVER}   Logs: journalctl -u {UNIT_SERVER} -f\""),
        format!("echo \"Stop: sudo systemctl stop {UNIT_SERVER}   Start: sudo systemctl start {UNIT_SERVER}\""),
        String::new(),
    ]);
    files.push(("install.sh".to_owned(), install.join("\n")));
    Ok(files)
}

/// Writes the systemd units and their `install.sh`; returns the files.
pub fn write_systemd_units(config: &HostConfig, paths: &Paths) -> Result<Vec<PathBuf>, HostError> {
    write_systemd_units_with(config, paths, &unit_context(paths)?)
}

pub fn write_systemd_units_with(
    config: &HostConfig,
    paths: &Paths,
    ctx: &UnitContext,
) -> Result<Vec<PathBuf>, HostError> {
    let files = systemd_units(config, paths, ctx)?;
    let dir = &paths.systemd;
    fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
    for entry in fs::read_dir(dir).map_err(|e| HostError::io(dir, e))? {
        let entry = entry.map_err(|e| HostError::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !files.iter().any(|(n, _)| *n == name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    let mut written = Vec::new();
    for (name, text) in files {
        let file = dir.join(&name);
        fs::write(&file, text).map_err(|e| HostError::io(&file, e))?;
        set_mode(&file, if name.ends_with(".sh") { 0o755 } else { 0o644 })?;
        written.push(file);
    }
    Ok(written)
}

pub fn systemd_installed(unit: &str) -> bool {
    Path::new("/etc/systemd/system").join(unit).exists()
}

// ------------------------------------------------------------------ host firewall

/// The ufw script setup writes to `firewall.sh`. The allowlist's own nftables table already keeps
/// the game ports closed to everyone who has not logged in; this only opens the way through a host
/// firewall (ufw), when one is active.
pub fn linux_firewall_script(c: &HostConfig) -> Result<String, HostError> {
    let game = game_ports_text(c, ":");
    let mut lines: Vec<String> = [
        "#!/bin/sh",
        "# Dauntless Reforged Revival: lets a host firewall (ufw) pass this server's traffic. Written by `reforged-host setup`.",
        "# Run once:  sudo sh firewall.sh",
        "set -eu",
        "[ \"$(id -u)\" -eq 0 ] || { echo \"run it with sudo: sudo sh $0\" >&2; exit 1; }",
        "if command -v ufw >/dev/null 2>&1 && ufw status | grep -q 'Status: active'; then",
    ]
    .map(str::to_owned)
    .to_vec();
    if c.mode == ServerMode::Public {
        let port = gateway_port(c);
        lines.extend([
            format!("    ufw allow {port}/tcp comment 'Dauntless Reforged Revival gateway'"),
            "    # The allowlist's nftables table (reforged_revival) still closes these to everyone who has not logged in.".to_owned(),
            format!("    ufw allow {game}/udp comment 'Dauntless Reforged Revival game ports (allowlist)'"),
            "    echo 'ufw: added the rules'".to_owned(),
            "else".to_owned(),
            "    echo 'No active ufw: nothing to add here (the allowlist manages the game ports itself).'".to_owned(),
            "fi".to_owned(),
            format!(
                "echo \"Also allow inbound TCP {port} and UDP {} in your provider's firewall, if it has one (Hostinger: the VPS's Firewall page in hPanel).\"",
                game_ports_text(c, "-")
            ),
        ]);
    } else {
        lines.extend([
            format!(
                "    ufw allow in on tailscale0 to any port {} proto tcp comment 'Dauntless Reforged Revival gateway (Tailscale)'",
                PORTS.private_gateway
            ),
            format!("    ufw allow in on tailscale0 to any port {game} proto udp comment 'Dauntless Reforged Revival game ports (Tailscale)'"),
            "    echo 'ufw: added the rules'".to_owned(),
            "else".to_owned(),
            format!(
                "    echo 'No active ufw. The game ports (UDP {}) are reachable from every network: block them outside Tailscale, for example with ufw (ufw allow OpenSSH; ufw allow in on tailscale0; ufw enable).'",
                game_ports_text(c, "-")
            ),
            "fi".to_owned(),
        ]);
    }
    Ok(lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tnode\nUid:\t1000\t1000\t1000\t1000\nCapEff:\t0000000000001400\n";

    #[test]
    fn reads_capabilities_and_the_user_from_proc_status() {
        assert_eq!(effective_capabilities(STATUS), 0x1400);
        assert!(has_capability(CAP_NET_BIND_SERVICE, Some(STATUS)));
        assert!(has_capability(CAP_NET_ADMIN, Some(STATUS)));
        assert!(!has_capability(CAP_NET_ADMIN, Some("CapEff:\t0000000000000000\n")));
        assert!(!has_capability(CAP_NET_ADMIN, None));
        assert_eq!(effective_uid(STATUS), Some(1000));
        assert_eq!(effective_uid("Uid:\t1000\t0\t0\t0\n"), Some(0));
    }

    #[test]
    fn port_privilege_as_the_node_kit() {
        assert!(can_bind_port_with(4443, false, None, None));
        assert!(can_bind_port_with(443, true, None, None));
        assert!(can_bind_port_with(443, false, None, Some("80\n")));
        assert!(!can_bind_port_with(443, false, Some("CapEff:\t0\n"), Some("1024")));
        assert!(can_bind_port_with(443, false, Some(STATUS), Some("1024")));
    }

    #[test]
    fn quotes_shell_and_unit_words() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(unit_word(r#"/a b/100%$x"y\"#).unwrap(), r#""/a b/100%%$$x\"y\\""#);
        assert_eq!(unit_path("/srv/100%").unwrap(), "/srv/100%%");
        assert!(unit_word("a\nb").is_err());
        let script = gameserver_wrapper_script(Path::new("/home/reforged/Reforged/bin/reforged-host"));
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("exec '/home/reforged/Reforged/bin/reforged-host' gameserver \"$@\"\n"));
    }
}
