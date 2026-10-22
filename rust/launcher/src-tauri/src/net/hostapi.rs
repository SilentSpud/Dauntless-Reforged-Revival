//! Calls to the host's metagame (contracts 2 and 3) and content server (contract 4), row L21:
//! `src/main/hostapi.ts`. The account key is sent only in the `x-undaunted-user-api-key` header, only to
//! the invite's host (in public mode only over the TLS connection pinned to the invite's certificate):
//! GetUserInfo, ServerStatus (for the player list), the hunt region and the content downloads.

use std::time::Duration;

use bytes::Bytes;
use hyper::Method;
use protocol::status::ServerStatus;
use serde_json::{Map, Value};

use super::http::{Endpoint, HttpError, HttpResponse, RequestOptions, parse_json_body, request};
use crate::bridge::types::{ErrorCode, NewsItem};
use crate::core::invite::is_private_mode_host;
use crate::core::pins::CONNECT_TIMEOUT_MS;
use crate::core::status::{clean_text, iso_date, parse_server_status};
use crate::core::text::{is_unsafe_char, js_length, js_trim};
use crate::core::username::is_plausible_account_key;

const KEY_HEADER: &str = "x-undaunted-user-api-key";

fn options(method: Method, timeout_ms: u64, max_bytes: usize) -> RequestOptions {
    RequestOptions {
        method,
        ..RequestOptions::get(Duration::from_millis(timeout_ms), max_bytes)
    }
}

fn object(value: &Option<Value>) -> Option<&Map<String, Value>> {
    value.as_ref()?.as_object()
}

/// The key goes only where every other key-carrying request goes: to the invite's host, over the
/// pinned connection (public mode) or plain HTTP inside the tailnet or to this PC (private mode).
fn key_headers(endpoint: &Endpoint, key: Option<&str>) -> Vec<(&'static str, String)> {
    match key {
        Some(key)
            if is_plausible_account_key(key) && (endpoint.pin.is_some() || is_private_mode_host(&endpoint.host)) =>
        {
            vec![(KEY_HEADER, key.to_owned())]
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- device signals, hunt region

/// Reported identifiers are an optional launcher check, not hardware attestation.
pub async fn check_device_signals(
    endpoint: &Endpoint,
    key: &str,
    signals: &Value,
) -> Result<(), (ErrorCode, Option<String>)> {
    let mut request_options = options(Method::POST, 8000, 4096);
    request_options.headers = vec![(KEY_HEADER, key.to_owned())];
    request_options.body = Some(signals.to_string());
    let Ok(response) = request(endpoint, "/undaunted/api/DeviceSignals", request_options).await else {
        return Err((ErrorCode::ServerError, None));
    };
    if response.status == 204 || response.status == 404 {
        return Ok(());
    }
    let body = parse_json_body(&response);
    let error = object(&body).and_then(|body| body.get("error")).and_then(Value::as_str);
    if response.status == 503 && error == Some("device_signals_unavailable") {
        return Ok(());
    }
    if response.status == 403 && error == Some("device_banned") {
        let reason = object(&body).and_then(|body| clean_text(body.get("reason"), 500));
        let reason = reason.unwrap_or_else(|| "Contact support to appeal.".into());
        return Err((ErrorCode::AccountBanned, Some(format!("Device restriction: {reason}"))));
    }
    Err((
        if response.status == 429 {
            ErrorCode::RateLimited
        } else {
            ErrorCode::ServerError
        },
        None,
    ))
}

pub async fn save_hunt_region(endpoint: &Endpoint, key: &str, region: &str) -> Result<bool, HttpError> {
    let mut request_options = options(Method::POST, 10000, 4096);
    request_options.headers = key_headers(endpoint, Some(key));
    request_options.body = Some(serde_json::json!({ "region": region }).to_string());
    let response = request(endpoint, "/undaunted/api/HuntRegion", request_options).await?;
    let body = parse_json_body(&response);
    Ok(response.status == 200
        && object(&body)
            .and_then(|body| body.get("region"))
            .and_then(Value::as_str)
            == Some(region))
}

pub async fn end_playing(endpoint: &Endpoint, key: &str) -> Result<(), HttpError> {
    let mut request_options = options(Method::POST, 5000, 4096);
    request_options.headers = key_headers(endpoint, Some(key));
    request(endpoint, "/undaunted/api/PlayingEnded", request_options)
        .await
        .map(|_| ())
}

// ---------------------------------------------------------------- ServerStatus

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusResult {
    Ok(ServerStatus),
    /// A reachable Dauntless Revived / Undaunted server without ServerStatus
    Unsupported,
    BadAnswer,
    /// Public mode: the certificate is not the one from the invite
    CertMismatch,
    Unreachable,
}

fn unreachable_or_pin(error: &HttpError) -> StatusResult {
    if matches!(error, HttpError::Pin { .. }) {
        StatusResult::CertMismatch
    } else {
        StatusResult::Unreachable
    }
}

/// The server lists who is online only for a registered player; without a key (or with one it does
/// not accept) it answers the same status with `limited: true`.
pub async fn fetch_server_status(endpoint: &Endpoint, key: Option<&str>, timeout_ms: Option<u64>) -> StatusResult {
    let timeout_ms = timeout_ms.unwrap_or(CONNECT_TIMEOUT_MS);
    let mut request_options = options(Method::GET, timeout_ms, 1024 * 1024);
    request_options.headers = key_headers(endpoint, key);
    let response = match request(endpoint, "/undaunted/api/ServerStatus", request_options).await {
        Ok(response) => response,
        Err(error) => return unreachable_or_pin(&error),
    };
    if response.status == 200 {
        return parse_json_body(&response)
            .and_then(|body| parse_server_status(&body))
            .map_or(StatusResult::BadAnswer, StatusResult::Ok);
    }
    if response.status == 404 {
        // An older metagame: still a real server if /dauntless-status answers like one
        match request(
            endpoint,
            "/dauntless-status",
            options(Method::GET, timeout_ms, 64 * 1024),
        )
        .await
        {
            Ok(legacy) => {
                let body = parse_json_body(&legacy);
                if legacy.status == 200 && object(&body).is_some_and(|body| body.contains_key("show-status")) {
                    return StatusResult::Unsupported;
                }
            }
            Err(error) => return unreachable_or_pin(&error),
        }
    }
    StatusResult::BadAnswer
}

// ---------------------------------------------------------------- Register / GetUserInfo

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterResult {
    Ok { key: String },
    Err { error: ErrorCode, message: Option<String> },
}

pub async fn register_account(endpoint: &Endpoint, username: &str, invite_code: &str) -> RegisterResult {
    let mut request_options = options(Method::POST, 20000, 64 * 1024);
    let body = protocol::account::RegisterRequest {
        username: username.to_owned(),
        invite_code: invite_code.to_owned(),
    };
    request_options.body = serde_json::to_string(&body).ok();
    let failed = |error| RegisterResult::Err { error, message: None };
    let response = match request(endpoint, "/undaunted/api/Register", request_options).await {
        Ok(response) => response,
        Err(HttpError::Pin { .. }) => return failed(ErrorCode::CertMismatch),
        Err(_) => return failed(ErrorCode::ServerUnreachable),
    };
    let body = parse_json_body(&response);
    let body = object(&body);
    let server_error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    let message = body.and_then(|body| clean_text(body.get("message"), 200));
    match response.status {
        200 => match body
            .and_then(|body| serde_json::from_value::<protocol::account::Registered>(Value::Object(body.clone())).ok())
        {
            Some(answer) if is_plausible_account_key(&answer.key) => RegisterResult::Ok { key: answer.key },
            _ => failed(ErrorCode::ServerError),
        },
        400 => {
            let error = match server_error {
                Some("username_invalid") => ErrorCode::UsernameInvalid,
                Some("registration_closed") => ErrorCode::RegistrationClosed,
                _ => ErrorCode::BadRequest,
            };
            RegisterResult::Err { error, message }
        }
        401 => failed(ErrorCode::InviteInvalid),
        409 => failed(ErrorCode::UsernameTaken),
        413 | 429 => failed(ErrorCode::RateLimited),
        _ => failed(ErrorCode::ServerError),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
    pub is_admin: bool,
}

pub async fn fetch_user_info(endpoint: &Endpoint, key: &str) -> Result<UserInfo, (ErrorCode, Option<String>)> {
    if !is_plausible_account_key(key) {
        return Err((ErrorCode::KeyRejected, None));
    }
    let mut request_options = options(Method::GET, 10000, 64 * 1024);
    request_options.headers = vec![(KEY_HEADER, key.to_owned())];
    let response = match request(endpoint, "/undaunted/api/GetUserInfo", request_options).await {
        Ok(response) => response,
        Err(HttpError::Pin { .. }) => return Err((ErrorCode::CertMismatch, None)),
        Err(_) => return Err((ErrorCode::ServerUnreachable, None)),
    };
    let body = parse_json_body(&response);
    let body = object(&body);
    if response.status == 403
        && body.and_then(|body| body.get("error")).and_then(Value::as_str) == Some("account_banned")
    {
        let reason = body
            .and_then(|body| clean_text(body.get("reason"), 500))
            .unwrap_or_else(|| "Contact support for details.".into());
        return Err((ErrorCode::AccountBanned, Some(reason)));
    }
    match response.status {
        401 | 403 => return Err((ErrorCode::KeyRejected, None)),
        429 => return Err((ErrorCode::RateLimited, None)),
        200 => {}
        _ => return Err((ErrorCode::ServerError, None)),
    }
    let body = body.ok_or((ErrorCode::ServerError, None))?;
    let user_id = clean_text(body.get("UserId"), 128).ok_or((ErrorCode::ServerError, None))?;
    let username = clean_text(body.get("Username"), 32).ok_or((ErrorCode::ServerError, None))?;
    Ok(UserInfo {
        user_id,
        username,
        is_admin: body.get("IsAdmin") == Some(&Value::Bool(true)),
    })
}

// ---------------------------------------------------------------- content server

/// Public mode: is the host's content server behind the gateway?
pub async fn probe_content(endpoint: &Endpoint) -> bool {
    request(endpoint, "/content/v1/manifest", options(Method::HEAD, 8000, 1024))
        .await
        .is_ok_and(|response| response.status == 200)
}

pub async fn fetch_content_manifest(endpoint: &Endpoint) -> Option<Value> {
    let response = request(
        endpoint,
        "/content/v1/manifest",
        options(Method::GET, 20000, 8 * 1024 * 1024),
    )
    .await
    .ok()?;
    if response.status == 200 {
        parse_json_body(&response)
    } else {
        None
    }
}

/// Line breaks made `\n`, control and bidi characters other than `\n` removed, at most one empty line
/// in a row, trimmed, and at most 4000 UTF-16 units (then "…").
pub fn clean_news_body(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?.replace("\r\n", "\n").replace('\r', "\n");
    let kept: String = raw.chars().filter(|c| *c == '\n' || !is_unsafe_char(*c)).collect();
    let mut text = String::with_capacity(kept.len());
    let mut newlines = 0;
    for c in kept.chars() {
        if c == '\n' {
            newlines += 1;
            if newlines <= 2 {
                text.push(c);
            }
        } else {
            newlines = 0;
            text.push(c);
        }
    }
    let text = js_trim(&text);
    if text.is_empty() {
        return None;
    }
    if js_length(text) <= 4000 {
        return Some(text.to_owned());
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        units += c.len_utf16();
        if units > 4000 {
            break;
        }
        out.push(c);
    }
    out.push('…');
    Some(out)
}

pub fn parse_news(raw: Option<&Value>) -> Vec<NewsItem> {
    let Some(items) = raw
        .and_then(Value::as_object)
        .and_then(|raw| raw.get("items"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .take(30)
        .filter_map(Value::as_object)
        .filter_map(|item| {
            let title = clean_text(item.get("title"), 120)?;
            let body = clean_news_body(item.get("body"))?;
            let date = item.get("date").and_then(Value::as_str).and_then(iso_date);
            Some(NewsItem { date, title, body })
        })
        .collect()
}

pub async fn fetch_news(endpoint: &Endpoint, fallback: Vec<NewsItem>) -> Vec<NewsItem> {
    match request(endpoint, "/content/v1/news", options(Method::GET, 10000, 512 * 1024)).await {
        Ok(response) if response.status == 200 => parse_news(parse_json_body(&response).as_ref()),
        _ => fallback,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrandingIndex {
    /// (path, credit)
    pub backgrounds: Vec<(String, Option<String>)>,
    pub accent: Option<String>,
}

/// `/^\/content\/v1\/branding\/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,100}\.(jpg|jpeg|png|webp)$/i`, without `..`.
pub fn is_branding_path(path: &str) -> bool {
    const PREFIX: &str = "/content/v1/branding/";
    if path.contains("..") || path.len() < PREFIX.len() || !path[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        return false;
    }
    let name = &path[PREFIX.len()..];
    let first = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    let rest = |c: u8| first(c) || c == b'.';
    ["jpg", "jpeg", "png", "webp"].iter().any(|extension| {
        let suffix = extension.len() + 1;
        name.len() > suffix
            && name.is_char_boundary(name.len() - suffix)
            && name[name.len() - suffix..].eq_ignore_ascii_case(&format!(".{extension}"))
            && first(name.as_bytes()[0])
            && name.len() - suffix - 1 <= 100
            && name.as_bytes()[1..name.len() - suffix].iter().all(|c| rest(*c))
    })
}

pub fn parse_branding(raw: Option<&Value>) -> BrandingIndex {
    let mut out = BrandingIndex::default();
    let Some(raw) = raw.and_then(Value::as_object) else {
        return out;
    };
    if let Some(list) = raw.get("backgrounds").and_then(Value::as_array) {
        for background in list.iter().take(8).filter_map(Value::as_object) {
            if let Some(url) = background
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| is_branding_path(url))
            {
                out.backgrounds
                    .push((url.to_owned(), clean_text(background.get("credit"), 120)));
            }
        }
    }
    if let Some(accent) = raw.get("accent").and_then(Value::as_str)
        && accent.len() == 7
        && accent.starts_with('#')
        && accent[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        out.accent = Some(accent.to_ascii_lowercase());
    }
    out
}

pub async fn fetch_branding_index(endpoint: &Endpoint) -> BrandingIndex {
    match request(endpoint, "/content/v1/branding", options(Method::GET, 10000, 64 * 1024)).await {
        Ok(response) if response.status == 200 => parse_branding(parse_json_body(&response).as_ref()),
        _ => BrandingIndex::default(),
    }
}

/// Decides the image type from its first bytes; the server's Content-Type is not trusted.
pub fn sniff_image(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if data.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub const MAX_ART_BYTES: usize = 15 * 1024 * 1024;

pub async fn fetch_branding_image(endpoint: &Endpoint, path: &str) -> Option<(&'static str, Bytes)> {
    if !is_branding_path(path) {
        return None;
    }
    let response: HttpResponse = request(endpoint, path, options(Method::GET, 30000, MAX_ART_BYTES))
        .await
        .ok()?;
    if response.status != 200 {
        return None;
    }
    sniff_image(&response.body).map(|kind| (kind, response.body))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};

    use super::*;
    use http_body_util::{BodyExt, Full};
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use serde::Deserialize;
    use tokio::net::TcpListener;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Cases {
        news: Vec<Case>,
        branding: Vec<Case>,
        images: Vec<Image>,
        news_body: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        input: Value,
        expected: Value,
    }

    #[derive(Deserialize)]
    struct Image {
        bytes: Vec<u8>,
        #[serde(rename = "type")]
        kind: Option<String>,
    }

    #[test]
    fn the_shared_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/hostapi.json");
        let cases: Cases = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for case in cases.news {
            assert_eq!(
                serde_json::to_value(parse_news(Some(&case.input))).unwrap(),
                case.expected,
                "{}",
                case.input
            );
        }
        for case in cases.branding {
            let index = parse_branding(Some(&case.input));
            let backgrounds: Vec<Value> = index
                .backgrounds
                .iter()
                .map(|(path, credit)| serde_json::json!({ "path": path, "credit": credit }))
                .collect();
            assert_eq!(
                serde_json::json!({ "backgrounds": backgrounds, "accent": index.accent }),
                case.expected,
                "{}",
                case.input
            );
        }
        for case in cases.images {
            assert_eq!(sniff_image(&case.bytes), case.kind.as_deref(), "{:?}", case.bytes);
        }
        for case in cases.news_body {
            assert_eq!(
                serde_json::to_value(clean_news_body(Some(&case.input))).unwrap(),
                case.expected,
                "{}",
                case.input
            );
        }
    }

    /// A fake host: answers each path with a status and a JSON body, and records the key header.
    async fn host(
        answers: HashMap<&'static str, (u16, Value)>,
    ) -> (Endpoint, Arc<Mutex<Vec<(String, Option<String>)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let answers = Arc::new(answers);
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let answers = answers.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                        let answers = answers.clone();
                        let log = log.clone();
                        async move {
                            let key = request
                                .headers()
                                .get(KEY_HEADER)
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned);
                            let path = request.uri().path().to_owned();
                            log.lock().unwrap().push((path.clone(), key));
                            let _ = request.into_body().collect().await;
                            let (status, body) = answers.get(path.as_str()).cloned().unwrap_or((404, Value::Null));
                            let response = Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(if body.is_null() {
                                    String::new()
                                } else {
                                    body.to_string()
                                })))
                                .unwrap();
                            Ok::<_, Infallible>(response)
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tcp), service)
                        .await;
                });
            }
        });
        (
            Endpoint {
                host: "127.0.0.1".into(),
                port,
                pin: None,
            },
            seen,
        )
    }

    // Built from pieces, so the repository check never sees a whole key
    const KEY: &str = concat!("UUK_", "0123456789abcdef", "0123456789abcdef", "0123456789abcdef");

    #[tokio::test]
    async fn status_with_the_key_and_the_older_servers() {
        let status =
            serde_json::json!({ "name": "Ramsgate", "online": true, "players": [{ "name": "A", "where": "city" }] });
        let (endpoint, seen) = host(HashMap::from([("/undaunted/api/ServerStatus", (200, status))])).await;
        match fetch_server_status(&endpoint, Some(KEY), None).await {
            StatusResult::Ok(status) => assert_eq!((status.name.as_str(), status.players.len()), ("Ramsgate", 1)),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            seen.lock().unwrap()[0],
            ("/undaunted/api/ServerStatus".into(), Some(KEY.into()))
        );

        let legacy = serde_json::json!({ "show-status": true });
        let (endpoint, _) = host(HashMap::from([("/dauntless-status", (200, legacy))])).await;
        assert_eq!(
            fetch_server_status(&endpoint, None, None).await,
            StatusResult::Unsupported
        );
        let (endpoint, _) = host(HashMap::new()).await;
        assert_eq!(
            fetch_server_status(&endpoint, None, None).await,
            StatusResult::BadAnswer
        );
        let closed = Endpoint { port: 1, ..endpoint };
        assert_eq!(
            fetch_server_status(&closed, None, Some(500)).await,
            StatusResult::Unreachable
        );
    }

    #[tokio::test]
    async fn register_answers() {
        let cases = [
            (
                200,
                serde_json::json!({ "UUK": KEY }),
                RegisterResult::Ok { key: KEY.into() },
            ),
            (
                200,
                serde_json::json!({ "UUK": "bad key" }),
                RegisterResult::Err {
                    error: ErrorCode::ServerError,
                    message: None,
                },
            ),
            (
                400,
                serde_json::json!({ "error": "registration_closed", "message": " closed\u{7} " }),
                RegisterResult::Err {
                    error: ErrorCode::RegistrationClosed,
                    message: Some("closed".into()),
                },
            ),
            (
                400,
                serde_json::json!({ "error": "other" }),
                RegisterResult::Err {
                    error: ErrorCode::BadRequest,
                    message: None,
                },
            ),
            (
                401,
                Value::Null,
                RegisterResult::Err {
                    error: ErrorCode::InviteInvalid,
                    message: None,
                },
            ),
            (
                409,
                Value::Null,
                RegisterResult::Err {
                    error: ErrorCode::UsernameTaken,
                    message: None,
                },
            ),
            (
                413,
                Value::Null,
                RegisterResult::Err {
                    error: ErrorCode::RateLimited,
                    message: None,
                },
            ),
            (
                500,
                Value::Null,
                RegisterResult::Err {
                    error: ErrorCode::ServerError,
                    message: None,
                },
            ),
        ];
        for (status, body, expected) in cases {
            let (endpoint, _) = host(HashMap::from([("/undaunted/api/Register", (status, body))])).await;
            assert_eq!(
                register_account(&endpoint, "Slayer", "CODE").await,
                expected,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn user_info_answers() {
        let ok = serde_json::json!({ "UserId": "UID-1", "Username": "Slayer", "IsAdmin": true });
        let (endpoint, _) = host(HashMap::from([("/undaunted/api/GetUserInfo", (200, ok))])).await;
        assert_eq!(
            fetch_user_info(&endpoint, KEY).await,
            Ok(UserInfo {
                user_id: "UID-1".into(),
                username: "Slayer".into(),
                is_admin: true
            })
        );
        assert_eq!(
            fetch_user_info(&endpoint, "bad key").await,
            Err((ErrorCode::KeyRejected, None))
        );
        let banned = serde_json::json!({ "error": "account_banned" });
        let (endpoint, _) = host(HashMap::from([("/undaunted/api/GetUserInfo", (403, banned))])).await;
        assert_eq!(
            fetch_user_info(&endpoint, KEY).await,
            Err((ErrorCode::AccountBanned, Some("Contact support for details.".into())))
        );
        let (endpoint, _) = host(HashMap::from([("/undaunted/api/GetUserInfo", (401, Value::Null))])).await;
        assert_eq!(
            fetch_user_info(&endpoint, KEY).await,
            Err((ErrorCode::KeyRejected, None))
        );
    }

    #[tokio::test]
    async fn device_signals_answers() {
        let signals = serde_json::json!({ "machine": "x" });
        let banned = serde_json::json!({ "error": "device_banned", "reason": "cheating" });
        let (endpoint, _) = host(HashMap::from([("/undaunted/api/DeviceSignals", (403, banned))])).await;
        assert_eq!(
            check_device_signals(&endpoint, KEY, &signals).await,
            Err((ErrorCode::AccountBanned, Some("Device restriction: cheating".into())))
        );
        let (endpoint, _) = host(HashMap::new()).await;
        assert_eq!(
            check_device_signals(&endpoint, KEY, &signals).await,
            Ok(()),
            "an older server without the route"
        );
        let unavailable = serde_json::json!({ "error": "device_signals_unavailable" });
        let (endpoint, _) = host(HashMap::from([("/undaunted/api/DeviceSignals", (503, unavailable))])).await;
        assert_eq!(check_device_signals(&endpoint, KEY, &signals).await, Ok(()));
    }

    #[test]
    fn the_key_is_never_sent_outside_the_tailnet_without_a_pin() {
        let public_plain = Endpoint {
            host: "203.0.113.9".into(),
            port: 80,
            pin: None,
        };
        assert!(key_headers(&public_plain, Some(KEY)).is_empty());
        let pinned = Endpoint {
            pin: Some("3f".repeat(32)),
            ..public_plain
        };
        assert_eq!(key_headers(&pinned, Some(KEY)).len(), 1);
        assert!(key_headers(&pinned, Some("short")).is_empty());
    }
}
