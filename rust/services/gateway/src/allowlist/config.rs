//! The helper's settings (`src/allowlist/config.ts`, and the Linux kit's `allowlist-core.mjs`),
//! read once at start; a bad one stops the start.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use service_kit::env::Env;

use super::firewall::NftSettings;
use crate::config::check_secret;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowlistConfig {
    pub bind_host: String,
    pub port: u16,
    pub secret: String,
    /// Only write what would change to the audit log; never touch the firewall
    pub dry_run: bool,
    pub allow_private: bool,
    pub ttl: Duration,
    pub min_interval: Duration,
    pub max_entries: usize,
    /// Local UDP ports the rule opens: "8770-8777" or "8777"
    pub ports: String,
    pub audit_log: PathBuf,
    pub state_file: PathBuf,
    pub nft: NftSettings,
}

fn int(env: &Env, name: &str, default: i64, min: i64, max: i64) -> Result<i64, String> {
    let Some(raw) = env.raw(name).filter(|r| !r.trim().is_empty()) else {
        return Ok(default);
    };
    let value = service_kit::js::number(Some(&json!(raw)));
    if value.fract() != 0.0 || value < min as f64 || value > max as f64 {
        return Err(format!("{name} must be a whole number from {min} to {max}"));
    }
    Ok(value as i64)
}

fn optional(env: &Env, name: &str) -> Option<String> {
    env.raw(name)
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
}

fn resolve(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// "8770-8777" or "8777": ports 1-65535, a range low to high, nothing else.
pub fn check_ports(text: &str) -> Result<String, String> {
    let shape = || {
        format!(
            "ALLOWLIST_PORTS must be a port or a range like 8770-8777 (got {})",
            json!(text)
        )
    };
    let digits = |s: &str| (1..=5).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    let (low, high) = match text.split_once('-') {
        Some((low, high)) if digits(low) && digits(high) => (low, Some(high)),
        None if digits(text) => (text, None),
        _ => return Err(shape()),
    };
    let low: u32 = low.parse().map_err(|_| shape())?;
    let high_value: u32 = high.map_or(Ok(low), str::parse).map_err(|_| shape())?;
    if low < 1 || high_value > 65535 || low > high_value {
        return Err("ALLOWLIST_PORTS must be within 1-65535, low to high".into());
    }
    Ok(if high.is_none() {
        low.to_string()
    } else {
        format!("{low}-{high_value}")
    })
}

impl AllowlistConfig {
    /// `LoadAllowlistConfig`.
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let dry_run = match optional(env, "ALLOWLIST_DRY_RUN").as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => {
                return Err("ALLOWLIST_DRY_RUN must be set: 1 only logs what it would do (any test machine), 0 really changes the firewall (the server)".into());
            }
        };
        let bind_host = optional(env, "ALLOWLIST_BIND").unwrap_or_else(|| "127.0.0.1".into());
        if bind_host != "127.0.0.1" && bind_host != "::1" {
            return Err("ALLOWLIST_BIND must be 127.0.0.1 or ::1: the helper runs with administrator rights and only the gateway on this machine may reach it".into());
        }
        Ok(Self {
            bind_host,
            port: int(env, "ALLOWLIST_PORT", 61005, 1, 65535)? as u16,
            secret: check_secret(optional(env, "ALLOWLIST_SECRET"), "ALLOWLIST_SECRET")?,
            dry_run,
            allow_private: env.raw("ALLOWLIST_ALLOW_PRIVATE") == Some("1"),
            ttl: Duration::from_secs(int(env, "ALLOWLIST_TTL_SECONDS", 600, 1, 86_400)? as u64),
            min_interval: Duration::from_millis(int(env, "ALLOWLIST_MIN_INTERVAL_MS", 3000, 50, 600_000)? as u64),
            max_entries: int(env, "ALLOWLIST_MAX_ENTRIES", 256, 1, 1000)? as usize,
            ports: check_ports(&optional(env, "ALLOWLIST_PORTS").unwrap_or_else(|| "8770-8777".into()))?,
            audit_log: resolve(&optional(env, "ALLOWLIST_AUDIT_LOG").unwrap_or_else(|| "allowlist-audit.log".into())),
            state_file: resolve(
                &optional(env, "ALLOWLIST_STATE_FILE").unwrap_or_else(|| "allowlist-state.json".into()),
            ),
            nft: NftSettings::from_env(env)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(pairs: &[(&str, &str)]) -> Result<AllowlistConfig, String> {
        let secret = "a".repeat(40);
        let mut all: Vec<(&str, &str)> = vec![("ALLOWLIST_SECRET", &secret), ("ALLOWLIST_DRY_RUN", "1")];
        all.retain(|(k, _)| !pairs.iter().any(|(p, _)| p == k));
        all.extend(pairs.iter().filter(|(_, v)| !v.is_empty()).copied());
        AllowlistConfig::from_env(&Env::from_pairs(all))
    }

    #[test]
    fn demands_an_explicit_dry_run_setting() {
        for value in ["", "yes"] {
            assert!(
                load(&[("ALLOWLIST_DRY_RUN", value)])
                    .unwrap_err()
                    .contains("ALLOWLIST_DRY_RUN")
            );
        }
        assert!(load(&[]).unwrap().dry_run);
        assert!(!load(&[("ALLOWLIST_DRY_RUN", "0")]).unwrap().dry_run);
    }

    #[test]
    fn listens_on_loopback_only_with_the_documented_defaults() {
        let config = load(&[]).unwrap();
        assert_eq!((config.bind_host.as_str(), config.port), ("127.0.0.1", 61005));
        assert_eq!(
            (config.ttl, config.min_interval),
            (Duration::from_secs(600), Duration::from_millis(3000))
        );
        assert_eq!(
            (config.ports.as_str(), config.allow_private, config.max_entries),
            ("8770-8777", false, 256)
        );
        for bind in ["0.0.0.0", "::", "192.168.1.5", "localhost"] {
            assert!(
                load(&[("ALLOWLIST_BIND", bind)])
                    .unwrap_err()
                    .contains("ALLOWLIST_BIND"),
                "{bind}"
            );
        }
        assert!(
            load(&[("ALLOWLIST_SECRET", "")])
                .unwrap_err()
                .contains("ALLOWLIST_SECRET")
        );
        for ports in ["8777-8770", "0-5", "8770-70000", "8770,8771", "any"] {
            assert!(
                load(&[("ALLOWLIST_PORTS", ports)])
                    .unwrap_err()
                    .contains("ALLOWLIST_PORTS"),
                "{ports}"
            );
        }
        assert_eq!(check_ports("8777").unwrap(), "8777");
        assert_eq!(check_ports("08770-08777").unwrap(), "8770-8777");
    }
}
