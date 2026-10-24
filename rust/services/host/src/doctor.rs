//! `doctor`'s checks (`legacy/ReforgedHost/src/doctor.ts`): everything the server needs, checked from
//! the files on this PC and the services that answer. Each check says what to do when it fails.
//! `start` (RH2) and `setup` use [`tailscale_ipv4`], [`port_in_use`] and [`port_owner_hint`].

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::commands::setup::options::training_text;
use crate::commands::setup::paradox::{Layout, missing_paradox};
use crate::error::HostError;
use crate::gamedata::{BACKEND_GAME_DATA, DIRECTOR_GAME_DATA, GameDataKind, game_data_state};
use crate::hostconfig::{
    HostConfig, PerformanceOptions, Platform, SECRET_FILE, ServerMode, game_ports_text, gateway_port, linux_options,
    load_host_config, performance_options, read_env_file,
};
use crate::invite::{certificate_fingerprint, http_request, served_fingerprint};
use crate::mongo::{MongoCheck, MongoProblem, ReplicaSetOptions, ensure_replica_set};
use crate::paths::{GAME_VERSION_TEXT, PINNED_EXE_SHA256, PORTS, Paths, RUNTIME_FILE_NAMES, game_exe_relative};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub area: &'static str,
    pub level: Level,
    pub text: String,
    pub fix: Option<String>,
}

fn check(area: &'static str, level: Level, text: impl Into<String>) -> Check {
    Check {
        area,
        level,
        text: text.into(),
        fix: None,
    }
}

fn fix(area: &'static str, level: Level, text: impl Into<String>, fix: impl Into<String>) -> Check {
    Check {
        area,
        level,
        text: text.into(),
        fix: Some(fix.into()),
    }
}

/// This PC's Tailscale IPv4 (`tailscale ip -4`), or `None` when Tailscale is not running.
pub fn tailscale_ipv4() -> Result<Option<String>, HostError> {
    let out = crate::run::capture("tailscale", &["ip", "-4"], None)?;
    Ok(first_ipv4_line(out.as_deref()))
}

fn first_ipv4_line(out: Option<&str>) -> Option<String> {
    let first = out?.lines().next()?.trim();
    first.parse::<std::net::Ipv4Addr>().is_ok().then(|| first.to_owned())
}

fn addresses(host: &str, port: u16) -> Vec<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .map(Iterator::collect)
        .unwrap_or_default()
}

/// Whether something listens on the TCP port (`host`: 127.0.0.1 when `None`).
pub fn port_in_use(port: u16, host: Option<&str>) -> Result<bool, HostError> {
    let host = host.unwrap_or("127.0.0.1");
    Ok(addresses(host, port)
        .iter()
        .any(|a| TcpStream::connect_timeout(a, Duration::from_millis(800)).is_ok()))
}

/// How to find the program on a port (`ss -tlnp ...` on Linux, `netstat -ano | findstr ...` on Windows).
pub fn port_owner_hint(port: u16) -> Result<String, HostError> {
    Ok(port_owner_hint_for(port, Platform::current()))
}

pub fn port_owner_hint_for(port: u16, platform: Platform) -> String {
    if platform == Platform::Windows {
        format!("netstat -ano | findstr :{port} shows the process id (Task Manager > Details names it)")
    } else {
        format!("sudo ss -tlnp 'sport = :{port}' shows which program")
    }
}

/// `waitForHttp`: whether an HTTP server answers the path (any status) before the time is up.
pub fn wait_for_http(port: u16, path: &str, timeout: Duration) -> bool {
    let until = Instant::now() + timeout;
    loop {
        if http_request("127.0.0.1", port, "GET", path, &[], None, Duration::from_secs(2)).is_ok() {
            return true;
        }
        if Instant::now() > until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn sha256_file(file: &Path) -> Result<String, HostError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut hasher = Sha256::new();
    let mut handle = std::fs::File::open(file).map_err(|e| HostError::io(file, e))?;
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = handle.read(&mut buffer).map_err(|e| HostError::io(file, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Node's `semver >= 20.19` for `node --version`.
pub fn node_version_ok(version: &str) -> bool {
    let mut parts = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => major > 20 || (major == 20 && minor >= 19),
        _ => false,
    }
}

pub fn count_function_files(sdk_dir: &Path) -> usize {
    std::fs::read_dir(sdk_dir).map_or(0, |d| {
        d.filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with("_functions.cpp"))
            .count()
    })
}

pub fn game_checks(game_dir: Option<&str>, hash_exe: bool) -> Result<Vec<Check>, HostError> {
    let Some(game_dir) = game_dir else {
        return Ok(vec![fix(
            "game",
            Level::Fail,
            "no game folder for the dedicated game servers",
            "reforged-host setup --game \"D:\\Games\\Dauntless\"",
        )]);
    };
    let exe = Path::new(game_dir).join(game_exe_relative());
    if !exe.exists() {
        return Ok(vec![fix(
            "game",
            Level::Fail,
            format!("no game executable at {}", exe.display()),
            "point --game at the Dauntless folder (the one with Archon inside)",
        )]);
    }
    let mut out = Vec::new();
    let version_text = std::fs::read_to_string(Path::new(game_dir).join("Version.txt"))
        .ok()
        .map(|t| {
            t.trim_start_matches('\u{feff}')
                .trim()
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default();
    out.push(if version_text == GAME_VERSION_TEXT {
        check("game", Level::Ok, format!("Version.txt: {version_text}"))
    } else {
        let shown = if version_text.is_empty() {
            "nothing"
        } else {
            &version_text
        };
        check(
            "game",
            Level::Warn,
            format!(
                "Version.txt says {}, not {GAME_VERSION_TEXT}",
                service_kit::jsjson::stringify(&shown.into())
            ),
        )
    });
    if hash_exe {
        let hash = sha256_file(&exe)?;
        out.push(if hash == PINNED_EXE_SHA256 {
            check(
                "game",
                Level::Ok,
                "the executable is the Dauntless 1.14.7 build (SHA-256 matches)",
            )
        } else {
            fix(
                "game",
                Level::Fail,
                format!("the executable is another build (SHA-256 {}…)", &hash[..16]),
                "install 1.14.7 with Legendary (docs/players.md) for the server too",
            )
        });
    }
    Ok(out)
}

/// Which of the game servers' CPU patches a built MysticParadox.dll carries, from strings only those
/// patches put in it: 0019 (upstream's CPU fixes) names `LEGACY_EXPECTED_PLAYER_WALK.flag` (UTF-16),
/// 0020 (the frame cap) reads `REFORGED_SERVER_MAX_FPS` (ASCII). `None` when the file cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuPatches {
    pub cpu_fixes: bool,
    pub frame_cap: bool,
}

pub fn runtime_cpu_patches(dll: &Path) -> Option<CpuPatches> {
    let bytes = std::fs::read(dll).ok()?;
    let utf16: Vec<u8> = "LEGACY_EXPECTED_PLAYER_WALK.flag"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    Some(CpuPatches {
        cpu_fixes: contains(&utf16),
        frame_cap: contains(b"REFORGED_SERVER_MAX_FPS"),
    })
}

/// What the services other than the game servers use (backend, Director, gateway, content, MongoDB), roughly.
pub const SERVICES_MEMORY_MB: i64 = 1536;

/// How many hunts fit in this machine's memory next to Ramsgate, the Training Grounds and the services.
pub fn hunts_that_fit(total_mb: u64, perf: &PerformanceOptions) -> u64 {
    let hunt = i64::from(perf.hunt_memory_mb.max(1));
    let left = total_mb as i64 - SERVICES_MEMORY_MB - i64::from(perf.min_free_memory_mb) - 2 * hunt;
    u64::try_from(left.div_euclid(hunt)).unwrap_or(0)
}

/// This machine's memory in MB (Node's `os.totalmem()`); `None` when it cannot be read.
pub fn total_memory_mb() -> Option<u64> {
    if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = text
            .lines()
            .find(|l| l.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        Some(kb / 1024)
    } else if cfg!(windows) {
        let out = crate::run::capture(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ],
            None,
        )
        .ok()??;
        Some(out.trim().parse::<u64>().ok()? / (1024 * 1024))
    } else {
        None
    }
}

pub fn performance_checks(
    config: &HostConfig,
    published_dll: Option<&Path>,
    total_memory_mb: Option<u64>,
    platform: Platform,
) -> Vec<Check> {
    let perf = performance_options(config);
    let mut out = vec![fix(
        "performance",
        Level::Info,
        format!(
            "game servers at {}{}; Training Grounds {}; {} game logs; at most {} hunts (UDP {}); {}",
            if perf.server_fps > 0 {
                format!("{} fps", perf.server_fps)
            } else {
                "no frame cap".into()
            },
            if perf.idle_fps > 0 {
                format!(", {} fps after 15 s without players", perf.idle_fps)
            } else {
                String::new()
            },
            training_text(&perf),
            if perf.verbose_game_logs { "verbose" } else { "quiet" },
            config.max_hunt_servers,
            game_ports_text(config, "-"),
            if perf.memory_guard {
                format!(
                    "a new hunt only with {} MB free",
                    perf.hunt_memory_mb + perf.min_free_memory_mb
                )
            } else {
                "no memory guard".into()
            }
        ),
        "reforged-host setup --server-fps 30 --idle-fps 10 --training-on-demand --quiet-game-logs (each one is optional)",
    )];
    if let Some(total) = total_memory_mb {
        let fit = hunts_that_fit(total, &perf);
        if fit < u64::from(config.max_hunt_servers) {
            let gb = format!("{:.1}", total as f64 / 1024.0);
            let need = perf.hunt_memory_mb + perf.min_free_memory_mb;
            let hunts = config.max_hunt_servers;
            out.push(if perf.memory_guard {
                check(
                    "performance",
                    Level::Info,
                    format!(
                        "{hunts} hunts at once may need more than this machine's {gb} GB: about {fit} fit; the memory guard refuses a new hunt while there is not {need} MB free"
                    ),
                )
            } else {
                fix(
                    "performance",
                    Level::Warn,
                    format!(
                        "{hunts} hunts at once may need more than this machine's {gb} GB (about {fit} fit), and the memory guard is off: a full server can run out of memory"
                    ),
                    format!("reforged-host setup --memory-guard, or --max-hunts {}", fit.max(1)),
                )
            });
        }
    }
    if let Some(found) = published_dll.and_then(runtime_cpu_patches) {
        let mut missing = Vec::new();
        if !found.cpu_fixes {
            missing.push("0019 (CPU fixes)");
        }
        if !found.frame_cap {
            missing.push("0020 (frame cap)");
        }
        out.push(if missing.is_empty() {
            check(
                "performance",
                Level::Ok,
                "the published runtime has the game servers' CPU patches (0019, 0020)",
            )
        } else {
            fix(
                "performance",
                Level::Warn,
                format!(
                    "the published runtime was built without patch {}: every game server uses far more CPU than it needs",
                    missing.join(" and ")
                ),
                if platform == Platform::Linux {
                    "on the Windows PC: reforged-host update, build the runtime again (Host tools page) and reforged-host publish-runtime; then send it here (Deploy-FromWindows.ps1 ... -SkipGame, or copy .workspace\\runtime and reforged-host publish-runtime --from <folder>) and restart the server (players accept the new runtime once)"
                } else {
                    "reforged-host update, build the runtime again (Host tools page, or native\\paradox-runtime\\_build.bat), reforged-host publish-runtime, restart the server (players accept the new runtime once)"
                },
            )
        });
    }
    out
}

/// `systemdInstalled` in `linux.ts`.
fn systemd_installed(unit: &str) -> bool {
    Path::new("/etc/systemd/system").join(unit).exists()
}

/// `winePrefixReady` in `wine.ts`.
fn wine_prefix_ready(prefix: &Path) -> bool {
    prefix.join("system.reg").exists()
}

/// Linux: Wine (or Proton), the virtual display, nftables, the launcher, the prefix and the systemd units.
pub fn linux_checks(p: &Paths, config: &HostConfig) -> Result<Vec<Check>, HostError> {
    use crate::linux::{RuntimeKind, UNIT_ALLOWLIST, UNIT_SERVER, can_bind_port, game_runtime};
    use crate::wine::find_on_path;
    let mut out = Vec::new();
    let lx = linux_options(config);
    let runtime = game_runtime(&lx)?;
    match &runtime {
        None => out.push(fix(
            "linux",
            Level::Fail,
            "Wine is not installed: the game servers cannot start",
            "sudo apt install wine wine64 (docs/hosting-linux.md)",
        )),
        Some(r) if r.kind == RuntimeKind::Proton => out.push(check("linux", Level::Ok, format!("Proton {}", r.path))),
        Some(r) => {
            let version = crate::run::capture(&r.path, &["--version"], None)?;
            out.push(check(
                "linux",
                Level::Ok,
                format!("{} ({})", version.as_deref().unwrap_or("Wine"), r.path),
            ));
        }
    }
    if lx.xvfb {
        let mut missing = Vec::new();
        for name in ["xvfb-run", "xauth"] {
            if find_on_path(&[name])?.is_none() {
                missing.push(name);
            }
        }
        out.push(if missing.is_empty() {
            check(
                "linux",
                Level::Ok,
                "xvfb-run and xauth (the game servers' virtual display)",
            )
        } else {
            fix(
                "linux",
                Level::Fail,
                format!(
                    "{} not installed (the game servers need a virtual display)",
                    missing.join(" and ")
                ),
                "sudo apt install xvfb xauth",
            )
        });
    }
    if config.mode == ServerMode::Public {
        let nft = ["/usr/sbin/nft", "/sbin/nft"].iter().any(|f| Path::new(f).exists());
        out.push(if nft {
            check("linux", Level::Ok, "nftables (the allowlist)")
        } else {
            fix(
                "linux",
                Level::Fail,
                "nftables is not installed (the allowlist needs it)",
                "sudo apt install nftables",
            )
        });
    }
    out.push(if p.gameserver_wrapper.exists() {
        check(
            "linux",
            Level::Ok,
            format!("game-server launcher {}", p.gameserver_wrapper.display()),
        )
    } else {
        fix(
            "linux",
            Level::Fail,
            "the game-server launcher is missing",
            "reforged-host setup",
        )
    });
    if runtime.as_ref().is_some_and(|r| r.kind == RuntimeKind::Wine) {
        out.push(if wine_prefix_ready(&p.wine_prefix) {
            check("linux", Level::Ok, format!("Wine prefix {}", p.wine_prefix.display()))
        } else {
            fix(
                "linux",
                Level::Warn,
                "the Wine prefix is not made yet",
                "reforged-host setup (or the first reforged-host start) makes it",
            )
        });
    }
    let public = config.mode == ServerMode::Public;
    let units: Vec<&str> = if public {
        vec![UNIT_SERVER, UNIT_ALLOWLIST]
    } else {
        vec![UNIT_SERVER]
    };
    let installed: Vec<&str> = units.iter().copied().filter(|u| systemd_installed(u)).collect();
    out.push(if installed.len() == units.len() {
        check(
            "linux",
            Level::Ok,
            format!("systemd units installed ({})", units.join(", ")),
        )
    } else {
        let level = if public && !can_bind_port(gateway_port(config))? {
            Level::Warn
        } else {
            Level::Info
        };
        fix(
            "linux",
            level,
            format!(
                "systemd units not installed{}: the server does not start at boot{}",
                if installed.is_empty() {
                    String::new()
                } else {
                    format!(" (only {})", installed.join(", "))
                },
                if public {
                    ", and a public server needs them for port 443 and nftables"
                } else {
                    ""
                }
            ),
            "sudo sh .workspace/config/systemd/install.sh",
        )
    });
    Ok(out)
}

pub struct DoctorOptions {
    pub hash_exe: bool,
}

pub fn run_doctor(p: &Paths, options: &DoctorOptions) -> Result<Vec<Check>, HostError> {
    let platform = Platform::current();
    let linux = platform == Platform::Linux;
    let layout = Layout::new(p);
    let mut checks: Vec<Check> = Vec::new();

    // ---- tools (Node.js runs the backend and the Director until RB14)
    let node = crate::run::capture("node", &["--version"], None)?;
    checks.push(match node.as_deref() {
        Some(v) if node_version_ok(v) => check("tools", Level::Ok, format!("Node.js {v}")),
        Some(v) => fix(
            "tools",
            Level::Fail,
            format!("Node.js {v} is too old"),
            "install Node.js 20.19 or newer (https://nodejs.org)",
        ),
        None => fix(
            "tools",
            Level::Fail,
            "Node.js is not installed",
            "install Node.js 20.19 or newer (https://nodejs.org)",
        ),
    });
    checks.push(match crate::run::capture("git", &["--version"], None)? {
        Some(v) => check("tools", Level::Ok, v),
        None => fix(
            "tools",
            Level::Fail,
            "git is not installed",
            if linux {
                "sudo apt install git"
            } else {
                "install Git for Windows (https://git-scm.com)"
            },
        ),
    });

    // ---- Mystic Paradox (paradox/ and native/)
    let missing = missing_paradox(&layout);
    checks.push(if missing.is_empty() {
        check("paradox", Level::Ok, "Mystic Paradox in paradox/ and native/")
    } else {
        fix(
            "paradox",
            Level::Fail,
            format!("this checkout has no {}", missing.join(", ")),
            "clone the project again with git (docs/hosting.md) and move .workspace into the new clone",
        )
    });
    let built: [(&str, PathBuf); 4] = [
        ("backend", layout.backend_dir.join("build").join("server.js")),
        ("Director", layout.director_dir.join("dist").join("server.js")),
        ("gateway", layout.gateway_dir.join("dist").join("server.js")),
        ("content server", layout.content_dir.join("dist").join("server.js")),
    ];
    for (name, file) in built {
        checks.push(if file.exists() {
            check("services", Level::Ok, format!("{name} built"))
        } else {
            fix(
                "services",
                Level::Fail,
                format!("{name} is not built"),
                "reforged-host setup",
            )
        });
    }

    // ---- configuration
    let config = match load_host_config(p) {
        Ok(c) => c,
        Err(e) => {
            checks.push(fix(
                "config",
                Level::Fail,
                format!("settings: {e}"),
                "reforged-host setup --mode ... --host ...",
            ));
            None
        }
    };
    if p.host_toml.exists() && p.host_json.exists() {
        checks.push(fix(
            "config",
            Level::Warn,
            "host.json is next to host.toml and is not read: host.toml holds this server's settings",
            "delete host.json, or rename it host.json.bak",
        ));
    } else if p.host_json.exists() {
        checks.push(fix(
            "config",
            Level::Info,
            "the settings are still in a Node kit's host.json",
            "reforged-host setup (moves them to host.toml)",
        ));
    }
    if let Some(config) = &config {
        checks.push(check(
            "config",
            Level::Ok,
            format!(
                "{} server \"{}\" at {}, game servers on {}",
                config.mode.as_str(),
                config.name,
                config.host,
                config.game_address
            ),
        ));
        let mut names = vec!["backend", "director", "content", "gateway"];
        if config.mode == ServerMode::Public {
            names.push("allowlist");
        }
        let mut missing: Vec<String> = names
            .iter()
            .filter(|n| !p.config.join(format!("{n}.env")).exists())
            .map(|n| format!("{n}.env"))
            .collect();
        if !p.config.join(SECRET_FILE).exists() {
            missing.push(SECRET_FILE.into());
        }
        checks.push(if missing.is_empty() {
            check("config", Level::Ok, "env files and secrets written")
        } else {
            fix(
                "config",
                Level::Fail,
                format!("missing {}", missing.join(", ")),
                "reforged-host setup",
            )
        });
        checks.extend(game_checks(config.game_dir.as_deref(), options.hash_exe)?);
        if config.mode == ServerMode::Private {
            checks.push(match tailscale_ipv4()? {
                None => fix(
                    "network",
                    Level::Fail,
                    "Tailscale is not running on this PC",
                    "install and sign in to Tailscale (https://tailscale.com/download)",
                ),
                Some(ip) if ip == config.game_address => check("network", Level::Ok, format!("Tailscale {ip}")),
                Some(ip) => fix(
                    "network",
                    Level::Fail,
                    format!(
                        "this PC's Tailscale address is {ip}, host.toml says {}",
                        config.game_address
                    ),
                    format!("reforged-host setup --host {ip}"),
                ),
            });
        } else {
            if p.cert_file.exists() {
                let ours = certificate_fingerprint(&p.cert_file)?;
                checks.push(check("network", Level::Ok, format!("gateway certificate {ours}")));
                let port = gateway_port(config);
                let move_it = format!(
                    "{}; stop it and restart the server, or move the gateway to a free port (reforged-host setup --public-port 4443, then new invites)",
                    port_owner_hint_for(port, platform)
                );
                match served_fingerprint(port, "127.0.0.1", Duration::from_secs(4)) {
                    Some(served) if served == ours => checks.push(check(
                        "network",
                        Level::Ok,
                        format!("port {port} serves this certificate (invites match)"),
                    )),
                    Some(served) => checks.push(fix(
                        "network",
                        Level::Fail,
                        format!(
                            "another program answers on port {port} with its own certificate ({}...): players' launchers refuse it",
                            &served[..16]
                        ),
                        move_it,
                    )),
                    None if port_in_use(port, None)? => checks.push(fix(
                        "network",
                        Level::Fail,
                        format!("something on port {port} does not speak TLS: another program holds the gateway's port"),
                        move_it,
                    )),
                    None => checks.push(check(
                        "running",
                        Level::Info,
                        format!("the gateway is not running (port {port})"),
                    )),
                }
            } else {
                checks.push(fix(
                    "network",
                    Level::Fail,
                    "no gateway certificate",
                    "reforged-host setup",
                ));
            }
            checks.push(check(
                "network",
                Level::Info,
                if linux {
                    format!(
                        "open inbound TCP {} and UDP {} in your provider's firewall (Hostinger: the VPS's Firewall page in hPanel); sudo sh .workspace/config/firewall.sh adds them to ufw",
                        gateway_port(config),
                        game_ports_text(config, "-")
                    )
                } else {
                    format!(
                        "open inbound TCP {} and UDP {} (run .workspace\\config\\firewall.ps1 as administrator); forward them on your router if this PC is behind NAT",
                        gateway_port(config),
                        game_ports_text(config, "-")
                    )
                },
            ));
        }
        if linux {
            checks.extend(linux_checks(p, config)?);
        }
    }

    // ---- SDK, builds, runtime
    let functions = count_function_files(&layout.sdk_dir);
    let not_here = if linux { Level::Info } else { Level::Fail };
    checks.push(if layout.sdk_header.exists() && functions > 0 {
        check(
            "sdk",
            Level::Ok,
            format!("Dumper-7 SDK imported ({functions} *_functions.cpp files)"),
        )
    } else {
        fix(
            "sdk",
            not_here,
            "no SDK in native/paradox-runtime yet",
            "dump it with Dumper-7 from your own 1.14.7 game (launcher > Settings > Host tools, or docs/sdk-and-game-data.md)",
        )
    });
    checks.push(if layout.runtime_dll.exists() {
        check("runtime", Level::Ok, "MysticParadox.dll built")
    } else {
        fix(
            "runtime",
            not_here,
            "MysticParadox.dll is not built here",
            if linux {
                "it is built on a Windows PC: reforged-host publish-runtime --from <folder> publishes it (docs/hosting-linux.md)"
            } else {
                "build native\\paradox-runtime (Host tools page, or native\\paradox-runtime\\_build.bat)"
            },
        )
    });
    checks.push(if layout.loader_dll.exists() {
        check("runtime", Level::Ok, "winmm.dll built")
    } else {
        fix(
            "runtime",
            not_here,
            "winmm.dll is not built here",
            if linux {
                "it is built on a Windows PC: reforged-host publish-runtime --from <folder> publishes it"
            } else {
                "cargo build --release in native\\runtime-loader (Host tools page)"
            },
        )
    });
    let published = RUNTIME_FILE_NAMES.iter().all(|n| p.runtime_publish.join(n).exists())
        && p.runtime_publish.join("runtime.json").exists();
    checks.push(if published {
        check("runtime", Level::Ok, "runtime published for players")
    } else {
        fix(
            "runtime",
            Level::Fail,
            "no runtime published for players",
            "reforged-host publish-runtime",
        )
    });
    if let Some(game_dir) = config
        .as_ref()
        .and_then(|c| c.game_dir.as_deref())
        .filter(|_| published)
    {
        let exe = Path::new(game_dir).join(game_exe_relative());
        let win64 = exe.parent().unwrap_or(Path::new(game_dir));
        let mut same = true;
        for name in RUNTIME_FILE_NAMES {
            let there = win64.join(name);
            same = same && there.exists() && sha256_file(&there)? == sha256_file(&p.runtime_publish.join(name))?;
        }
        checks.push(if same {
            check("runtime", Level::Ok, "the game servers use the published runtime")
        } else {
            fix(
                "runtime",
                Level::Fail,
                "the server's game folder does not have the published runtime",
                "reforged-host publish-runtime, then restart the server",
            )
        });
    }

    if let Some(config) = &config {
        let dll = p.runtime_publish.join("MysticParadox.dll");
        checks.extend(performance_checks(
            config,
            published.then_some(dll.as_path()),
            total_memory_mb(),
            platform,
        ));
    }

    // ---- game data
    for (label, dir, files) in [
        ("backend", &layout.backend_game_data, &BACKEND_GAME_DATA),
        ("Director", &layout.director_game_data, &DIRECTOR_GAME_DATA),
    ] {
        let g = game_data_state(dir, files)?;
        checks.push(match g.state {
            GameDataKind::Real => check("game data", Level::Ok, format!("{label}: real data")),
            GameDataKind::Synthetic => fix(
                "game data",
                Level::Warn,
                format!(
                    "{label}: example data in {} (start-up tests only, not playable)",
                    g.synthetic.join(", ")
                ),
                "export from your game and reforged-host gamedata (docs/sdk-and-game-data.md)",
            ),
            _ => fix(
                "game data",
                Level::Fail,
                format!("{label}: missing {}", g.missing.join(", ")),
                "reforged-host gamedata, or reforged-host gamedata --examples for a first start-up test",
            ),
        });
    }

    // ---- database and running services
    if let Some(config) = &config {
        let backend_env = read_env_file(&p.config.join("backend.env"))?;
        let uri = backend_env.get("MONGODB_URI").unwrap_or(&config.mongo.uri).to_owned();
        let mongo = ensure_replica_set(
            &uri,
            &ReplicaSetOptions {
                initiate: false,
                host: None,
                timeout_ms: Some(2500),
            },
        )?;
        checks.push(database_check(&mongo, &uri, config.mongo.manage));
    }
    for (name, port, path) in [
        ("backend", PORTS.backend, "/dauntless-status"),
        ("Director", PORTS.director, "/api/instances"),
        ("content server", PORTS.content, "/content/v1/news"),
    ] {
        let answering = port_in_use(port, None)? && wait_for_http(port, path, Duration::from_millis(1500));
        checks.push(check(
            "running",
            Level::Info,
            format!(
                "{name} {} (127.0.0.1:{port})",
                if answering { "is running" } else { "is not running" }
            ),
        ));
    }
    Ok(checks)
}

pub fn database_check(mongo: &MongoCheck, uri: &str, manage: bool) -> Check {
    match mongo {
        MongoCheck::Ok {
            set_name,
            primary: true,
            ..
        } => check(
            "database",
            Level::Ok,
            format!("MongoDB replica set {set_name}, primary"),
        ),
        MongoCheck::Ok { set_name, .. } => check(
            "database",
            Level::Warn,
            format!("MongoDB replica set {set_name}, but no primary yet"),
        ),
        MongoCheck::Failed {
            reason: MongoProblem::Unreachable,
            ..
        } => fix(
            "database",
            if manage { Level::Info } else { Level::Fail },
            format!("MongoDB is not answering at {uri}"),
            if manage {
                "reforged-host start starts it"
            } else {
                "start your MongoDB, with replication.replSetName: rs0"
            },
        ),
        MongoCheck::Failed { detail, .. } => check("database", Level::Fail, detail.clone()),
    }
}

/// Prints the checks grouped by area; returns the failures and warnings.
pub fn print_checks(checks: &[Check], print: &mut dyn FnMut(&str)) -> (usize, usize) {
    let mut area = "";
    for c in checks {
        if c.area != area {
            area = c.area;
            print(&format!("\n{area}"));
        }
        let mark = match c.level {
            Level::Ok => "[ ok ]",
            Level::Warn => "[warn]",
            Level::Fail => "[FAIL]",
            Level::Info => "[info]",
        };
        print(&format!("  {mark} {}", c.text));
        if let Some(fix) = c.fix.as_deref().filter(|_| c.level != Level::Ok) {
            print(&format!("         -> {fix}"));
        }
    }
    (
        checks.iter().filter(|c| c.level == Level::Fail).count(),
        checks.iter().filter(|c| c.level == Level::Warn).count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tailscale_address_from_the_first_line() {
        assert_eq!(
            first_ipv4_line(Some("100.101.102.103\nfd7a::1")),
            Some("100.101.102.103".into())
        );
        assert_eq!(first_ipv4_line(Some("not running")), None);
        assert_eq!(first_ipv4_line(None), None);
    }

    #[test]
    fn node_20_19_or_newer() {
        assert!(node_version_ok("v26.7.0"));
        assert!(node_version_ok("v20.19.1"));
        assert!(!node_version_ok("v20.18.0"));
        assert!(!node_version_ok("v18.20.4"));
    }
}
