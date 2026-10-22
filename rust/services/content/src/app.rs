//! The routes (`src/app.ts`), all GET or HEAD, behind the gateway:
//!
//! - /content/v1/runtime/manifest: the game runtime the host published, its version and files with
//!   their sizes and SHA-256 (no key needed: the launcher shows it to the player before asking them
//!   to accept it)
//! - /content/v1/runtime/files/<name>: `MysticParadox.dll` or `winmm.dll`; needs
//!   `x-reforged-user-api-key`; always the whole file (no HTTP ranges)
//! - /content/v1/branding: the host's art pack (background image URLs and an accent colour)
//! - /content/v1/branding/<file>: one of those images
//! - /content/v1/news: the host's news items
//!
//! Anything else is 404; there is no directory listing anywhere. Every request is logged once its
//! answer is done (or the client went away), downloads with how many bytes went out.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use futures_util::StreamExt;
use protocol::manifest::{RUNTIME_FILE_NAMES, RUNTIME_MANIFEST_PATH};
use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;

use crate::auth::{AuthCache, LookupResult};
use crate::branding::{BRANDING_PREFIX, Branding, MAX_IMAGE_BYTES, is_safe_image_name};
use crate::limits::{Refusal, Slot, StreamLimiter};
use crate::log;
use crate::news::News;
use crate::runtime::{RuntimeStatus, RuntimeStore};

pub const RUNTIME_FILES_PREFIX: &str = "/content/v1/runtime/files/";
pub const KEY_HEADER: &str = "x-reforged-user-api-key";
const READ_CHUNK: usize = 256 * 1024;

pub struct Deps {
    pub runtime: Arc<RuntimeStore>,
    pub auth: AuthCache,
    pub limiter: StreamLimiter,
    pub branding: Branding,
    pub news: Arc<News>,
}

pub type AppState = Arc<App>;

pub struct App {
    deps: Deps,
}

impl App {
    pub fn new(deps: Deps) -> AppState {
        Arc::new(Self { deps })
    }

    pub fn deps(&self) -> &Deps {
        &self.deps
    }

    /// The runtime's status, read off the async threads (a check may hash both files).
    async fn runtime(&self) -> RuntimeStatus {
        let store = self.deps.runtime.clone();
        tokio::task::spawn_blocking(move || store.get())
            .await
            .unwrap_or_else(|_| RuntimeStatus::Invalid("runtime check failed".into()))
    }
}

pub fn router(state: AppState) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(state)
}

/// `IfNoneMatchHits`: "*" or a list of entity tags, weak ones included (RFC 9110 13.1.2).
pub fn if_none_match_hits(header: Option<&str>, etag: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    service_kit::js::trim(header) == "*"
        || header.split(',').any(|tag| {
            let tag = service_kit::js::trim(tag);
            tag.strip_prefix("W/").unwrap_or(tag) == etag
        })
}

/// The route a request path names, before the method is looked at (`IsKnownRoute` and the order
/// `CreateContentHandler` tries them in).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route<'a> {
    RuntimeManifest,
    Branding,
    News,
    BrandingImage(&'a str),
    RuntimeFile(&'a str),
    Unknown,
}

impl Route<'_> {
    /// The path up to any `?`, as it came (no decoding, no dot segments removed).
    pub fn of(path_only: &str) -> Route<'_> {
        if path_only == RUNTIME_MANIFEST_PATH {
            Route::RuntimeManifest
        } else if path_only == "/content/v1/branding" {
            Route::Branding
        } else if path_only == "/content/v1/news" {
            Route::News
        } else if let Some(name) = path_only.strip_prefix(BRANDING_PREFIX) {
            Route::BrandingImage(name)
        } else if let Some(name) = path_only.strip_prefix(RUNTIME_FILES_PREFIX) {
            Route::RuntimeFile(name)
        } else {
            Route::Unknown
        }
    }
}

/// A runtime file name: exactly one of the two, letter case included (no decoding, no folders, no
/// lookalikes).
pub fn is_runtime_file_name(name: &str) -> bool {
    RUNTIME_FILE_NAMES.contains(&name)
}

/// A header as Node's `req.headers` gives it: repeated lines joined with ", ", each byte a Latin-1
/// character.
pub fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers.get_all(name).iter().peekable();
    values.peek()?;
    Some(
        values
            .map(|v| v.as_bytes().iter().map(|&b| b as char).collect::<String>())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

// ---- The request log ----

#[derive(Default)]
struct Info {
    route: &'static str,
    path: Option<String>,
    reason: Option<String>,
    account: Option<String>,
    user: Option<String>,
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
        self.status != 304 && self.inner.is_end_stream()
    }

    /// A 304 has no length: an empty body's exact 0 would make hyper add `content-length: 0` to the
    /// answer to a HEAD, which Node does not send
    fn size_hint(&self) -> http_body::SizeHint {
        if self.status == 304 {
            http_body::SizeHint::default()
        } else {
            self.inner.size_hint()
        }
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
        let msg = if self.info.route == "runtime-file" && self.method == "GET" && self.info.bytes.is_some() {
            "download"
        } else {
            "request"
        };
        log::log(level, msg, Value::Object(fields));
    }
}

/// A reply being built: Node sets headers on `res` as it goes.
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

    /// `SendJson` with text already serialised: it goes out byte for byte.
    fn text(&mut self, head: bool, status: u16, text: String, extra: &[(&'static str, &str)]) {
        self.status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        self.set("content-type", "application/json; charset=utf-8");
        self.set("content-length", text.len());
        self.set("cache-control", "no-cache");
        for (name, value) in extra {
            self.set(name, value);
        }
        self.body = if head { Body::empty() } else { Body::from(text) };
    }

    /// `SendJson`.
    fn json(&mut self, head: bool, status: u16, body: &Value, extra: &[(&'static str, &str)]) {
        self.text(head, status, service_kit::jsjson::stringify(body), extra);
    }

    fn error(&mut self, head: bool, status: u16, code: &str, extra: &[(&'static str, &str)]) {
        self.json(head, status, &json!({"error": code}), extra);
    }
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
    let ip = header_text(&headers, "x-forwarded-for").unwrap_or_else(|| peer.ip().to_string());
    let mut info = Info {
        route: "other",
        ..Info::default()
    };
    let mut reply = Reply::new();
    let mut slot = None;
    let route = Route::of(&path_only);
    if route == Route::Unknown {
        reply.error(head, 404, "not_found", &[]);
    } else if method != Method::GET && method != Method::HEAD {
        reply.error(head, 405, "method_not_allowed", &[("allow", "GET, HEAD")]);
    } else {
        match route {
            Route::RuntimeManifest => {
                info.route = "runtime-manifest";
                runtime_manifest(&app, &headers, head, &mut reply, &mut info).await;
            }
            Route::Branding => {
                info.route = "branding";
                let body = app.deps.branding.body();
                reply.json(head, 200, &body, &[]);
            }
            Route::News => {
                info.route = "news";
                let body = app.deps.news.body();
                reply.json(head, 200, &body, &[]);
            }
            Route::BrandingImage(name) => {
                info.route = "branding-image";
                branding_image(&app, &headers, head, name, &mut reply, &mut info).await;
            }
            Route::RuntimeFile(name) => {
                info.route = "runtime-file";
                slot = runtime_file(&app, &headers, head, name, &mut reply, &mut info).await;
            }
            Route::Unknown => {}
        }
    }
    let status = reply.status.as_u16();
    let logged = Logged {
        inner: reply.body,
        method: method.to_string(),
        path_only,
        status,
        started,
        ip,
        info,
        _slot: slot,
    };
    let mut response = Response::new(Body::new(logged));
    *response.status_mut() = reply.status;
    *response.headers_mut() = reply.headers;
    response
}

async fn runtime_manifest(app: &App, headers: &HeaderMap, head: bool, reply: &mut Reply, info: &mut Info) {
    let (body, etag) = match app.runtime().await {
        RuntimeStatus::Ok { body, etag, .. } => (body, etag),
        // 404: nothing published yet (the launcher shows "waiting for the host").
        // 503: something is published but does not check out; the host has to fix it.
        RuntimeStatus::None(reason) => {
            info.reason = Some(reason);
            reply.error(head, 404, "no_runtime", &[]);
            return;
        }
        RuntimeStatus::Invalid(reason) => {
            info.reason = Some(reason);
            reply.error(head, 503, "runtime_invalid", &[]);
            return;
        }
    };
    if if_none_match_hits(header_text(headers, "if-none-match").as_deref(), &etag) {
        reply.status = StatusCode::NOT_MODIFIED;
        reply.set("etag", &etag);
        return;
    }
    reply.text(head, 200, body, &[("etag", &etag)]);
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
    if if_none_match_hits(
        header_text(headers, "if-none-match").as_deref(),
        etag.trim_start_matches("W/"),
    ) {
        reply.status = StatusCode::NOT_MODIFIED;
        return;
    }
    reply.set("content-type", image.content_type);
    reply.set("content-length", size);
    if !head {
        reply.body = file_body(file, size);
    }
}

/// The file's bytes in 256 KiB reads (`createReadStream`), `length` of them.
fn file_body(file: tokio::fs::File, length: u64) -> Body {
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(length), READ_CHUNK);
    Body::from_stream(stream.map(|chunk| chunk.map_err(axum::Error::new)))
}

/// One runtime file; the download slot it holds, if it got one.
async fn runtime_file(
    app: &App,
    headers: &HeaderMap,
    head: bool,
    name: &str,
    reply: &mut Reply,
    info: &mut Info,
) -> Option<Slot> {
    let deps = &app.deps;
    if !is_runtime_file_name(name) {
        info.reason = Some("not_a_runtime_file".into());
        reply.error(head, 404, "not_found", &[]);
        return None;
    }
    info.path = Some(name.to_owned());
    let Some(key) = header_text(headers, KEY_HEADER).filter(|key| !key.is_empty()) else {
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
    let store = deps.runtime.clone();
    let wanted = name.to_owned();
    let found = tokio::task::spawn_blocking(move || store.file(&wanted))
        .await
        .ok()
        .flatten();
    let Some((file, full)) = found else {
        info.reason = Some("no_runtime".into());
        reply.error(head, 404, "not_found", &[]);
        return None;
    };
    let etag = format!("\"{}\"", file.sha256);
    if if_none_match_hits(header_text(headers, "if-none-match").as_deref(), &etag) {
        reply.status = StatusCode::NOT_MODIFIED;
        reply.set("etag", &etag);
        return None;
    }
    if head {
        reply.set("etag", &etag);
        reply.set("content-type", "application/octet-stream");
        reply.set("content-length", file.size);
        return None;
    }
    let slot = match deps.limiter.try_acquire(&account.user_id) {
        Ok(slot) => slot,
        Err(refusal) => {
            info.reason = Some(refusal.as_str().into());
            match refusal {
                Refusal::AccountLimit => reply.error(head, 429, "too_many_streams", &[("retry-after", "2")]),
                Refusal::TotalLimit => reply.error(head, 503, "server_busy", &[("retry-after", "10")]),
            }
            return None;
        }
    };
    let opened = match tokio::fs::File::open(&full).await {
        Ok(opened) => opened,
        Err(_) => {
            deps.runtime.invalidate();
            info.reason = Some("open_failed".into());
            reply.error(head, 503, "file_unavailable", &[]);
            return Some(slot);
        }
    };
    let Ok(meta) = opened.metadata().await else {
        reply.error(head, 503, "file_unavailable", &[]);
        return Some(slot);
    };
    // Replaced since the last check (a new publish in progress): never send a size the list does not name
    if !meta.is_file() || meta.len() != file.size {
        deps.runtime.invalidate();
        info.reason = Some("changed_on_disk".into());
        reply.error(head, 503, "file_unavailable", &[("retry-after", "10")]);
        return Some(slot);
    }
    reply.set("etag", &etag);
    reply.set("content-type", "application/octet-stream");
    reply.set("content-length", file.size);
    reply.set("cache-control", "private, no-transform");
    info.bytes = Some((Arc::new(AtomicU64::new(0)), file.size));
    reply.body = file_body(opened, file.size);
    Some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_headers() {
        let tag = "\"abc\"";
        assert!(!if_none_match_hits(None, tag));
        assert!(if_none_match_hits(Some(" * "), tag));
        assert!(if_none_match_hits(Some("\"x\", \"abc\""), tag));
        assert!(if_none_match_hits(Some("W/\"abc\""), tag));
        assert!(!if_none_match_hits(Some("\"abcd\""), tag));
        assert!(!if_none_match_hits(Some("*, \"x\""), tag));
    }

    #[test]
    fn routes_are_matched_on_the_raw_path() {
        assert_eq!(Route::of("/content/v1/runtime/manifest"), Route::RuntimeManifest);
        assert_eq!(Route::of("/content/v1/runtime/manifest/"), Route::Unknown);
        assert_eq!(
            Route::of("/content/v1/runtime/files/%2e%2e/runtime.json"),
            Route::RuntimeFile("%2e%2e/runtime.json")
        );
        assert_eq!(Route::of("/content/v1/branding/"), Route::BrandingImage(""));
        assert_eq!(Route::of("/content/v1/files/x"), Route::Unknown);
        assert!(is_runtime_file_name("winmm.dll") && !is_runtime_file_name("winmm.DLL"));
    }

    #[test]
    fn repeated_headers_are_joined_as_node_joins_them() {
        let mut headers = HeaderMap::new();
        headers.append(KEY_HEADER, HeaderValue::from_static("RRK_a"));
        headers.append(KEY_HEADER, HeaderValue::from_static("RRK_b"));
        assert_eq!(header_text(&headers, KEY_HEADER).as_deref(), Some("RRK_a, RRK_b"));
        assert_eq!(header_text(&headers, "if-none-match"), None);
    }
}
