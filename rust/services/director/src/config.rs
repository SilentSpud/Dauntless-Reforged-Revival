//! The Director's settings, read as the Node Director reads `process.env` (`src/server.ts`,
//! `src/controllers/gameservers.ts`, `src/runtimeUpdater.ts`): numbers with JavaScript's `Number()`,
//! so an empty value is 0 and text is NaN, which the comparisons below treat as Node does. The one
//! difference: PORT and the port range must be whole port numbers, where Node would listen on a
//! random port or offer no hunt ports at all.

use std::path::{Path, PathBuf};

use serde_json::Value;
use service_kit::env::Env;
use service_kit::js;

/// 1, true, yes or on, any case, trimmed.
pub fn enabled(value: Option<&str>) -> bool {
    value
        .map(|v| js::trim(v).to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

fn off(value: &str) -> bool {
    matches!(
        js::trim(value).to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// `Number(process.env[name] ?? fallback)`.
fn number(env: &Env, name: &str, fallback: &str) -> f64 {
    js::number(Some(&Value::String(env.raw(name).unwrap_or(fallback).to_owned())))
}

fn port(env: &Env, name: &str) -> Result<u16, String> {
    let raw = env.raw(name).unwrap_or("");
    js::trim(raw)
        .parse::<u16>()
        .map_err(|_| format!("{name} must be a port number (got '{raw}')"))
}

pub const DEFAULT_LOG_CMDS: &str = "LogPhoenixCharacter VeryVerbose, LogOnline Verbose, LogLoad Off";

#[derive(Debug, Clone)]
pub struct RuntimeUpdate {
    pub auto_update: bool,
    pub required: bool,
    pub allow_http: bool,
    pub dll_path: Option<PathBuf>,
    pub manifest_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub bind_host: String,
    pub port_range_begin: u16,
    pub port_range_end: u16,
    pub gameserver_binary: Option<String>,
    /// GAMESERVER_LOG_CMDS when it is valid; a warning is logged at start when it is not.
    pub log_cmds: String,
    pub log_cmds_invalid: bool,
    pub metagame_api_key: Option<String>,
    pub my_ip: Option<String>,
    pub seconds_between_startups: f64,
    pub ready_timeout_ms: f64,
    pub hub_prompt_restart: bool,
    pub training_hub_at_boot: bool,
    pub ramsgate_hub_enabled: bool,
    pub training_idle_stop_minutes: Option<String>,
    pub runtime_log_dir: String,
    pub max_concurrent_hunt_servers: f64,
    pub enable_public_hunt_reuse: bool,
    pub public_hunt_max_players: f64,
    pub gameserver_log_dir: PathBuf,
    pub windows_hide: bool,
    pub game_data_dir: PathBuf,
    pub trials_rotation_minutes: Option<String>,
    pub p2p_module: bool,
    pub runtime_update: RuntimeUpdate,
    pub log_level: Option<String>,
    pub memory: crate::memory::Options,
    pub memory_problems: Vec<String>,
    /// Keys still named MYSTPAX_ (renamed MYSTICPARADOX_ on 2026-10-08), sorted.
    pub legacy_keys: Vec<String>,
    /// What every game server inherits: the process environment and the env file.
    pub child_env: Vec<(String, String)>,
}

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

impl Config {
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|e| format!("the working folder: {e}"))?;
        let raw_log_cmds = js::trim(env.raw("GAMESERVER_LOG_CMDS").unwrap_or("")).to_owned();
        let log_cmds_valid = (1..=512).contains(&raw_log_cmds.len())
            && raw_log_cmds
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b',' || b == b' ');
        let training_on_demand = enabled(env.raw("TRAINING_ON_DEMAND"));
        let mut legacy_keys: Vec<String> = env
            .vars()
            .filter(|(k, _)| k.starts_with("MYSTPAX_"))
            .map(|(k, _)| k.to_owned())
            .collect();
        legacy_keys.sort();
        let mut child_env: Vec<(String, String)> = env.vars().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        child_env.sort();
        let (memory, memory_problems) = crate::memory::options_from_env(env);
        let text = |name: &str| env.raw(name).map(|v| js::trim(v).to_owned()).filter(|v| !v.is_empty());
        Ok(Self {
            port: port(env, "PORT")?,
            bind_host: text("BIND_HOST").unwrap_or_else(|| "127.0.0.1".to_owned()),
            port_range_begin: port(env, "PORT_RANGE_BEGIN")?,
            port_range_end: port(env, "PORT_RANGE_END")?,
            gameserver_binary: env.raw("GAMESERVER_BINARY_PATH").map(str::to_owned),
            log_cmds: if log_cmds_valid {
                raw_log_cmds.clone()
            } else {
                DEFAULT_LOG_CMDS.to_owned()
            },
            log_cmds_invalid: !raw_log_cmds.is_empty() && !log_cmds_valid,
            metagame_api_key: env.raw("METAGAME_API_KEY").map(str::to_owned),
            my_ip: env.raw("MY_IP").map(str::to_owned),
            seconds_between_startups: number(env, "SECONDS_TO_WAIT_BETWEEN_GAMESERVER_STARTUP", "undefined"),
            ready_timeout_ms: number(env, "GAMESERVER_READY_TIMEOUT_MS", "30000"),
            hub_prompt_restart: !off(env.raw("HUB_PROMPT_RESTART").unwrap_or("1")),
            training_hub_at_boot: crate::hub::switch_on(env.raw("TRAINING_HUB"), !training_on_demand),
            ramsgate_hub_enabled: crate::hub::switch_on(env.raw("RAMSGATE_HUB"), true),
            training_idle_stop_minutes: env.raw("TRAINING_IDLE_STOP_MINUTES").map(str::to_owned),
            runtime_log_dir: js::trim(env.raw("GAMESERVER_RUNTIME_LOG_DIR").unwrap_or("")).to_owned(),
            max_concurrent_hunt_servers: number(env, "MAX_CONCURRENT_HUNT_SERVERS", "3"),
            enable_public_hunt_reuse: env.raw("ENABLE_PUBLIC_HUNT_REUSE").unwrap_or("0") == "1",
            public_hunt_max_players: number(env, "PUBLIC_HUNT_MAX_PLAYERS", "4"),
            gameserver_log_dir: env.raw("GAMESERVER_LOG_DIR").map_or_else(
                || cwd.join("..").join("debug").join("gameservers"),
                |dir| resolve(&cwd, dir),
            ),
            windows_hide: env.raw("GAMESERVER_WINDOWS_HIDE").unwrap_or("1") != "0",
            // Node: the folder beside dist/, which is the Director's folder, where start runs it.
            game_data_dir: env
                .get("PARADOX_GAME_DATA_DIR")
                .map_or_else(|| cwd.join("game-data"), |dir| resolve(&cwd, dir)),
            trials_rotation_minutes: env.raw("TRIALS_ROTATION_MINUTES").map(str::to_owned),
            p2p_module: !env.raw("MYSTICPARADOX_P2P_MODULE").is_some_and(off),
            runtime_update: RuntimeUpdate {
                auto_update: enabled(env.raw("SERVER_RUNTIME_AUTO_UPDATE")),
                required: enabled(env.raw("SERVER_RUNTIME_UPDATE_REQUIRED")),
                allow_http: enabled(env.raw("SERVER_RUNTIME_ALLOW_HTTP")),
                dll_path: text("GAMESERVER_RUNTIME_DLL_PATH").map(|p| resolve(&cwd, &p)),
                manifest_url: text("SERVER_RUNTIME_MANIFEST_URL"),
            },
            log_level: env.get("LOG_LEVEL").map(str::to_owned),
            memory,
            memory_problems,
            legacy_keys,
            child_env,
        })
    }
}
