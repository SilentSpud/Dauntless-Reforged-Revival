//! The routes (`src/app.ts`), all GET or HEAD:
//!
//! - /content/v1/manifest: the file list, public inside the tailnet (it is in the launcher anyway)
//! - /content/v1/files/<path>: one game file; needs `x-undaunted-user-api-key`; one HTTP range
//! - /content/v1/branding: the host's art pack (background image URLs and an accent colour)
//! - /content/v1/branding/<file>: one of those images
//! - /content/v1/news: the host's news items
//!
//! Anything else is 404; there is no directory listing anywhere. Every request is logged once its
//! answer is done (or the client went away), downloads with how many bytes went out.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::auth::{AuthCache, LookupResult};
use crate::branding::{BRANDING_PREFIX, Branding, MAX_IMAGE_BYTES, is_safe_image_name};
use crate::gamedir::FileState;
use crate::limits::{Refusal, Slot, StreamLimiter};
use crate::log;
use crate::manifest::{Manifest, ManifestFile, on_disk_path};
use crate::news::News;
use crate::paths::{FILES_PREFIX, parse_file_request_path};
use crate::range::{Range, if_none_match_hits, if_range_allows_partial, parse_range};

const READ_CHUNK: usize = 1024 * 1024;

pub struct Deps {
    pub download_base_url: Option<String>,
    pub manifest: Manifest,
    pub index: HashMap<String, ManifestFile>,
    pub game_dir: PathBuf,
    pub states: Mutex<HashMap<String, FileState>>,
    pub auth: AuthCache,
    pub limiter: StreamLimiter,
    pub branding: Branding,
    pub news: Arc<News>,
}

pub type AppState = Arc<App>;

pub struct App {
    deps: Deps,
    manifest_text: String,
    manifest_etag: String,
}

impl App {
    pub fn new(deps: Deps) -> AppState {
        let manifest_text = crate::manifest::to_json(&deps.manifest, deps.download_base_url.as_deref());
        let digest: String = Sha256::digest(manifest_text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Arc::new(Self {
            deps,
            manifest_text,
            manifest_etag: format!("\"{digest}\""),
        })
    }

    pub fn deps(&self) -> &Deps {
        &self.deps
    }
}

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(state)
}

// ---- The request log ----

#[derive(Default)]
struct Info {
    route: &'static str,
    path: Option<String>,
    reason: Option<String>,
    account: Option<String>,
    user: Option<String>,
    range: Option<String>,
    /// Bytes sent and expected, for a download
    bytes: Option<(Arc<AtomicU64>, u64)>,
}

/// Written when the body is done or dropped (Node's `res.on("close")`); holds the download slot too.
struct Logged {
    inner: Body,
    method: String,
    path_only: String,
    status: u16,
    started: Instant,
    ip: String,
    info: Info,
    _slot: Option<Slot>,
}

impl http_body::Body for Logged {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        let polled = Pin::new(&mut self.inner).poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let (Some(data), Some((sent, _))) = (frame.data_ref(), &self.info.bytes)
        {
            sent.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for Logged {
    fn drop(&mut self) {
        let mut fields = Map::new();
        fields.insert("method".into(), json!(self.method));
        fields.insert("route".into(), json!(self.info.route));
        let path = self
            .info
            .path
            .clone()
            .unwrap_or_else(|| crate::branding::slice_utf16(&self.path_only, 200));
        fields.insert("path".into(), json!(path));
        fields.insert("status".into(), json!(self.status));
        fields.insert("ms".into(), json!(self.started.elapsed().as_millis() as u64));
        fields.insert("ip".into(), json!(self.ip));
        for (name, value) in [
            ("reason", &self.info.reason),
            ("account", &self.info.account),
            ("user", &self.info.user),
            ("range", &self.info.range),
        ] {
            if let Some(value) = value {
                fields.insert(name.into(), json!(value));
            }
        }
        if let Some((sent, expected)) = &self.info.bytes {
            let sent = sent.load(Ordering::Relaxed);
            fields.insert("bytes".into(), json!(sent));
            fields.insert("expected".into(), json!(expected));
            // Every byte handed to the connection (hyper stops reading a body at its length)
            fields.insert("complete".into(), json!(sent == *expected));
        }
        let level = if self.status >= 500 || self.status == 400 || self.status == 401 {
            log::Level::Warn
        } else {
            log::Level::Info
        };
        let msg = if self.info.route == "files" && self.method == "GET" && self.info.bytes.is_some() {
            "download"
        } else {
            "request"
        };
        log::log(level, msg, Value::Object(fields));
    }
}

/// A reply being built: Node sets headers on `res` as it goes and removes some on an error.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
}

impl Reply {
    fn new() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
        Self {
            status: StatusCode::OK,
            headers,
            body: Body::empty(),
        }
    }

    fn set(&mut self, name: &'static str, value: impl ToString) {
        if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
            self.headers.insert(HeaderName::from_static(name), value);
        }
    }

    fn remove(&mut self, names: &[&str]) {
        for name in names {
            self.headers.remove(*name);
        }
    }

    /// For the public, key-free JSON and images, so the launcher's window can read them directly.
    fn public(&mut self) {
        self.set("access-control-allow-origin", "*");
        self.set("cross-origin-resource-policy", "cross-origin");
    }

    /// `SendJson`.
    fn json(&mut self, head: bool, status: u16, body: &Value, extra: &[(&'static str, &str)]) {
        let text = service_kit::jsjson::stringify(body);
        self.status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        self.set("content-type", "application/json; charset=utf-8");
        self.set("content-length", text.len());
        self.set("cache-control", "no-cache");
        for (name, value) in extra {
            self.set(name, value);
        }
        self.body = if head { Body::empty() } else { Body::from(text) };
    }

    fn error(&mut self, head: bool, status: u16, code: &str, extra: &[(&'static str, &str)]) {
        self.json(head, status, &json!({"error": code}), extra);
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn handle(State(app): State<AppState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let target = request
        .uri()
        .path_and_query()
        .map_or_else(|| request.uri().path().to_owned(), |p| p.as_str().to_owned());
    let path_only = target.split('?').next().unwrap_or("").to_owned();
    let head = method == Method::HEAD;
    // The request's headers alone cross the awaits below (a request is not Sync)
    let headers = request.headers().clone();
    drop(request);
    let mut info = Info {
        route: "other",
        ..Info::default()
    };
    let mut reply = Reply::new();
    let mut slot = None;
    let known = path_only == "/content/v1/manifest"
        || path_only.starts_with(FILES_PREFIX)
        || path_only == "/content/v1/branding"
        || path_only.starts_with(BRANDING_PREFIX)
        || path_only == "/content/v1/news";
    if !known {
        reply.error(head, 404, "not_found", &[]);
    } else if method != Method::GET && method != Method::HEAD {
        reply.error(head, 405, "method_not_allowed", &[("allow", "GET, HEAD")]);
    } else if path_only == "/content/v1/manifest" {
        info.route = "manifest";
        reply.public();
        if if_none_match_hits(header_text(&headers, "if-none-match"), &app.manifest_etag) {
            reply.status = StatusCode::NOT_MODIFIED;
            reply.set("etag", &app.manifest_etag);
        } else {
            reply.set("content-type", "application/json; charset=utf-8");
            reply.set("content-length", app.manifest_text.len());
            reply.set("cache-control", "no-cache");
            reply.set("etag", &app.manifest_etag);
            reply.body = if head {
                Body::empty()
            } else {
                Body::from(app.manifest_text.clone())
            };
        }
    } else if path_only == "/content/v1/branding" {
        info.route = "branding";
        reply.public();
        let body = app.deps.branding.body();
        reply.json(head, 200, &body, &[]);
    } else if path_only == "/content/v1/news" {
        info.route = "news";
        reply.public();
        let body = app.deps.news.body();
        reply.json(head, 200, &body, &[]);
    } else if let Some(name) = path_only.strip_prefix(BRANDING_PREFIX) {
        info.route = "branding-image";
        reply.public();
        branding_image(&app, &headers, head, name, &mut reply, &mut info).await;
    } else {
        info.route = "files";
        slot = game_file(&app, &headers, head, &target, &mut reply, &mut info).await;
    }
    let status = reply.status.as_u16();
    let logged = Logged {
        inner: reply.body,
        method: method.to_string(),
        path_only,
        status,
        started,
        ip: peer.ip().to_string(),
        info,
        _slot: slot,
    };
    let mut response = Response::new(Body::new(logged));
    *response.status_mut() = reply.status;
    *response.headers_mut() = reply.headers;
    response
}

/// Milliseconds since 1970, floored (`Math.floor(stat.mtimeMs)`).
fn modified_ms(modified: Option<SystemTime>) -> u128 {
    modified
        .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis())
}

async fn branding_image(app: &App, headers: &HeaderMap, head: bool, name: &str, reply: &mut Reply, info: &mut Info) {
    if !is_safe_image_name(name) {
        reply.error(head, 404, "not_found", &[]);
        return;
    }
    let Some((image, full)) = app.deps.branding.image(name) else {
        reply.error(head, 404, "not_found", &[]);
        return;
    };
    info.path = Some(name.to_owned());
    let Ok(file) = tokio::fs::File::open(&full).await else {
        reply.error(head, 404, "not_found", &[]);
        return;
    };
    let Ok(meta) = file.metadata().await else {
        reply.error(head, 404, "not_found", &[]);
        return;
    };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
        reply.error(head, 404, "not_found", &[]);
        return;
    }
    let size = meta.len();
    let etag = format!("W/\"{size:x}-{:x}\"", modified_ms(meta.modified().ok()));
    reply.set("etag", &etag);
    reply.set("cache-control", "public, max-age=300");
    if if_none_match_hits(header_text(headers, "if-none-match"), etag.trim_start_matches("W/")) {
        reply.status = StatusCode::NOT_MODIFIED;
        return;
    }
    reply.set("content-type", image.content_type);
    reply.set("content-length", size);
    if !head {
        reply.body = file_body(file, size);
    }
}

/// The file's bytes in 1 MB reads (`createReadStream`), `length` of them from where it stands.
fn file_body(file: tokio::fs::File, length: u64) -> Body {
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(length), READ_CHUNK);
    Body::from_stream(stream.map(|chunk| chunk.map_err(axum::Error::new)))
}

/// One game file; the download slot it holds, if it got one.
async fn game_file(
    app: &App,
    headers: &HeaderMap,
    head: bool,
    target: &str,
    reply: &mut Reply,
    info: &mut Info,
) -> Option<Slot> {
    let deps = &app.deps;
    let path = match parse_file_request_path(target) {
        Ok(path) => path,
        Err(reason) => {
            info.reason = Some(reason.into());
            reply.error(head, 400, "bad_path", &[]);
            return None;
        }
    };
    let Some(entry) = deps.index.get(&path) else {
        info.reason = Some("not_in_manifest".into());
        reply.error(head, 404, "not_found", &[]);
        return None;
    };
    info.path = Some(entry.path.clone());
    let key = headers.get("x-undaunted-user-api-key");
    let key = match key {
        None => None,
        Some(value) if value.is_empty() => None,
        Some(value) => Some(String::from_utf8_lossy(value.as_bytes()).into_owned()),
    };
    let Some(key) = key else {
        info.reason = Some("no_key".into());
        reply.error(head, 401, "unauthorized", &[]);
        return None;
    };
    let account = match deps.auth.check(&key).await {
        LookupResult::Unavailable(reason) => {
            info.reason = Some(reason);
            reply.error(head, 503, "auth_unavailable", &[("retry-after", "5")]);
            return None;
        }
        LookupResult::Invalid => {
            info.reason = Some("bad_key".into());
            reply.error(head, 401, "unauthorized", &[]);
            return None;
        }
        LookupResult::Ok(account) => account,
    };
    info.account = Some(account.user_id.clone());
    info.user = Some(account.username.clone());
    let state = deps
        .states
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&entry.path)
        .cloned();
    let Some(FileState::Ok {
        modified: seen_modified,
        ..
    }) = state
    else {
        info.reason = Some(format!("file_{}", state.as_ref().map_or("unknown", FileState::status)));
        reply.error(head, 503, "file_unavailable", &[]);
        return None;
    };
    let etag = format!("\"{}\"", entry.sha256);
    reply.set("etag", &etag);
    reply.set("accept-ranges", "bytes");
    reply.set("cache-control", "private, no-transform");
    if if_none_match_hits(header_text(headers, "if-none-match"), &etag) {
        reply.status = StatusCode::NOT_MODIFIED;
        return None;
    }
    // A stale If-Range means the client's partial copy is of something else: send it all
    let if_range = header_text(headers, "if-range");
    let range_header = if if_range_allows_partial(if_range, &etag) {
        header_text(headers, "range")
    } else {
        None
    };
    let range = parse_range(range_header, entry.size);
    if let Some(range_header) = range_header {
        info.range = Some(crate::branding::slice_utf16(range_header, 64));
    }
    let (start, end) = match range {
        Range::Unsatisfiable => {
            info.reason = Some("bad_range".into());
            let content_range = format!("bytes */{}", entry.size);
            reply.error(head, 416, "range_not_satisfiable", &[("content-range", &content_range)]);
            return None;
        }
        Range::Partial { start, end } => (start, end as i64),
        Range::Full => (0, entry.size as i64 - 1),
    };
    let length = (end - start as i64 + 1).max(0) as u64;
    reply.status = if matches!(range, Range::Partial { .. }) {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    reply.set("content-type", "application/octet-stream");
    reply.set("content-length", length);
    if matches!(range, Range::Partial { .. }) {
        reply.set("content-range", format!("bytes {start}-{end}/{}", entry.size));
    }
    if head {
        return None;
    }
    let slot = match deps.limiter.try_acquire(&account.user_id) {
        Ok(slot) => slot,
        Err(refusal) => {
            info.reason = Some(refusal.as_str().into());
            reply.remove(&["content-range", "etag", "accept-ranges"]);
            match refusal {
                Refusal::AccountLimit => reply.error(head, 429, "too_many_streams", &[("retry-after", "2")]),
                Refusal::TotalLimit => reply.error(head, 503, "server_busy", &[("retry-after", "10")]),
            }
            return None;
        }
    };
    let unavailable = |reply: &mut Reply| {
        reply.remove(&[
            "content-range",
            "etag",
            "accept-ranges",
            "content-type",
            "content-length",
        ]);
        reply.error(head, 503, "file_unavailable", &[]);
    };
    let mut file = match tokio::fs::File::open(on_disk_path(&deps.game_dir, entry)).await {
        Ok(file) => file,
        Err(error) => {
            deps.states
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(entry.path.clone(), FileState::Missing);
            info.reason = Some("open_failed".into());
            log::warn(
                "game file could not be opened; no longer serving it",
                json!({"path": entry.path, "error": error.to_string()}),
            );
            unavailable(reply);
            return Some(slot);
        }
    };
    let Ok(meta) = file.metadata().await else {
        unavailable(reply);
        return Some(slot);
    };
    // Changed on disk since the startup check: no longer served until the host verifies and restarts
    if !meta.is_file() || meta.len() != entry.size || meta.modified().ok() != seen_modified {
        deps.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(entry.path.clone(), FileState::Changed);
        info.reason = Some("changed_on_disk".into());
        log::warn(
            "game file changed since startup; no longer serving it (run content verify, then restart)",
            json!({"path": entry.path}),
        );
        unavailable(reply);
        return Some(slot);
    }
    info.bytes = Some((Arc::new(AtomicU64::new(0)), length));
    if length == 0 {
        return Some(slot);
    }
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        unavailable(reply);
        return Some(slot);
    }
    reply.body = file_body(file, length);
    Some(slot)
}
