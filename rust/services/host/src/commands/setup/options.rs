//! Setup's options into host.json (`hostConfigFromArgs()` and the `*FromArgs` readers in `cli.ts`).
//! Each option keeps what host.json had when it is not given.

use std::path::{Component, PathBuf};

use serde_json::{Map, Value, json};
use service_kit::{js, jsjson};

use super::SetupEnv;
use crate::cli::SetupArgs;
use crate::error::HostError;
use crate::hostconfig::{
    DEFAULT_MONGO_URI, HostConfig, LinuxOptions, MAX_MULTIPLIER, Multipliers, PerformanceOptions, Platform, ServerMode,
    game_ports_text, is_linux_interface, is_public_ipv4, is_tailscale_ipv4, performance_options, valid_fps,
    valid_megabytes, valid_minutes,
};
use crate::paths::PORTS;

fn bad(text: impl Into<String>) -> HostError {
    HostError::message(text)
}

/// The Node kit's `str()`: a value with something in it, trimmed.
pub fn text(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(js::trim).filter(|v| !v.is_empty())
}

/// `Number(text)`.
pub fn number(text: &str) -> f64 {
    js::number(Some(&Value::String(text.to_owned())))
}

/// A number as `JSON.stringify` writes it (NaN and the infinities as null).
pub fn number_value(value: f64) -> Value {
    jsjson::to_js(&json!(value))
}

/// A number as a template literal writes it.
pub fn number_text(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        jsjson::number(value)
    }
}

/// One multiplier option: the names it goes by (the first is the one errors name) and the value given.
#[derive(Debug, Clone, Copy)]
pub struct MultiplierOption<'a> {
    pub name: &'a str,
    pub value: Option<&'a str>,
}

/// `--xp-multiplier` / `--reward-multiplier` on setup (or `--xp` / `--rewards` on `multipliers`):
/// a number from 1 to 100.
fn multiplier_arg(option: MultiplierOption<'_>, fallback: f64) -> Result<f64, HostError> {
    let Some(raw) = option.value else {
        return Ok(fallback);
    };
    let stripped = raw.strip_prefix(['x', 'X']).unwrap_or(raw);
    let value = number(stripped);
    if !value.is_finite() || !(1.0..=MAX_MULTIPLIER).contains(&value) {
        return Err(bad(format!(
            "--{} must be a number from 1 to {} (got {raw})",
            option.name,
            number_text(MAX_MULTIPLIER)
        )));
    }
    Ok(value)
}

pub fn multipliers_from_args(
    off: bool,
    xp: MultiplierOption<'_>,
    rewards: MultiplierOption<'_>,
    mode: ServerMode,
    existing: Option<Multipliers>,
) -> Result<Option<Multipliers>, HostError> {
    if off {
        return Ok(None);
    }
    let xp = multiplier_arg(xp, existing.map_or(1.0, |m| m.xp))?;
    let rewards = multiplier_arg(rewards, existing.map_or(1.0, |m| m.rewards))?;
    if xp == 1.0 && rewards == 1.0 {
        return Ok(None);
    }
    if mode != ServerMode::Private {
        return Err(bad(
            "XP and reward multipliers are for private (Tailscale) test servers only",
        ));
    }
    Ok(Some(Multipliers { xp, rewards }))
}

/// Linux: `--wine <path or name>`, `--proton <script>` (`--no-proton`), `--no-xvfb` (`--xvfb`),
/// `--firewall-interface <eth0>` (`--no-firewall-interface`).
pub fn linux_from_args(
    args: &SetupArgs,
    existing: Option<&LinuxOptions>,
    find: &dyn Fn(&str) -> Result<Option<String>, HostError>,
) -> Result<LinuxOptions, HostError> {
    let program = |name: &str, what: &str| -> Result<String, HostError> {
        let found = if name.starts_with('/') {
            Some(name.to_owned())
        } else {
            find(name)?
        };
        found.ok_or_else(|| bad(format!("--{what} {name}: not found (give its full path)")))
    };
    let iface = text(&args.firewall_interface);
    if let Some(iface) = iface
        && !is_linux_interface(iface)
    {
        return Err(bad(format!(
            "--firewall-interface must be a network interface name like eth0 (got {iface})"
        )));
    }
    Ok(LinuxOptions {
        wine: match text(&args.wine) {
            Some(name) => Some(program(name, "wine")?),
            None => existing.and_then(|lx| lx.wine.clone()),
        },
        proton: if args.no_proton {
            None
        } else {
            match text(&args.proton) {
                Some(name) => Some(program(name, "proton")?),
                None => existing.and_then(|lx| lx.proton.clone()),
            }
        },
        xvfb: if args.no_xvfb {
            false
        } else if args.xvfb {
            true
        } else {
            existing.is_none_or(|lx| lx.xvfb)
        },
        firewall_interface: if args.no_firewall_interface {
            None
        } else {
            iface
                .map(str::to_owned)
                .or_else(|| existing.and_then(|lx| lx.firewall_interface.clone()))
        },
    })
}

/// `--public-port <port>`: the gateway's port when 443 is taken (a hosting panel); `--public-port 443`
/// goes back. The number as given; host.json's checks refuse one out of range.
pub fn public_port_from_args(
    args: &SetupArgs,
    mode: ServerMode,
    existing: Option<&HostConfig>,
) -> Result<Option<f64>, HostError> {
    let raw = text(&args.public_port);
    if mode != ServerMode::Public {
        if raw.is_some() {
            return Err(bad("--public-port is for public servers only"));
        }
        return Ok(None);
    }
    let port = match raw {
        None => existing
            .filter(|c| c.mode == ServerMode::Public)
            .and_then(|c| c.public_port)
            .map(f64::from),
        Some(raw) => Some(number(raw)),
    };
    match port {
        None => Ok(None),
        Some(p) if p == f64::from(PORTS.public_gateway) => Ok(None),
        Some(p) if !p.is_finite() || p.fract() != 0.0 => Err(bad(format!(
            "--public-port must be a TCP port number (got {})",
            raw.unwrap_or_default()
        ))),
        Some(p) => Ok(Some(p)),
    }
}

/// `--server-fps <0|10-240>`, `--idle-fps <0|1-240>`, `--training-on-demand` (`--training-at-boot`),
/// `--verbose-game-logs` (`--quiet-game-logs`), `--memory-guard` (`--no-memory-guard`),
/// `--min-free-ram <MB>`, `--hunt-ram <MB>`, `--training-idle-minutes <0-1440>`.
pub fn performance_from_args(
    args: &SetupArgs,
    existing: Option<PerformanceOptions>,
) -> Result<PerformanceOptions, HostError> {
    let current = existing.unwrap_or(crate::hostconfig::DEFAULT_PERFORMANCE);
    let fps = |name: &str, value: &Option<String>, min: f64, fallback: u32, meaning: &str| -> Result<u32, HostError> {
        let Some(raw) = text(value) else {
            return Ok(fallback);
        };
        let v = number(raw);
        if !valid_fps(v, min) {
            return Err(bad(format!(
                "--{name} must be 0 ({meaning}) or {} to 240 frames per second (got {raw})",
                number_text(min)
            )));
        }
        Ok(v as u32)
    };
    let minutes = |name: &str, value: &Option<String>, fallback: u32| -> Result<u32, HostError> {
        let Some(raw) = text(value) else {
            return Ok(fallback);
        };
        let v = number(raw);
        if !valid_minutes(v) {
            return Err(bad(format!(
                "--{name} must be 0 (keep them running) or 1 to 1440 minutes (got {raw})"
            )));
        }
        Ok(v as u32)
    };
    let megabytes = |name: &str, value: &Option<String>, min: f64, fallback: u32| -> Result<u32, HostError> {
        let Some(raw) = text(value) else {
            return Ok(fallback);
        };
        let v = number(strip_mb(raw));
        if !valid_megabytes(v, min) {
            return Err(bad(format!(
                "--{name} must be a whole number of megabytes, at least {} (got {raw})",
                number_text(min)
            )));
        }
        Ok(v as u32)
    };
    Ok(PerformanceOptions {
        server_fps: fps("server-fps", &args.server_fps, 10.0, current.server_fps, "no cap")?,
        idle_fps: fps(
            "idle-fps",
            &args.idle_fps,
            1.0,
            current.idle_fps,
            "the same as --server-fps",
        )?,
        training_on_demand: if args.training_at_boot {
            false
        } else if args.training_on_demand {
            true
        } else {
            current.training_on_demand
        },
        training_idle_minutes: minutes(
            "training-idle-minutes",
            &args.training_idle_minutes,
            current.training_idle_minutes,
        )?,
        verbose_game_logs: if args.quiet_game_logs {
            false
        } else if args.verbose_game_logs {
            true
        } else {
            current.verbose_game_logs
        },
        memory_guard: if args.no_memory_guard {
            false
        } else if args.memory_guard {
            true
        } else {
            current.memory_guard
        },
        min_free_memory_mb: megabytes("min-free-ram", &args.min_free_ram, 0.0, current.min_free_memory_mb)?,
        hunt_memory_mb: megabytes("hunt-ram", &args.hunt_ram, 256.0, current.hunt_memory_mb)?,
    })
}

/// `raw.replace(/\s*mb$/i, "")`.
fn strip_mb(raw: &str) -> &str {
    let lower = raw.to_ascii_lowercase();
    if lower.ends_with("mb") {
        raw[..raw.len() - 2].trim_end_matches(js::is_space)
    } else {
        raw
    }
}

pub fn training_text(perf: &PerformanceOptions) -> String {
    if !perf.training_on_demand {
        return "at boot".into();
    }
    if perf.training_idle_minutes > 0 {
        format!(
            "on demand, stopped after {} min without players",
            perf.training_idle_minutes
        )
    } else {
        "on demand".into()
    }
}

pub fn performance_summary(c: &HostConfig) -> String {
    let perf = performance_options(c);
    let fps = if perf.server_fps > 0 {
        format!("{} fps", perf.server_fps)
    } else {
        "no frame cap".into()
    };
    let idle = if perf.idle_fps > 0 {
        format!("{} fps after 15 s without players", perf.idle_fps)
    } else {
        "no idle cap".into()
    };
    let memory = if perf.memory_guard {
        format!(
            "a new hunt only with {} MB free",
            perf.hunt_memory_mb + perf.min_free_memory_mb
        )
    } else {
        "no memory guard".into()
    };
    format!(
        "Game servers: {fps}, {idle}; Training Grounds {}; {} game logs; at most {} hunt{} at once (UDP {}), {memory}",
        training_text(&perf),
        if perf.verbose_game_logs { "verbose" } else { "quiet" },
        c.max_hunt_servers,
        if c.max_hunt_servers == 1 { "" } else { "s" },
        game_ports_text(c, "-"),
    )
}

/// `path.win32.isAbsolute(p) || path.posix.isAbsolute(p)`: tests and Linux hosts give either form.
pub fn is_absolute_any(p: &str) -> bool {
    let b = p.as_bytes();
    matches!(b.first(), Some(b'/' | b'\\'))
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'))
}

/// `path.resolve(p)`: absolute, with `.` and `..` folded.
pub fn resolve_path(p: &str) -> String {
    let absolute = std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p));
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

/// host.json from the options, the old host.json and what setup finds on this PC (the Node kit's
/// `hostConfigFromArgs`), checked as host.json is. The keys come in setup's order.
pub fn host_config_from_args(
    args: &SetupArgs,
    existing: Option<&HostConfig>,
    env: &dyn SetupEnv,
) -> Result<HostConfig, HostError> {
    let mode = match text(&args.mode)
        .map(str::to_owned)
        .or_else(|| existing.map(|c| c.mode.as_str().to_owned()))
    {
        Some(m) if m == "private" => ServerMode::Private,
        Some(m) if m == "public" => ServerMode::Public,
        _ => {
            return Err(bad(
                "say which kind of server: --mode private (friends over Tailscale) or --mode public (a server on the internet)",
            ));
        }
    };
    let same_mode = existing.filter(|c| c.mode == mode);
    // A private server follows this PC's Tailscale address, which changes with the Tailscale account:
    // setup without --host takes the current one, and only falls back to the old one when Tailscale
    // is not running.
    let current = if mode == ServerMode::Private {
        env.tailscale_ipv4()?
    } else {
        None
    };
    let host = text(&args.host)
        .map(str::to_owned)
        .or_else(|| current.clone())
        .or_else(|| same_mode.map(|c| c.host.clone()));
    let Some(host) = host else {
        return Err(bad(if mode == ServerMode::Private {
            "Tailscale is not running here, so this PC's Tailscale address is unknown. Start Tailscale, or pass --host <your Tailscale IP>"
        } else {
            "a public server needs --host <its public IPv4 address or DNS name>"
        }));
    };
    let mut game_address = text(&args.game_address)
        .map(str::to_owned)
        .or_else(|| same_mode.filter(|c| c.host == host).map(|c| c.game_address.clone()));
    if game_address.is_none() {
        game_address = match mode {
            ServerMode::Private if is_tailscale_ipv4(&host) => Some(host.clone()),
            ServerMode::Private => env.tailscale_ipv4()?,
            ServerMode::Public => is_public_ipv4(&host).then(|| host.clone()),
        };
    }
    let Some(game_address) = game_address else {
        return Err(bad(if mode == ServerMode::Private {
            "pass --game-address <your Tailscale IPv4>"
        } else {
            "the host is a DNS name: pass --game-address <the server's public IPv4> as well (the game servers announce an address, not a name)"
        }));
    };
    let mongo_uri = text(&args.mongo_uri)
        .map(str::to_owned)
        .or_else(|| existing.map(|c| c.mongo.uri.clone()))
        .unwrap_or_else(|| DEFAULT_MONGO_URI.to_owned());
    let manage = if args.no_manage_mongo {
        false
    } else if args.manage_mongo {
        true
    } else {
        existing.map_or(mongo_uri == DEFAULT_MONGO_URI, |c| c.mongo.manage)
    };
    let registration = text(&args.registration)
        .map(str::to_uppercase)
        .or_else(|| existing.map(|c| c.registration.as_str().to_owned()))
        .unwrap_or_else(|| "INVITECODE".into());
    let game_arg = text(&args.game);
    if let Some(game) = game_arg
        && !is_absolute_any(game)
    {
        return Err(bad(format!(
            "--game needs the full path of the Dauntless 1.14.7 folder, like \"D:\\Games\\Dauntless\" (got {game})"
        )));
    }
    let game_dir = game_arg
        .map(str::to_owned)
        .or_else(|| existing.and_then(|c| c.game_dir.clone()));
    let mongod = match text(&args.mongod)
        .map(str::to_owned)
        .or_else(|| existing.and_then(|c| c.mongo.mongod.clone()))
    {
        Some(m) => Some(m),
        None if manage => env.find_mongod()?,
        None => None,
    };
    let max_hunts = match text(&args.max_hunts) {
        Some(raw) => number(raw),
        None => existing.map_or(3.0, |c| f64::from(c.max_hunt_servers)),
    };
    let multipliers = multipliers_from_args(
        args.off || args.no_multipliers,
        MultiplierOption {
            name: "xp-multiplier",
            value: text(&args.xp_multiplier),
        },
        MultiplierOption {
            name: "reward-multiplier",
            value: text(&args.reward_multiplier),
        },
        mode,
        same_mode.and_then(|c| c.multipliers),
    )?;
    let public_port = public_port_from_args(args, mode, existing)?;
    let performance = performance_from_args(args, existing.and_then(|c| c.performance))?;
    let linux = if env.platform() == Platform::Linux {
        Some(linux_from_args(args, existing.and_then(|c| c.linux.as_ref()), &|n| {
            env.find_on_path(n)
        })?)
    } else {
        existing.and_then(|c| c.linux.clone())
    };

    let opt = |v: Option<String>| v.map_or(Value::Null, Value::String);
    let mut doc = Map::new();
    doc.insert("version".into(), 1.into());
    doc.insert("mode".into(), mode.as_str().into());
    doc.insert("host".into(), host.into());
    doc.insert("gameAddress".into(), game_address.into());
    doc.insert(
        "name".into(),
        text(&args.name)
            .map(str::to_owned)
            .or_else(|| existing.map(|c| c.name.clone()))
            .unwrap_or_else(|| "Dauntless Reforged Revival server".into())
            .into(),
    );
    doc.insert("gameDir".into(), opt(game_dir.map(|d| resolve_path(&d))));
    doc.insert("registration".into(), registration.into());
    doc.insert(
        "shareUrl".into(),
        opt(if mode == ServerMode::Private {
            text(&args.share)
                .map(str::to_owned)
                .or_else(|| existing.and_then(|c| c.share_url.clone()))
        } else {
            None
        }),
    );
    doc.insert(
        "mongo".into(),
        json!({ "uri": mongo_uri, "manage": manage, "mongod": opt(mongod) }),
    );
    doc.insert("maxHuntServers".into(), number_value(max_hunts));
    if let Some(m) = multipliers {
        doc.insert(
            "multipliers".into(),
            json!({ "xp": number_value(m.xp), "rewards": number_value(m.rewards) }),
        );
    }
    if let Some(port) = public_port {
        doc.insert("publicPort".into(), number_value(port));
    }
    doc.insert(
        "performance".into(),
        json!({
            "serverFps": performance.server_fps,
            "idleFps": performance.idle_fps,
            "trainingOnDemand": performance.training_on_demand,
            "trainingIdleMinutes": performance.training_idle_minutes,
            "verboseGameLogs": performance.verbose_game_logs,
            "memoryGuard": performance.memory_guard,
            "minFreeMemoryMb": performance.min_free_memory_mb,
            "huntMemoryMb": performance.hunt_memory_mb,
        }),
    );
    if let Some(lx) = linux {
        doc.insert(
            "linux".into(),
            json!({
                "wine": opt(lx.wine),
                "proton": opt(lx.proton),
                "xvfb": lx.xvfb,
                "firewallInterface": opt(lx.firewall_interface),
            }),
        );
    }
    HostConfig::from_json(&Value::Object(doc))
}
