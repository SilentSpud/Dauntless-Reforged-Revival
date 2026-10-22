//! The helper itself (`src/allowlist/helper.ts`): the loopback HTTP endpoint the gateway reports
//! logged-in addresses to, the expiry of those addresses, the audit log and the saved state.

use std::convert::Infallible;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Map, Value, json};
use service_kit::linelog::{self, Throttle};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::{Notify, Semaphore, watch};

use super::config::AllowlistConfig;
use super::firewall::{Backend, plan};
use super::state::{AllowlistState, Refresh};
use super::sync::{Apply, ApplyResult, FirewallSync, OnResult};
use crate::cert::iso;
use crate::feed::Clock;
use crate::ip::{AllowlistCheck, IpClass, check_allowlist_ip, parse_ip};

const MAX_BODY: usize = 1024;
const MAX_CONNECTIONS: usize = 64;

fn iso_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(iso).unwrap_or_default()
}

/// Append-only JSON lines: every address added or expired, every refusal, every rule change (with
/// what a dry run would have done) and every failed secret. Never the secret itself.
pub struct AuditLog {
    file: Option<PathBuf>,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(file: Option<PathBuf>) -> Self {
        if let Some(dir) = file.as_deref().and_then(Path::parent) {
            let _ = std::fs::create_dir_all(dir);
        }
        Self {
            file,
            lock: Mutex::new(()),
        }
    }

    pub fn write(&self, event: &str, fields: Value) {
        let mut line = Map::new();
        line.insert("t".into(), json!(iso(chrono::Utc::now())));
        line.insert("event".into(), json!(event));
        if let Value::Object(fields) = &fields {
            line.extend(fields.clone());
        }
        if let Some(file) = &self.file {
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)
                .and_then(|mut f| writeln!(f, "{}", Value::Object(line)));
            if let Err(error) = written {
                linelog::error(
                    "audit log write failed",
                    json!({ "file": file.display().to_string(), "error": error.to_string() }),
                );
            }
        }
        linelog::info(&format!("allowlist {event}"), fields);
    }
}

/// What applies one desired set: a dry run writes it to the audit log, a real backend changes the
/// firewall.
pub fn applier(backend: Arc<Backend>, audit: Arc<AuditLog>, ports: String, dry_run: bool) -> Apply {
    Arc::new(move |ips| {
        let (backend, audit, ports) = (backend.clone(), audit.clone(), ports.clone());
        Box::pin(async move {
            let plan = plan(&ips, &ports)?;
            if dry_run {
                let mut fields = backend.describe(&plan);
                fields["backend"] = json!(backend.name());
                audit.write("dry_run", fields);
                return Ok(());
            }
            backend.apply(&plan).await
        })
    })
}

fn secret_matches(given: Option<&[u8]>, expected: &str) -> bool {
    let Some(given) = given else {
        return false;
    };
    // Hashing first gives equal lengths, so the comparison time says nothing about the secret
    let a = Sha256::digest(given);
    let b = Sha256::digest(expected.as_bytes());
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn reply(status: u16, body: &Value, close: bool) -> Response<Full<Bytes>> {
    let text = Bytes::from(body.to_string());
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("content-length", text.len())
        .header("cache-control", "no-store");
    if close {
        response = response.header("connection", "close");
    }
    response
        .body(Full::new(text))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

/// What a refused value looks like in the audit log: a string's first 64 characters, printable
/// ASCII only, or JavaScript's `typeof` of anything else.
fn shown(given: Option<&Value>) -> String {
    match given {
        Some(Value::String(text)) => text
            .chars()
            .take(64)
            .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
            .collect(),
        Some(Value::Number(_)) => "number".into(),
        Some(Value::Bool(_)) => "boolean".into(),
        Some(Value::Null | Value::Object(_) | Value::Array(_)) => "object".into(),
        None => "undefined".into(),
    }
}

pub struct Helper {
    config: AllowlistConfig,
    backend: Arc<Backend>,
    audit: Arc<AuditLog>,
    state: Mutex<AllowlistState>,
    sync: FirewallSync,
    now: Clock,
    throttle: Throttle,
    dirty: AtomicBool,
    expiry: Notify,
    stop: watch::Sender<bool>,
}

pub struct Running {
    pub port: u16,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Helper {
    pub fn new(config: AllowlistConfig, backend: Backend, audit: Arc<AuditLog>, now: Clock) -> Arc<Self> {
        let backend = Arc::new(backend);
        Arc::new_cyclic(|this: &std::sync::Weak<Self>| {
            let apply = applier(backend.clone(), audit.clone(), config.ports.clone(), config.dry_run);
            let on_result: OnResult = {
                let this = this.clone();
                Arc::new(move |result: &ApplyResult| {
                    if let Some(helper) = this.upgrade() {
                        helper.applied(result);
                    }
                })
            };
            // The interval between rule changes is real time, whatever clock the entries use
            let sync = FirewallSync::new(apply, config.min_interval, on_result);
            Self {
                state: Mutex::new(AllowlistState::new(config.ttl.as_millis() as i64, config.max_entries)),
                config,
                backend,
                audit,
                sync,
                now,
                throttle: Throttle::new(60_000),
                dirty: AtomicBool::new(false),
                expiry: Notify::new(),
                stop: watch::Sender::new(false),
            }
        })
    }

    pub fn sync(&self) -> &FirewallSync {
        &self.sync
    }

    pub fn list(&self) -> Vec<String> {
        self.lock().list()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AllowlistState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn applied(&self, result: &ApplyResult) {
        if result.ok {
            self.audit.write(
                "rule_applied",
                json!({ "enabled": !result.ips.is_empty(), "ips": result.ips, "dryRun": self.config.dry_run, "ms": result.ms }),
            );
        } else {
            self.audit.write(
                "rule_failed",
                json!({ "enabled": !result.ips.is_empty(), "ips": result.ips, "error": result.error }),
            );
        }
    }

    /// Restores unexpired entries from the state file, then brings the rule to that set (an empty
    /// set closes the ports): the rule never keeps addresses from before a crash. Then listens.
    pub async fn start(self: &Arc<Self>) -> Result<Running, String> {
        let mut restored = 0;
        match std::fs::read_to_string(&self.config.state_file) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(saved) => {
                    let allow_private = self.config.allow_private;
                    restored = self.lock().load(&saved["entries"], (self.now)(), |ip| {
                        matches!(
                            check_allowlist_ip(Some(&ip.canonical), allow_private),
                            AllowlistCheck::Ok(_)
                        )
                    });
                }
                Err(error) => self.state_unreadable(&error.to_string()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => self.state_unreadable(&error.to_string()),
        }
        self.audit.write(
            "start",
            json!({
                "dryRun": self.config.dry_run,
                "restored": restored,
                "ttlSeconds": self.config.ttl.as_secs(),
                "ports": self.config.ports,
                "allowPrivate": self.config.allow_private,
                "backend": self.backend.name(),
            }),
        );
        self.sync.request(self.list());

        let address = SocketAddr::new(
            self.config.bind_host.parse::<IpAddr>().map_err(|e| e.to_string())?,
            self.config.port,
        );
        let listener = TcpListener::bind(address).await.map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let tasks = vec![
            tokio::spawn(self.clone().expire_loop()),
            tokio::spawn(self.clone().save_loop()),
            tokio::spawn(self.clone().serve(listener)),
        ];
        Ok(Running { port, tasks })
    }

    fn state_unreadable(&self, error: &str) {
        linelog::warn(
            "allowlist state file unreadable; starting empty",
            json!({ "file": self.config.state_file.display().to_string(), "error": error }),
        );
    }

    /// Stops listening, closes the ports (fail closed) and saves the live set, so a restart opens
    /// them again for the same players right away.
    pub async fn close(&self, running: Running) {
        self.stop.send_replace(true);
        for task in running.tasks {
            task.abort();
            let _ = task.await;
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.save_if_dirty();
        self.sync.finish(Vec::new()).await;
        self.audit.write("stop", json!({ "saved": self.lock().len() }));
    }

    fn save_if_dirty(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let entries: Vec<Value> = self
            .lock()
            .snapshot()
            .into_iter()
            .map(|(ip, at)| json!({ "ip": ip, "expiresAt": at }))
            .collect();
        let text = json!({ "version": 1, "entries": entries }).to_string();
        let file = &self.config.state_file;
        let mut temp = file.clone().into_os_string();
        temp.push(".tmp");
        let temp = PathBuf::from(temp);
        let saved = write_private(&temp, text.as_bytes()).and_then(|()| std::fs::rename(&temp, file));
        if let Err(error) = saved {
            linelog::warn(
                "allowlist state file not saved",
                json!({ "file": file.display().to_string(), "error": error.to_string() }),
            );
        }
    }

    async fn save_loop(self: Arc<Self>) {
        let mut every = tokio::time::interval(Duration::from_secs(30));
        every.tick().await;
        loop {
            every.tick().await;
            self.save_if_dirty();
        }
    }

    async fn expire_loop(self: Arc<Self>) {
        loop {
            let woken = self.expiry.notified();
            let next = self.lock().next_expiry();
            match next {
                None => woken.await,
                Some(at) => {
                    let wait = Duration::from_millis((at - (self.now)()).max(0) as u64 + 5);
                    tokio::select! {
                        () = tokio::time::sleep(wait) => self.expire_now(),
                        () = woken => {}
                    }
                }
            }
        }
    }

    /// Drops the addresses whose time is up (also called by tests with a moved clock).
    pub fn expire_now(&self) {
        let gone = self.lock().expire((self.now)());
        for ip in &gone {
            self.audit.write("expired", json!({ "ip": ip }));
        }
        if !gone.is_empty() {
            self.dirty.store(true, Ordering::Relaxed);
            self.save_if_dirty();
            self.sync.request(self.list());
        }
        self.expiry.notify_one();
    }

    pub fn status(&self) -> Value {
        let entries: Vec<Value> = self
            .lock()
            .snapshot()
            .into_iter()
            .map(|(ip, at)| json!({ "ip": ip, "expiresAt": iso_ms(at) }))
            .collect();
        let last = self.sync.last_result().map(|result| {
            let mut out = json!({
                "ok": result.ok,
                "enabled": !result.ips.is_empty(),
                "ips": result.ips,
                "at": iso_ms(result.at),
            });
            if let Some(error) = result.error {
                out["error"] = json!(error);
            }
            out
        });
        json!({
            "backend": self.backend.name(),
            "dryRun": self.config.dry_run,
            "allowPrivate": self.config.allow_private,
            "ttlSeconds": self.config.ttl.as_secs(),
            "ports": self.config.ports,
            "entries": entries,
            "pending": self.sync.pending(),
            "lastApply": last,
        })
    }

    async fn serve(self: Arc<Self>, listener: TcpListener) {
        let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                continue;
            };
            let Ok(slot) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let helper = self.clone();
            tokio::spawn(async move {
                let _slot = slot;
                let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                    let helper = helper.clone();
                    async move { Ok::<_, Infallible>(helper.handle(request, peer).await) }
                });
                let connection = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(Duration::from_secs(5))
                    .serve_connection(TokioIo::new(stream), service);
                let _ = tokio::time::timeout(Duration::from_secs(30), connection).await;
            });
        }
    }

    async fn handle(&self, request: Request<Incoming>, peer: SocketAddr) -> Response<Full<Bytes>> {
        // Defence in depth: the bind address is loopback already
        let loopback = parse_ip(&peer.ip().to_string()).is_some_and(|ip| ip.class == IpClass::Loopback);
        if !loopback {
            return reply(403, &json!({ "error": "forbidden" }), true);
        }
        let path = request.uri().path().to_owned();
        let method = match path.as_str() {
            "/allow" => "POST",
            "/status" => "GET",
            _ => return reply(404, &json!({ "error": "not_found" }), false),
        };
        if request.method().as_str() != method {
            let mut response = reply(405, &json!({ "error": "method_not_allowed" }), false);
            response
                .headers_mut()
                .insert("allow", http::HeaderValue::from_static(method));
            return response;
        }
        let given = request.headers().get("x-allowlist-secret").map(|v| v.as_bytes());
        if !secret_matches(given, &self.config.secret) {
            if let Some(suppressed) = self.throttle.allow("unauthorized", crate::now_ms()) {
                self.audit
                    .write("unauthorized", json!({ "path": path, "suppressed": suppressed }));
            }
            return reply(401, &json!({ "error": "unauthorized" }), true);
        }
        if path == "/status" {
            return reply(200, &self.status(), false);
        }
        let declared = request
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .map_or(0.0, |v| service_kit::js::number(Some(&json!(v))));
        if declared > MAX_BODY as f64 {
            return reply(413, &json!({ "error": "body_too_large" }), true);
        }
        let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
            Ok(body) => body.to_bytes(),
            Err(_) => return reply(413, &json!({ "error": "body_too_large" }), true),
        };
        let Ok(body) = serde_json::from_slice::<Value>(&body) else {
            return reply(400, &json!({ "error": "bad_request" }), false);
        };
        self.allow(body.as_object().and_then(|o| o.get("ip")))
    }

    fn allow(&self, given: Option<&Value>) -> Response<Full<Bytes>> {
        let ip = match check_allowlist_ip(given.and_then(Value::as_str), self.config.allow_private) {
            AllowlistCheck::Ok(ip) => ip,
            AllowlistCheck::Refused(reason) => {
                let value = shown(given);
                if let Some(suppressed) = self
                    .throttle
                    .allow(&format!("refused:{reason}:{value}"), crate::now_ms())
                {
                    self.audit.write(
                        "refused",
                        json!({ "value": value, "reason": reason, "suppressed": suppressed }),
                    );
                }
                return reply(400, &json!({ "error": "invalid_ip", "reason": reason }), false);
            }
        };
        let result = self.lock().refresh(&ip, (self.now)());
        if result == Refresh::Full {
            if let Some(suppressed) = self.throttle.allow("full", crate::now_ms()) {
                self.audit.write(
                    "full",
                    json!({ "ip": ip.canonical, "max": self.config.max_entries, "suppressed": suppressed }),
                );
            }
            return reply(503, &json!({ "error": "allowlist_full" }), false);
        }
        self.dirty.store(true, Ordering::Relaxed);
        if result == Refresh::Added {
            self.audit.write("added", json!({ "ip": ip.canonical }));
            self.save_if_dirty();
            self.sync.request(self.list());
        }
        self.expiry.notify_one();
        reply(
            200,
            &json!({ "ip": ip.canonical, "added": result == Refresh::Added, "ttlSeconds": self.config.ttl.as_secs() }),
            false,
        )
    }
}

/// Mode 0600 where the file system has modes.
fn write_private(file: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)?.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_compare_by_hash() {
        assert!(secret_matches(Some(b"abc"), "abc"));
        assert!(!secret_matches(Some(b"abd"), "abc"));
        assert!(!secret_matches(None, "abc"));
    }

    #[test]
    fn refused_values_as_the_audit_shows_them() {
        assert_eq!(shown(Some(&json!("a\u{1}b"))), "a?b");
        assert_eq!(shown(Some(&json!("x".repeat(100)))).len(), 64);
        assert_eq!(shown(Some(&json!(7))), "number");
        assert_eq!(shown(Some(&json!(null))), "object");
        assert_eq!(shown(None), "undefined");
    }
}
