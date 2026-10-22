//! Settings from the environment (`src/config.ts`), read once at start; a bad one stops the start.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::json;
use service_kit::env::Env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub port: u16,
    pub bind_hosts: Vec<String>,
    pub allow_any_bind: bool,
    pub metagame_url: String,
    pub game_dir: PathBuf,
    /// `CONTENT_MANIFEST`, or `None` for the manifest built into the binary
    pub manifest_path: Option<PathBuf>,
    pub download_base_url: Option<String>,
    pub branding_dir: Option<PathBuf>,
    pub news_file: Option<PathBuf>,
    pub news_url: Option<String>,
    pub max_streams_per_account: usize,
    pub max_streams_total: usize,
    pub auth_cache_seconds: u64,
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

/// `path.resolve`.
fn resolve(path: &str) -> PathBuf {
    std::path::absolute(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path))
}

/// Addresses a friend can only reach through the VPN: loopback, and Tailscale's own ranges
/// (100.64.0.0/10, fd7a:115c:a1e0::/48). Anything else, 0.0.0.0 included, needs
/// `CONTENT_ALLOW_ANY_BIND=1`: game files must never be reachable from the internet.
pub fn is_private_bind_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            let [a, b, _, _] = v4.octets();
            a == 127 || (a == 100 && (64..=127).contains(&b))
        }
        Ok(IpAddr::V6(_)) => {
            let lower = host.to_lowercase();
            lower == "::1" || lower.starts_with("fd7a:115c:a1e0:")
        }
        Err(_) => false,
    }
}

impl Config {
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let port = int(env, "PORT", 61002, 1, 65535)? as u16;
        let bind_hosts: Vec<String> = optional(env, "BIND_HOST")
            .unwrap_or_else(|| "127.0.0.1".into())
            .split(',')
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty())
            .collect();
        if bind_hosts.is_empty() {
            return Err("BIND_HOST is empty".into());
        }
        for host in &bind_hosts {
            if host != "localhost" && host.parse::<IpAddr>().is_err() {
                return Err(format!(
                    "BIND_HOST entries must be IP addresses (got {})",
                    service_kit::jsjson::stringify(&json!(host))
                ));
            }
        }
        let allow_any_bind = env.raw("CONTENT_ALLOW_ANY_BIND") == Some("1");
        if !allow_any_bind {
            let public: Vec<&str> = bind_hosts
                .iter()
                .filter(|h| !is_private_bind_host(h))
                .map(String::as_str)
                .collect();
            if !public.is_empty() {
                return Err(format!(
                    "Refusing to listen on {}: only loopback and Tailscale addresses (100.64.0.0/10) are allowed. Set CONTENT_ALLOW_ANY_BIND=1 only if a firewall keeps the internet out.",
                    public.join(", ")
                ));
            }
        }
        let metagame_url = optional(env, "METAGAME_URL")
            .unwrap_or_else(|| "http://127.0.0.1:61000".into())
            .trim_end_matches('/')
            .to_owned();
        let parsed = url::Url::parse(&metagame_url).map_err(|_| "METAGAME_URL is not a URL".to_owned())?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err("METAGAME_URL must be http:// or https://".into());
        }
        let game_dir = optional(env, "CONTENT_GAME_DIR")
            .ok_or_else(|| "CONTENT_GAME_DIR is not set (the folder that contains Archon\\)".to_owned())?;
        let download_base_url = match optional(env, "CONTENT_DOWNLOAD_BASE_URL") {
            None => None,
            Some(base) => {
                let url = url::Url::parse(&base).map_err(|e| format!("CONTENT_DOWNLOAD_BASE_URL: {e}"))?;
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(
                        "CONTENT_DOWNLOAD_BASE_URL must be HTTPS without credentials, query or fragment".into(),
                    );
                }
                Some(format!("{}/", url.as_str().trim_end_matches('/')))
            }
        };
        let news_url = crate::news::github_news_url(env.raw("CONTENT_NEWS_URL").filter(|u| !u.is_empty()))?;
        Ok(Self {
            port,
            bind_hosts,
            allow_any_bind,
            metagame_url,
            game_dir: resolve(&game_dir),
            manifest_path: optional(env, "CONTENT_MANIFEST").map(|p| resolve(&p)),
            download_base_url,
            branding_dir: optional(env, "CONTENT_BRANDING_DIR").map(|p| resolve(&p)),
            news_file: optional(env, "CONTENT_NEWS_FILE").map(|p| resolve(&p)),
            news_url,
            max_streams_per_account: int(env, "CONTENT_MAX_STREAMS_PER_ACCOUNT", 6, 1, 64)? as usize,
            max_streams_total: int(env, "CONTENT_MAX_STREAMS_TOTAL", 48, 1, 1024)? as usize,
            auth_cache_seconds: int(env, "CONTENT_AUTH_CACHE_SECONDS", 300, 0, 3600)? as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(pairs: &[(&str, &str)]) -> Result<Config, String> {
        Config::from_env(&Env::from_pairs(pairs.iter().copied()))
    }

    #[test]
    fn defaults() {
        let c = load(&[("CONTENT_GAME_DIR", "C:\\Games\\Dauntless")]).unwrap();
        assert_eq!(c.port, 61002);
        assert_eq!(c.bind_hosts, ["127.0.0.1"]);
        assert_eq!(c.metagame_url, "http://127.0.0.1:61000");
        assert_eq!(c.max_streams_per_account, 6);
        assert_eq!(c.auth_cache_seconds, 300);
        assert_eq!((c.branding_dir, c.news_file), (None, None));
    }

    #[test]
    fn the_game_folder_is_required() {
        assert!(load(&[]).unwrap_err().contains("CONTENT_GAME_DIR"));
    }

    #[test]
    fn the_cdn_base_is_https_without_credentials_query_or_fragment() {
        assert_eq!(load(&[("CONTENT_GAME_DIR", "x")]).unwrap().download_base_url, None);
        assert_eq!(
            load(&[
                ("CONTENT_GAME_DIR", "x"),
                ("CONTENT_DOWNLOAD_BASE_URL", "https://downloads.example/game")
            ])
            .unwrap()
            .download_base_url
            .as_deref(),
            Some("https://downloads.example/game/")
        );
        for bad in [
            "http://example/game",
            "https://u:p@example/game",
            "https://example/game?q=1",
            "https://example/game#x",
        ] {
            assert!(
                load(&[("CONTENT_GAME_DIR", "x"), ("CONTENT_DOWNLOAD_BASE_URL", bad)]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn bind_addresses_are_loopback_and_tailscale_unless_allowed() {
        for ok in [
            "127.0.0.1",
            "::1",
            "localhost",
            "100.64.0.1",
            "100.101.102.103",
            "100.127.255.254",
            "fd7a:115c:a1e0::1",
        ] {
            assert!(is_private_bind_host(ok), "{ok}");
        }
        for bad in [
            "0.0.0.0",
            "::",
            "192.168.1.10",
            "10.0.0.5",
            "100.63.255.255",
            "100.128.0.1",
            "8.8.8.8",
            "fe80::1",
        ] {
            assert!(!is_private_bind_host(bad), "{bad}");
        }
        assert_eq!(
            load(&[("CONTENT_GAME_DIR", "x"), ("BIND_HOST", "127.0.0.1, 100.100.1.2")])
                .unwrap()
                .bind_hosts,
            ["127.0.0.1", "100.100.1.2"]
        );
        assert!(
            load(&[("CONTENT_GAME_DIR", "x"), ("BIND_HOST", "0.0.0.0")])
                .unwrap_err()
                .contains("Refusing")
        );
        assert!(
            load(&[("CONTENT_GAME_DIR", "x"), ("BIND_HOST", "my-host.example")])
                .unwrap_err()
                .contains("IP addresses")
        );
        assert_eq!(
            load(&[
                ("CONTENT_GAME_DIR", "x"),
                ("BIND_HOST", "0.0.0.0"),
                ("CONTENT_ALLOW_ANY_BIND", "1")
            ])
            .unwrap()
            .bind_hosts,
            ["0.0.0.0"]
        );
    }

    #[test]
    fn numbers_and_urls_are_checked() {
        assert!(
            load(&[("CONTENT_GAME_DIR", "x"), ("PORT", "70000")])
                .unwrap_err()
                .contains("PORT")
        );
        assert!(
            load(&[("CONTENT_GAME_DIR", "x"), ("CONTENT_MAX_STREAMS_PER_ACCOUNT", "0")])
                .unwrap_err()
                .contains("STREAMS")
        );
        assert!(
            load(&[("CONTENT_GAME_DIR", "x"), ("METAGAME_URL", "ftp://x")])
                .unwrap_err()
                .contains("METAGAME_URL")
        );
        assert_eq!(
            load(&[
                ("CONTENT_GAME_DIR", "x"),
                ("METAGAME_URL", "http://100.64.0.1:61000///")
            ])
            .unwrap()
            .metagame_url,
            "http://100.64.0.1:61000"
        );
    }
}
