//! Everything that reads input from outside this server, as functions with no state: the request
//! path and headers a player's launcher sends (through the gateway), the host's `runtime.json`, and
//! the news document (from the host's file or from GitHub). The fuzz targets
//! (`rust/fuzz/fuzz_targets/content_*.rs`; docs/security/content.md) drive these functions.

use http::HeaderMap;
use protocol::manifest::{
    DEFAULT_CHANGELIST, MAX_RUNTIME_FILE_BYTES, RUNTIME_FILE_NAMES, RUNTIME_MANIFEST_PATH, RuntimeFile, RuntimeManifest,
};
use serde_json::{Value, json};
use service_kit::linelog as log;

pub const RUNTIME_FILES_PREFIX: &str = "/content/v1/runtime/files/";
pub const KEY_HEADER: &str = "x-reforged-user-api-key";
pub const BRANDING_PREFIX: &str = "/content/v1/branding/";

// ---- The request ----

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

/// Printable ASCII without spaces, at most 256 characters. Anything else can't be a key and is
/// refused without asking the backend (and can't be smuggled into the header sent to it).
pub fn is_plausible_key(key: &str) -> bool {
    (1..=256).contains(&key.len()) && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

// ---- runtime.json ----

fn is_version(text: &str) -> bool {
    (1..=48).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `JSON.stringify(x)` in an error message, `undefined` for a missing value.
fn shown(value: Option<&Value>) -> String {
    value.map_or_else(|| "undefined".to_owned(), service_kit::jsjson::stringify)
}

/// `ParseRuntimeIndex` (`src/runtime.ts`): the index when `raw` is exactly the expected shape, else
/// the reason.
pub fn parse_runtime_index(raw: &Value) -> Result<RuntimeManifest, String> {
    let Value::Object(object) = raw else {
        return Err("runtime.json must be an object".into());
    };
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .map_or("", service_kit::js::trim);
    if !is_version(version) {
        return Err("version must be 1-48 letters, digits, dots, dashes, plus signs or underscores".into());
    }
    // A list is an object to JavaScript too: it has no changelist, so the default applies
    let changelist = match object.get("build") {
        Some(Value::Object(build)) => build.get("changelist"),
        _ => None,
    };
    let changelist = match changelist {
        None => DEFAULT_CHANGELIST,
        Some(value) => match service_kit::js::safe_integer(Some(value)) {
            Some(n) if n > 0 => n as u64,
            _ => return Err("build.changelist must be a positive whole number".into()),
        },
    };
    let Some(Value::Array(entries)) = object.get("files") else {
        return Err("files must be a list".into());
    };
    let mut files: Vec<RuntimeFile> = Vec::new();
    let empty = serde_json::Map::new();
    for entry in entries {
        let entry = entry.as_object().unwrap_or(&empty);
        let name = match entry.get("name") {
            Some(Value::String(name)) if RUNTIME_FILE_NAMES.contains(&name.as_str()) => name.clone(),
            other => {
                return Err(format!(
                    "unknown file {} (only {})",
                    shown(other),
                    RUNTIME_FILE_NAMES.join(" and ")
                ));
            }
        };
        if files.iter().any(|other| other.name == name) {
            return Err(format!("{name} is listed twice"));
        }
        let size = match service_kit::js::safe_integer(entry.get("size")) {
            Some(size) if size >= 1 && size as u64 <= MAX_RUNTIME_FILE_BYTES => size as u64,
            _ => return Err(format!("{name}: size must be 1 to {MAX_RUNTIME_FILE_BYTES} bytes")),
        };
        let sha256 = match entry.get("sha256") {
            Some(Value::String(sha256)) if is_sha256(sha256) => sha256.clone(),
            _ => return Err(format!("{name}: sha256 must be 64 lowercase hex characters")),
        };
        files.push(RuntimeFile { name, size, sha256 });
    }
    for name in RUNTIME_FILE_NAMES {
        if !files.iter().any(|f| f.name == name) {
            return Err(format!("{name} is missing from the list"));
        }
    }
    Ok(RuntimeManifest::new(version.to_owned(), changelist, files))
}

/// runtime.json's bytes as Node reads them (`readFileSync(f, "utf8")`, a leading BOM dropped,
/// `JSON.parse`), then [`parse_runtime_index`]. The error is the reason, as the log shows it.
pub fn parse_runtime_json(bytes: &[u8]) -> Result<RuntimeManifest, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let raw = service_kit::jsjson::parse(text).map_err(|error| format!("runtime.json: {error}"))?;
    parse_runtime_index(&raw).map_err(|error| format!("runtime.json: {error}"))
}

// ---- News ----

const MAX_NEWS_ITEMS: usize = 50;
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 10000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsItem {
    pub date: String,
    pub title: String,
    pub body: String,
}

/// The first `max` UTF-16 units, as `slice(0, max)` (a cut surrogate pair loses its half).
pub fn slice_utf16(text: &str, max: usize) -> String {
    let mut units = 0;
    text.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= max
        })
        .collect()
}

fn clean_text(value: &str, max: usize, keep_newlines: bool) -> String {
    let text = value.replace("\r\n", "\n").replace('\r', "\n");
    let text: String = if keep_newlines {
        text.chars()
            .filter(|c| !matches!(*c as u32, 0x00..=0x08 | 0x0b..=0x1f | 0x7f))
            .collect()
    } else {
        text.chars()
            .map(|c| if (c as u32) < 0x20 || c == '\x7f' { ' ' } else { c })
            .collect()
    };
    slice_utf16(service_kit::js::trim(&text), max)
}

/// `ParseNews` (`src/news.ts`): the items, newest first; a document of the wrong shape is an error, a bad item is
/// skipped with a warning.
pub fn parse_news(raw: &Value) -> Result<Vec<NewsItem>, String> {
    let list = match raw {
        Value::Array(list) => list,
        Value::Object(o) => match o.get("items") {
            Some(Value::Array(list)) => list,
            _ => return Err("news file must be {\"items\": [...]} or an array".into()),
        },
        _ => return Err("news file must be {\"items\": [...]} or an array".into()),
    };
    let mut items = Vec::new();
    for entry in list {
        let Some(entry) = entry.as_object() else {
            log::warn("news item skipped: not an object", json!({}));
            continue;
        };
        let (Some(date), Some(title)) = (
            entry.get("date").and_then(Value::as_str),
            entry.get("title").and_then(Value::as_str),
        ) else {
            log::warn("news item skipped: needs a date and a title", json!({}));
            continue;
        };
        let Some(when) = service_kit::js::date_parse(date) else {
            log::warn(
                "news item skipped: date not understood",
                json!({"date": slice_utf16(date, 40)}),
            );
            continue;
        };
        let title = clean_text(title, MAX_TITLE, false);
        if title.is_empty() {
            log::warn("news item skipped: empty title", json!({}));
            continue;
        }
        let Some(date) = chrono::DateTime::from_timestamp_millis(when) else {
            log::warn(
                "news item skipped: date not understood",
                json!({"date": slice_utf16(date, 40)}),
            );
            continue;
        };
        items.push(NewsItem {
            date: date.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            title,
            body: entry
                .get("body")
                .and_then(Value::as_str)
                .map_or_else(String::new, |b| clean_text(b, MAX_BODY, true)),
        });
    }
    // Newest first; the sort is stable, so same-day items keep the file's order
    items.sort_by(|a, b| b.date.cmp(&a.date));
    items.truncate(MAX_NEWS_ITEMS);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

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
