//! The signed server runtime update (`src/runtimeUpdater.ts`), off unless SERVER_RUNTIME_AUTO_UPDATE
//! is on (the host kit writes it off: Reforged builds the runtime on each server). The manifest names
//! one artifact for changelist 647472; it is installed only when its size, SHA-256, PE header and
//! Ed25519 signature check out, never while a Dauntless process runs, and atomically, keeping the old
//! DLL as `.bak`.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::Config;
use crate::log;

const TARGET_CHANGELIST: i64 = 647_472;
const MAX_RUNTIME_BYTES: usize = 200 * 1024 * 1024;
const DEFAULT_MANIFEST_URL: &str = "https://paradox.mysticfox.dev/launcher/v1/runtime/server/stable/windows-x86_64";
/// The Ed25519 key of `-----BEGIN PUBLIC KEY----- MCowBQYDK2VwAyEA3ZMtA7qU... -----END PUBLIC KEY-----`.
const RUNTIME_PUBLIC_KEY_SPKI: &str = "MCowBQYDK2VwAyEA3ZMtA7qUgs1F+1NQs2kmSG2zbOvXfjsh6+axI6eC/tc=";
const TIMEOUT: Duration = Duration::from_secs(30);

type Https = hyper_rustls::HttpsConnector<HttpConnector>;

fn client() -> &'static Client<Https, Empty<Bytes>> {
    static CLIENT: OnceLock<Client<Https, Empty<Bytes>>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let builder = hyper_rustls::HttpsConnectorBuilder::new();
        let builder = match builder.with_provider_and_webpki_roots(rustls::crypto::aws_lc_rs::default_provider()) {
            Ok(builder) => builder,
            Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
        };
        Client::builder(TokioExecutor::new()).build(builder.https_or_http().enable_http1().build())
    })
}

struct Reply {
    status: u16,
    content_length: Option<f64>,
    body: Bytes,
}

/// `fetch(url, { redirect: "error", signal: AbortSignal.timeout(30_000) })`.
async fn get(url: &Url) -> Result<Reply, String> {
    let work = async {
        let request = Request::get(url.as_str())
            .header("user-agent", "MysticParadox-DeployServer/0.0.3")
            .body(Empty::<Bytes>::new())
            .map_err(|e| e.to_string())?;
        let response = client()
            .request(request)
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Err("fetch failed: unexpected redirect".to_owned());
        }
        let status = response.status().as_u16();
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok());
        if content_length.is_some_and(|n| n > MAX_RUNTIME_BYTES as f64) {
            return Ok(Reply {
                status,
                content_length,
                body: Bytes::new(),
            });
        }
        let body = Limited::new(response.into_body(), MAX_RUNTIME_BYTES + 1)
            .collect()
            .await
            .map(|b| b.to_bytes())
            .map_err(|e| e.to_string())?;
        Ok(Reply {
            status,
            content_length,
            body,
        })
    };
    tokio::time::timeout(TIMEOUT, work)
        .await
        .map_err(|_| "The operation was aborted due to timeout".to_owned())?
}

fn validate_transport(url: &Url, allow_http: bool, expected_origin: Option<&url::Origin>) -> Result<(), String> {
    if url.scheme() != "https" && !(url.scheme() == "http" && allow_http) {
        return Err("Runtime updates require HTTPS (or SERVER_RUNTIME_ALLOW_HTTP=true for local development)".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Runtime update URLs may not contain credentials".into());
    }
    if expected_origin.is_some_and(|origin| &url.origin() != origin) {
        return Err("Runtime artifact URL must use the same origin as the manifest".into());
    }
    Ok(())
}

struct Manifest {
    version: String,
    size: usize,
    sha256: String,
    signature: String,
    url: String,
}

fn is_version(text: &str) -> bool {
    let (core, suffix) = match text.find(['-', '+']) {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && suffix
            .is_none_or(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
}

fn parse_manifest(value: &Value) -> Result<Manifest, String> {
    let invalid = || "Runtime update manifest failed validation".to_owned();
    let Value::Object(data) = value else {
        return Err("Runtime update manifest is not an object".into());
    };
    let text = |name: &str| data.get(name).and_then(Value::as_str);
    let size = data.get("size").and_then(Value::as_f64);
    // "MystPaxInternalServer" names the component in manifests signed before the 8 Oct 2026 rename.
    let ok = data.get("schema").and_then(Value::as_f64) == Some(1.0)
        && matches!(text("component"), Some("ParadoxRuntime" | "MystPaxInternalServer"))
        && text("target") == Some("server")
        && data.get("targetChangelist").and_then(Value::as_f64) == Some(TARGET_CHANGELIST as f64)
        && text("platform") == Some("windows-x86_64")
        && text("version").is_some_and(is_version)
        && text("channel").is_some_and(|c| {
            !c.is_empty()
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
        && size.is_some_and(|s| s.fract() == 0.0 && s > 0.0 && s <= MAX_RUNTIME_BYTES as f64)
        && text("sha256").is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        && text("signature").is_some()
        && text("url").is_some();
    if !ok {
        return Err(invalid());
    }
    Ok(Manifest {
        version: text("version").unwrap_or_default().to_owned(),
        size: size.unwrap_or(0.0) as usize,
        sha256: text("sha256").unwrap_or_default().to_owned(),
        signature: text("signature").unwrap_or_default().to_owned(),
        url: text("url").unwrap_or_default().to_owned(),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(windows)]
fn dauntless_running() -> Result<bool, String> {
    let output = std::process::Command::new("tasklist.exe")
        .args(["/FI", "IMAGENAME eq Dauntless-Win64-Shipping.exe", "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .to_lowercase()
        .contains("dauntless-win64-shipping.exe"))
}

#[cfg(not(windows))]
fn dauntless_running() -> Result<bool, String> {
    Ok(false)
}

fn replace_atomically(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    let io = |e: std::io::Error| format!("{}: {e}", destination.display());
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let staged = destination.with_file_name(format!(
        "{}.update-{}.tmp",
        destination.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    let backup = destination.with_file_name(format!(
        "{}.bak",
        destination.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&staged);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .and_then(|mut f| std::io::Write::write_all(&mut f, bytes))
        .map_err(io)?;
    let mut moved_old = false;
    let result = (|| {
        if destination.exists() {
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(destination, &backup)?;
            moved_old = true;
        }
        std::fs::rename(&staged, destination)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staged);
        if moved_old && !destination.exists() && backup.exists() {
            let _ = std::fs::rename(&backup, destination);
        }
        return Err(io(error));
    }
    Ok(())
}

fn verify_signature(bytes: &[u8], signature: &str) -> bool {
    let engine = base64::engine::general_purpose::STANDARD;
    let (Ok(spki), Ok(signature)) = (engine.decode(RUNTIME_PUBLIC_KEY_SPKI), engine.decode(signature)) else {
        return false;
    };
    if signature.len() != 64 || spki.len() < 32 {
        return false;
    }
    let key = aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::ED25519, &spki[spki.len() - 32..]);
    key.verify(bytes, &signature).is_ok()
}

pub async fn ensure_updated(config: &Config) -> Result<(), String> {
    let settings = &config.runtime_update;
    if !settings.auto_update {
        log::info("Server runtime auto-update is disabled");
        return Ok(());
    }
    let binary = config
        .gameserver_binary
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .ok_or("GAMESERVER_BINARY_PATH is required for server runtime updates")?;
    let dll = settings.dll_path.clone().unwrap_or_else(|| {
        let binary = std::path::absolute(binary).unwrap_or_else(|_| binary.into());
        binary.parent().unwrap_or(Path::new(".")).join("MysticParadox.dll")
    });
    let manifest_url = Url::parse(settings.manifest_url.as_deref().unwrap_or(DEFAULT_MANIFEST_URL))
        .map_err(|e| format!("Invalid URL: {e}"))?;
    validate_transport(&manifest_url, settings.allow_http, None)?;

    let reply = get(&manifest_url).await?;
    if reply.status == 404 || reply.status == 204 {
        log::info("No server runtime update is published");
        return Ok(());
    }
    if !(200..300).contains(&reply.status) {
        return Err(format!("Runtime manifest request failed with HTTP {}", reply.status));
    }
    let value: Value = serde_json::from_slice(&reply.body).map_err(|e| e.to_string())?;
    let manifest = parse_manifest(&value)?;

    if let Ok(current) = std::fs::read(&dll)
        && sha256_hex(&current) == manifest.sha256
    {
        log::info(&format!(
            "Server runtime is current (version {}, sha256 {})",
            manifest.version, manifest.sha256
        ));
        return Ok(());
    }
    if dauntless_running()? {
        return Err("A Dauntless process is running; refusing to replace the server runtime DLL".into());
    }

    let artifact_url = manifest_url
        .join(&manifest.url)
        .map_err(|e| format!("Invalid URL: {e}"))?;
    validate_transport(&artifact_url, settings.allow_http, Some(&manifest_url.origin()))?;
    let reply = get(&artifact_url).await?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("Runtime artifact request failed with HTTP {}", reply.status));
    }
    if reply.content_length.is_some_and(|n| n > MAX_RUNTIME_BYTES as f64) {
        return Err("Runtime artifact exceeds the maximum size".into());
    }
    let bytes = reply.body;
    if bytes.len() != manifest.size
        || bytes.len() > MAX_RUNTIME_BYTES
        || bytes.first() != Some(&0x4d)
        || bytes.get(1) != Some(&0x5a)
    {
        return Err("Runtime artifact size or PE header is invalid".into());
    }
    let actual = sha256_hex(&bytes);
    if actual != manifest.sha256 {
        return Err("Runtime artifact SHA-256 mismatch".into());
    }
    if !verify_signature(&bytes, &manifest.signature) {
        return Err("Runtime artifact signature verification failed".into());
    }
    replace_atomically(&dll, &bytes)?;
    log::info(&format!(
        "Installed signed server runtime update (version {}, sha256 {actual}, path {})",
        manifest.version,
        dll.display()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Value {
        json!({
            "schema": 1, "component": "ParadoxRuntime", "target": "server", "version": "1.2.3-rc.1",
            "channel": "stable", "targetChangelist": 647472, "platform": "windows-x86_64", "size": 10,
            "sha256": "a".repeat(64), "signature": "AA==", "url": "/x.dll"
        })
    }

    #[test]
    fn a_manifest_is_checked_field_by_field() {
        assert!(parse_manifest(&manifest()).is_ok());
        for (field, bad) in [
            ("schema", json!(2)),
            ("component", json!("Other")),
            ("targetChangelist", json!(1)),
            ("version", json!("1.2")),
            ("channel", json!("a b")),
            ("size", json!(0)),
            ("sha256", json!("A".repeat(64))),
            ("url", json!(null)),
        ] {
            let mut m = manifest();
            m[field] = bad;
            assert!(parse_manifest(&m).is_err(), "{field}");
        }
        assert!(parse_manifest(&json!([])).is_err());
    }

    #[test]
    fn transport_is_https_without_credentials_on_the_manifests_origin() {
        let manifest = Url::parse("https://a.example/m").unwrap();
        assert!(validate_transport(&manifest, false, None).is_ok());
        assert!(validate_transport(&Url::parse("http://a.example/m").unwrap(), false, None).is_err());
        assert!(validate_transport(&Url::parse("http://a.example/m").unwrap(), true, None).is_ok());
        assert!(validate_transport(&Url::parse("https://u:p@a.example/m").unwrap(), false, None).is_err());
        let other = Url::parse("https://b.example/x.dll").unwrap();
        assert!(validate_transport(&other, false, Some(&manifest.origin())).is_err());
        assert!(!verify_signature(b"MZ", "AA=="));
    }
}
