//! Calls to the host, row L21: `src/main/hostapi.ts`. The backend's launcher API (`/reforged/api`, the
//! same contract as Dauntless Revived's `/undaunted/api`) and the content server behind the gateway
//! (`/content/v1`). Reforged plays 1.14.7 only, so there is no 1.4.4 flavour of any call (RD5).
//!
//! The account key is sent only in the `x-reforged-user-api-key` header, only to the invite's host (in
//! public mode only over the TLS connection pinned to the invite's certificate fingerprint): GetUserInfo,
//! ServerStatus (for the player list) and, through RA7, the runtime downloads. It is never logged and
//! never part of an error.

use std::time::Duration;

use bytes::Bytes;
use hyper::Method;
use protocol::account::REGISTER_PATH;
use protocol::manifest::RUNTIME_MANIFEST_PATH;
use serde_json::Value;

use super::http::{Endpoint, HttpError, HttpResponse, RequestOptions, parse_json_body, request};
use crate::bridge::types::{ErrorCode, NewsItem};
use crate::core::invite::is_private_mode_host;
use crate::core::pins::CONNECT_TIMEOUT_MS;
use crate::core::status::{STATUS_PATH, ServerStatus, parse_server_status};
use crate::core::username::is_plausible_account_key;

/// The parsers, without the network: the fuzz target `launcher_net` compiles this file in
pub mod parse;

pub use parse::{
    BrandingIndex, RegisterResult, RuntimeManifest, UserInfo, is_branding_path, parse_branding, parse_news,
    parse_runtime_manifest, sniff_image,
};

pub const KEY_HEADER: &str = "x-reforged-user-api-key";
const USER_INFO_PATH: &str = "/reforged/api/GetUserInfo";
/// An older server's status route; it never gets the key
const LEGACY_STATUS_PATH: &str = "/dauntless-status";

fn options(method: Method, timeout_ms: u64, max_bytes: usize) -> RequestOptions {
    RequestOptions {
        method,
        ..RequestOptions::get(Duration::from_millis(timeout_ms), max_bytes)
    }
}

fn is_pin(error: &HttpError) -> bool {
    matches!(error, HttpError::Pin { .. })
}

/// The key goes only where every other key-carrying request goes: to the invite's host, over the TLS
/// connection pinned to the invite's certificate (public mode) or plain HTTP inside the tailnet or to
/// this PC (private mode, which `request` enforces as well). Otherwise no header at all.
pub fn key_headers(endpoint: &Endpoint, key: Option<&str>) -> Vec<(&'static str, String)> {
    match key {
        Some(key)
            if is_plausible_account_key(key) && (endpoint.pin.is_some() || is_private_mode_host(&endpoint.host)) =>
        {
            vec![(KEY_HEADER, key.to_owned())]
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- ServerStatus

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusResult {
    Ok(ServerStatus),
    /// Something like a Reforged server, without ServerStatus
    Unsupported,
    BadAnswer,
    /// Public mode: the certificate is not the one from the invite
    CertMismatch,
    Unreachable,
}

fn unreachable_or_pin(error: &HttpError) -> StatusResult {
    if is_pin(error) {
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
    let response = match request(endpoint, STATUS_PATH, request_options).await {
        Ok(response) => response,
        Err(error) => return unreachable_or_pin(&error),
    };
    if response.status == 200 {
        return parse_json_body(&response)
            .and_then(|body| parse_server_status(&body))
            .map_or(StatusResult::BadAnswer, StatusResult::Ok);
    }
    if response.status == 404 {
        // An older server: still a real one if /dauntless-status answers like one
        match request(
            endpoint,
            LEGACY_STATUS_PATH,
            options(Method::GET, timeout_ms, 64 * 1024),
        )
        .await
        {
            Ok(legacy) => {
                let body = parse_json_body(&legacy);
                let shows = body
                    .as_ref()
                    .and_then(Value::as_object)
                    .is_some_and(|body| body.contains_key("show-status"));
                if legacy.status == 200 && shows {
                    return StatusResult::Unsupported;
                }
            }
            Err(error) => return unreachable_or_pin(&error),
        }
    }
    StatusResult::BadAnswer
}

// ---------------------------------------------------------------- Register / GetUserInfo

/// `POST /reforged/api/Register`. The key in a successful answer goes to the keystore, nowhere else.
pub async fn register_account(endpoint: &Endpoint, username: &str, invite_code: &str) -> RegisterResult {
    let mut request_options = options(Method::POST, 20000, 64 * 1024);
    let body = protocol::account::RegisterRequest {
        username: username.to_owned(),
        invite_code: invite_code.to_owned(),
    };
    request_options.body = serde_json::to_string(&body).ok();
    match request(endpoint, REGISTER_PATH, request_options).await {
        Ok(response) => parse::read_register(response.status, parse_json_body(&response).as_ref()),
        Err(error) => RegisterResult::Err {
            error: if is_pin(&error) {
                ErrorCode::CertMismatch
            } else {
                ErrorCode::ServerUnreachable
            },
            message: None,
        },
    }
}

/// `GET /reforged/api/GetUserInfo`, with the key under the same rule as ServerStatus.
pub async fn fetch_user_info(endpoint: &Endpoint, key: &str) -> Result<UserInfo, (ErrorCode, Option<String>)> {
    if !is_plausible_account_key(key) {
        return Err((ErrorCode::KeyRejected, None));
    }
    let mut request_options = options(Method::GET, 10000, 64 * 1024);
    request_options.headers = key_headers(endpoint, Some(key));
    match request(endpoint, USER_INFO_PATH, request_options).await {
        Ok(response) => parse::read_user_info(response.status, parse_json_body(&response).as_ref()),
        Err(error) if is_pin(&error) => Err((ErrorCode::CertMismatch, None)),
        Err(_) => Err((ErrorCode::ServerUnreachable, None)),
    }
}

// ---------------------------------------------------------------- the game runtime

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeManifestResult {
    Ok(RuntimeManifest),
    /// The server has no runtime to hand out yet (404 or 503)
    None,
    BadAnswer,
    CertMismatch,
    Unreachable,
}

/// `GET /content/v1/runtime/manifest`, without the key.
pub async fn fetch_runtime_manifest(endpoint: &Endpoint) -> RuntimeManifestResult {
    let response = match request(endpoint, RUNTIME_MANIFEST_PATH, options(Method::GET, 10000, 64 * 1024)).await {
        Ok(response) => response,
        Err(error) if is_pin(&error) => return RuntimeManifestResult::CertMismatch,
        Err(_) => return RuntimeManifestResult::Unreachable,
    };
    match response.status {
        404 | 503 => RuntimeManifestResult::None,
        200 => parse_runtime_manifest(parse_json_body(&response).as_ref())
            .map_or(RuntimeManifestResult::BadAnswer, RuntimeManifestResult::Ok),
        _ => RuntimeManifestResult::BadAnswer,
    }
}

// ---------------------------------------------------------------- news and the art pack

pub async fn fetch_news(endpoint: &Endpoint, fallback: Vec<NewsItem>) -> Vec<NewsItem> {
    match request(endpoint, "/content/v1/news", options(Method::GET, 10000, 512 * 1024)).await {
        Ok(response) if response.status == 200 => parse_news(parse_json_body(&response).as_ref()),
        _ => fallback,
    }
}

pub async fn fetch_branding_index(endpoint: &Endpoint) -> BrandingIndex {
    match request(endpoint, "/content/v1/branding", options(Method::GET, 10000, 64 * 1024)).await {
        Ok(response) if response.status == 200 => parse_branding(parse_json_body(&response).as_ref()),
        _ => BrandingIndex::default(),
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
mod tests;
