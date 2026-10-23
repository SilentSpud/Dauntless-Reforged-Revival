//! What the host API reads from a server's answers (`src/main/hostapi.ts`): pure functions over a status
//! code and a JSON body, so the fuzz target `launcher_net` compiles them in without the network stack.
//! Every value comes from the network: anything unexpected is refused or dropped, never trusted.

use protocol::manifest::{MAX_RUNTIME_FILE_BYTES, RUNTIME_FILE_NAMES};
use protocol::status::{clean_text, iso_date};
use protocol::text::{is_unsafe_char, js_length, js_trim};
use serde_json::{Map, Value};

use crate::bridge::types::{ErrorCode, NewsItem, RuntimeFileInfo};

/// `Number.MAX_SAFE_INTEGER`
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value?.as_object()
}

/// `Number.isSafeInteger` for a JSON value: a number, whole, within ±(2^53 − 1). Read as JavaScript reads
/// it, through a double, so `1.0` counts and a larger integer does not.
fn safe_integer(value: Option<&Value>) -> Option<i64> {
    let n = value?.as_number()?.as_f64()?;
    (n.fract() == 0.0 && n.abs() <= MAX_SAFE_INTEGER).then_some(n as i64)
}

// ---------------------------------------------------------------- Register / GetUserInfo

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterResult {
    Ok { key: String },
    Err { error: ErrorCode, message: Option<String> },
}

/// The answer to `POST /reforged/api/Register`.
pub fn read_register(status: u16, body: Option<&Value>) -> RegisterResult {
    let body = object(body);
    let failed = |error| RegisterResult::Err { error, message: None };
    let server_error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    let message = body.and_then(|body| clean_text(body.get("message"), 200));
    match status {
        200 => match body.and_then(|body| body.get("UUK")).and_then(Value::as_str) {
            Some(key) if protocol::account::is_plausible_account_key(key) => RegisterResult::Ok { key: key.to_owned() },
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

/// `^[A-Za-z0-9_-]{1,128}$`: the user id goes on the game's command line (`-AUTH_LOGIN`, `-epicuserid`).
pub fn is_valid_user_id(text: &str) -> bool {
    (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// The answer to `GET /reforged/api/GetUserInfo`. A ban (or a disabled account) carries the server's
/// reason when it gave one, and no default text.
pub fn read_user_info(status: u16, body: Option<&Value>) -> Result<UserInfo, (ErrorCode, Option<String>)> {
    let body = object(body);
    if status == 403
        && let Some(body) = body
        && matches!(
            body.get("error").and_then(Value::as_str),
            Some("account_banned" | "account_disabled")
        )
    {
        return Err((ErrorCode::AccountBanned, clean_text(body.get("reason"), 500)));
    }
    match status {
        401 | 403 => return Err((ErrorCode::KeyRejected, None)),
        429 => return Err((ErrorCode::RateLimited, None)),
        200 => {}
        _ => return Err((ErrorCode::ServerError, None)),
    }
    let failed = (ErrorCode::ServerError, None);
    let body = body.ok_or(failed.clone())?;
    let user_id = clean_text(body.get("UserId"), 128).filter(|id| is_valid_user_id(id));
    let username = clean_text(body.get("Username"), 32);
    let (Some(user_id), Some(username)) = (user_id, username) else {
        return Err(failed);
    };
    Ok(UserInfo {
        user_id,
        username,
        is_admin: body.get("IsAdmin") == Some(&Value::Bool(true)),
    })
}

// ---------------------------------------------------------------- the game runtime

/// The runtime a server offers, as the launcher keeps it (Reforged's `RuntimeManifest`). Read more
/// loosely than `protocol::manifest::RuntimeManifest` is written: `build.name` is not read and an
/// unreadable changelist is null, as the TypeScript has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeManifest {
    pub version: String,
    pub changelist: Option<i64>,
    /// Exactly the two runtime files, sorted by name
    pub files: Vec<RuntimeFileInfo>,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `parseRuntimeManifest`: only the two known runtime files, each once, with a plain SHA-256 and a size
/// from 1 byte to 64 MiB; anything else refuses the whole manifest. The launcher never writes a file the
/// server names freely.
pub fn parse_runtime_manifest(raw: Option<&Value>) -> Option<RuntimeManifest> {
    let raw = object(raw)?;
    let list = raw.get("files")?.as_array()?;
    let version = clean_text(raw.get("version"), 48)?;
    let mut files: Vec<RuntimeFileInfo> = Vec::with_capacity(RUNTIME_FILE_NAMES.len());
    for file in list {
        let file = file.as_object()?;
        let name = file.get("name")?.as_str()?;
        if !RUNTIME_FILE_NAMES.contains(&name) || files.iter().any(|seen| seen.name == name) {
            return None;
        }
        let sha256 = file.get("sha256")?.as_str().filter(|hash| is_sha256_hex(hash))?;
        let size = safe_integer(file.get("size")).and_then(|size| u64::try_from(size).ok())?;
        if !(1..=MAX_RUNTIME_FILE_BYTES).contains(&size) {
            return None;
        }
        files.push(RuntimeFileInfo {
            name: name.to_owned(),
            size,
            sha256: sha256.to_owned(),
        });
    }
    // Both files, or the runtime would not load
    if files.len() != RUNTIME_FILE_NAMES.len() {
        return None;
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let changelist = object(raw.get("build")).and_then(|build| safe_integer(build.get("changelist")));
    Some(RuntimeManifest {
        version,
        changelist,
        files,
    })
}

// ---------------------------------------------------------------- news and the art pack

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
    let Some(items) = object(raw).and_then(|raw| raw.get("items")).and_then(Value::as_array) else {
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrandingIndex {
    /// (path, credit)
    pub backgrounds: Vec<(String, Option<String>)>,
    pub accent: Option<String>,
}

/// `/^\/content\/v1\/branding\/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,100}\.(jpg|jpeg|png|webp)$/i`, without `..`.
pub fn is_branding_path(path: &str) -> bool {
    const PREFIX: &str = "/content/v1/branding/";
    if path.contains("..")
        || path.len() < PREFIX.len()
        || !path.as_bytes()[..PREFIX.len()].eq_ignore_ascii_case(PREFIX.as_bytes())
    {
        return false;
    }
    let name = &path.as_bytes()[PREFIX.len()..];
    let first = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    let rest = |c: u8| first(c) || c == b'.';
    ["jpg", "jpeg", "png", "webp"].iter().any(|extension| {
        let suffix = extension.len() + 1;
        name.len() > suffix
            && name[name.len() - suffix..].eq_ignore_ascii_case(format!(".{extension}").as_bytes())
            && first(name[0])
            && name.len() - suffix - 1 <= 100
            && name[1..name.len() - suffix].iter().all(|c| rest(*c))
    })
}

pub fn parse_branding(raw: Option<&Value>) -> BrandingIndex {
    let mut out = BrandingIndex::default();
    let Some(raw) = object(raw) else {
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
