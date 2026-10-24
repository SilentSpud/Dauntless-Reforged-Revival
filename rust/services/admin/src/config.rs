//! Settings from `admin.env`, which `reforged-host setup` writes (`hostconfig.rs`); read once at
//! start, and a bad one stops the start.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use service_kit::env::Env;

/// The backend's checks, matched here so a short secret fails at start rather than on every call.
const MIN_SECRET: usize = 32;
/// `paths::MAX_HUNTS` in the host kit: UDP 8766-8777 at most.
const MAX_HUNTS: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub port: u16,
    pub bind_hosts: Vec<String>,
    pub backend_url: String,
    pub director_url: String,
    /// `REFORGED_ADMIN_API_KEY`: what the bot sends, `Authorization: Bearer <key>`
    pub api_key: String,
    /// `REFORGED_ADMIN_KEY`: the backend's `/reforged/api` admin routes (the kick)
    pub admin_key: String,
    /// `MYSTICPARADOX_MANAGER_OPERATOR_SECRET`: signs the calls to the backend's `/admin/v1`
    pub operator_secret: String,
    pub timeouts_file: PathBuf,
    pub max_hunt_servers: u32,
    pub server_mode: ServerMode,
}

fn optional(env: &Env, name: &str) -> Option<String> {
    env.raw(name)
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
}

fn secret(env: &Env, name: &str) -> Result<String, String> {
    match optional(env, name) {
        Some(value) if value.chars().count() >= MIN_SECRET => Ok(value),
        Some(_) => Err(format!("{name} must be at least {MIN_SECRET} characters")),
        None => Err(format!("{name} is not set: run reforged-host setup")),
    }
}

/// `localhost`, any 127.x address or `::1`, as the content server's check.
pub fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// An `http://` URL on this machine, without its trailing slashes.
fn local_url(env: &Env, name: &str, default: &str) -> Result<String, String> {
    let url = optional(env, name)
        .unwrap_or_else(|| default.into())
        .trim_end_matches('/')
        .to_owned();
    let parsed = url::Url::parse(&url).map_err(|_| format!("{name} is not a URL"))?;
    let host = parsed.host_str().unwrap_or("");
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if parsed.scheme() != "http" || !is_loopback_host(host) || parsed.path() != "/" || parsed.query().is_some() {
        return Err(format!(
            "{name} must be http://<address>:<port> on this machine (the admin keys travel in it)"
        ));
    }
    Ok(url)
}

impl Config {
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let port = match optional(env, "PORT") {
            None => 61003,
            Some(raw) => raw
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("PORT must be a whole number from 1 to 65535")?,
        };
        let bind_hosts: Vec<String> = optional(env, "BIND_HOST")
            .unwrap_or_else(|| "127.0.0.1".into())
            .split(',')
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty())
            .collect();
        if bind_hosts.is_empty() {
            return Err("BIND_HOST is empty".into());
        }
        if let Some(host) = bind_hosts.iter().find(|h| !is_loopback_host(h)) {
            return Err(format!(
                "BIND_HOST must be a loopback address (got {host}): the admin API is for tools on this machine"
            ));
        }
        let max_hunt_servers = match optional(env, "REFORGED_MAX_HUNT_SERVERS") {
            None => 3,
            Some(raw) => raw
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_HUNTS).contains(n))
                .ok_or(format!("REFORGED_MAX_HUNT_SERVERS must be 1 to {MAX_HUNTS}"))?,
        };
        let server_mode = match optional(env, "REFORGED_SERVER_MODE").as_deref() {
            None | Some("private") => ServerMode::Private,
            Some("public") => ServerMode::Public,
            Some(_) => return Err("REFORGED_SERVER_MODE must be private or public".into()),
        };
        let timeouts_file = optional(env, "REFORGED_ADMIN_API_TIMEOUTS_FILE")
            .ok_or("REFORGED_ADMIN_API_TIMEOUTS_FILE is not set: run reforged-host setup")?;
        Ok(Self {
            port,
            bind_hosts,
            backend_url: local_url(env, "BACKEND_URL", "http://127.0.0.1:61000")?,
            director_url: local_url(env, "REFORGED_DIRECTOR_URL", "http://127.0.0.1:61001")?,
            api_key: secret(env, "REFORGED_ADMIN_API_KEY")?,
            admin_key: secret(env, "REFORGED_ADMIN_KEY")?,
            operator_secret: secret(env, "MYSTICPARADOX_MANAGER_OPERATOR_SECRET")?,
            timeouts_file: std::path::absolute(Path::new(&timeouts_file)).unwrap_or_else(|_| timeouts_file.into()),
            max_hunt_servers,
            server_mode,
        })
    }

    /// The address a bind host listens on (`localhost` is 127.0.0.1).
    pub fn bind_address(host: &str) -> Option<IpAddr> {
        if host == "localhost" {
            return Some(IpAddr::from([127, 0, 0, 1]));
        }
        host.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn load(extra: &[(&str, &str)]) -> Result<Config, String> {
        let mut pairs = vec![
            ("REFORGED_ADMIN_API_KEY", KEY),
            ("REFORGED_ADMIN_KEY", KEY),
            ("MYSTICPARADOX_MANAGER_OPERATOR_SECRET", KEY),
            ("REFORGED_ADMIN_API_TIMEOUTS_FILE", "timeouts.json"),
        ];
        pairs.extend_from_slice(extra);
        Config::from_env(&Env::from_pairs(pairs))
    }

    #[test]
    fn has_defaults_for_everything_but_the_secrets_and_the_file() {
        let c = load(&[]).unwrap();
        assert_eq!(c.port, 61003);
        assert_eq!(c.bind_hosts, ["127.0.0.1"]);
        assert_eq!(c.backend_url, "http://127.0.0.1:61000");
        assert_eq!(c.director_url, "http://127.0.0.1:61001");
        assert_eq!(c.max_hunt_servers, 3);
        assert_eq!(c.server_mode, ServerMode::Private);
        assert!(c.timeouts_file.is_absolute());
        for name in [
            "REFORGED_ADMIN_API_KEY",
            "REFORGED_ADMIN_KEY",
            "MYSTICPARADOX_MANAGER_OPERATOR_SECRET",
            "REFORGED_ADMIN_API_TIMEOUTS_FILE",
        ] {
            assert!(load(&[(name, " ")]).unwrap_err().contains(name), "{name}");
        }
        assert!(load(&[("REFORGED_ADMIN_API_KEY", "short")]).unwrap_err().contains("32"));
    }

    #[test]
    fn listens_and_calls_on_loopback_only() {
        for bind in ["0.0.0.0", "100.101.102.103", "::", "my-host.example"] {
            assert!(load(&[("BIND_HOST", bind)]).unwrap_err().contains("loopback"), "{bind}");
        }
        assert_eq!(
            load(&[("BIND_HOST", "127.0.0.1, ::1")]).unwrap().bind_hosts,
            ["127.0.0.1", "::1"]
        );
        for url in [
            "http://203.0.113.7:61000",
            "https://127.0.0.1:61000",
            "http://127.0.0.1:61000/api",
            "nope",
        ] {
            assert!(load(&[("BACKEND_URL", url)]).is_err(), "{url}");
            assert!(load(&[("REFORGED_DIRECTOR_URL", url)]).is_err(), "{url}");
        }
        assert_eq!(
            load(&[("BACKEND_URL", "http://[::1]:61000//")]).unwrap().backend_url,
            "http://[::1]:61000"
        );
    }

    #[test]
    fn numbers_and_the_mode_are_checked() {
        assert!(load(&[("PORT", "0")]).is_err());
        assert!(load(&[("REFORGED_MAX_HUNT_SERVERS", "11")]).is_err());
        assert_eq!(
            load(&[("REFORGED_MAX_HUNT_SERVERS", "10")]).unwrap().max_hunt_servers,
            10
        );
        assert_eq!(
            load(&[("REFORGED_SERVER_MODE", "public")]).unwrap().server_mode,
            ServerMode::Public
        );
        assert!(load(&[("REFORGED_SERVER_MODE", "PUBLIC")]).is_err());
    }
}
