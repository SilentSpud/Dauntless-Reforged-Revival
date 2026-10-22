//! The host's optional art pack (`src/branding.ts`): background images for the launcher and an accent
//! colour, from a folder the host controls (`CONTENT_BRANDING_DIR`). The folder holds jpg/png/webp
//! files and, optionally, branding.json:
//!
//! ```json
//! { "accent": "#c8a24a", "backgrounds": [ { "file": "harbour-dusk.jpg", "credit": "Photo by a friend" }, "second-image.webp" ] }
//! ```
//!
//! Without "backgrounds", every image in the folder is used in name order. The folder is re-read at
//! most every 10 s, so the host can drop images in without a restart.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::log;

pub const BRANDING_PREFIX: &str = "/content/v1/branding/";
pub const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_BACKGROUNDS: usize = 50;
const MAX_CREDIT: usize = 200;
const RESCAN_MS: i64 = 10 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub file: String,
    pub credit: Option<String>,
    pub content_type: &'static str,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub accent: Option<String>,
    /// In display order
    pub images: Vec<Image>,
}

/// `IMAGE_NAME`: `[A-Za-z0-9][A-Za-z0-9_-]*(\.[A-Za-z0-9_-]+)*\.(jpe?g|png|webp)`, case-insensitive.
pub fn is_safe_image_name(name: &str) -> bool {
    if name.len() > 100 {
        return false;
    }
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let word = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let mut parts = stem.split('.');
    let first = parts.next().unwrap_or("");
    matches!(extension.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp")
        && word(first)
        && first.as_bytes()[0].is_ascii_alphanumeric()
        && parts.all(word)
}

pub fn content_type_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// The first bytes must match the extension, so a renamed file of any other kind is never served.
pub fn magic_matches(head: &[u8], content_type: &str) -> bool {
    match content_type {
        "image/jpeg" => head.len() >= 3 && head[..3] == [0xff, 0xd8, 0xff],
        "image/png" => head.len() >= 8 && head[..8] == [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
        "image/webp" => head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP",
        _ => false,
    }
}

/// JavaScript's `\s`.
fn is_js_space(c: char) -> bool {
    c.is_whitespace() && c != '\u{85}' || c == '\u{feff}'
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

pub fn clean_credit(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    let spaced: String = text
        .chars()
        .map(|c| if (c as u32) < 0x20 || c == '\x7f' { ' ' } else { c })
        .collect();
    let mut collapsed = String::new();
    let mut in_space = false;
    for c in spaced.chars() {
        if is_js_space(c) {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }
    let cleaned = slice_utf16(service_kit::js::trim(&collapsed), MAX_CREDIT);
    (!cleaned.is_empty()).then_some(cleaned)
}

pub fn clean_accent(value: Option<&Value>) -> Option<String> {
    let text = service_kit::js::trim(value?.as_str()?);
    let hex = text.strip_prefix('#')?;
    ((hex.len() == 3 || hex.len() == 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_lowercase())
}

fn read_head(file: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(12);
    std::fs::File::open(file)?.take(12).read_to_end(&mut head)?;
    Ok(head)
}

/// `ScanBranding`: reads the folder once. Never fails: a broken art pack means no art.
pub fn scan(dir: Option<&Path>) -> State {
    let Some(dir) = dir else {
        return State::default();
    };
    let names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(error) => {
            log::warn(
                "branding folder unreadable",
                json!({"dir": dir.display().to_string(), "error": error.to_string()}),
            );
            return State::default();
        }
    };
    let mut config = serde_json::Map::new();
    let config_path = dir.join("branding.json");
    if config_path.exists() {
        match std::fs::read_to_string(&config_path)
            .map_err(|e| e.to_string())
            .and_then(|t| {
                serde_json::from_str::<Value>(t.strip_prefix('\u{feff}').unwrap_or(&t)).map_err(|e| e.to_string())
            }) {
            Ok(Value::Object(object)) => config = object,
            Ok(_) => log::warn(
                "branding.json is not an object; ignoring it",
                json!({"dir": dir.display().to_string()}),
            ),
            Err(error) => log::warn(
                "branding.json is not valid JSON; ignoring it",
                json!({"dir": dir.display().to_string(), "error": error}),
            ),
        }
    }
    let wanted: Vec<(Option<Value>, Option<Value>)> = match config.get("backgrounds") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(_) => (Some(item.clone()), None),
                Value::Object(o) => (o.get("file").cloned(), o.get("credit").cloned()),
                _ => (None, None),
            })
            .collect(),
        _ => {
            let mut safe: Vec<&String> = names.iter().filter(|n| is_safe_image_name(n)).collect();
            safe.sort();
            safe.into_iter().map(|n| (Some(json!(n)), None)).collect()
        }
    };
    let mut images: Vec<Image> = Vec::new();
    for (file, credit) in wanted {
        if images.len() >= MAX_BACKGROUNDS {
            break;
        }
        let name = file.as_ref().and_then(Value::as_str);
        let Some(name) = name
            .filter(|n| is_safe_image_name(n) && names.iter().any(|x| x == n) && !images.iter().any(|i| i.file == *n))
        else {
            let shown = match &file {
                None => "undefined".to_owned(),
                Some(Value::String(s)) => s.clone(),
                Some(Value::Object(_)) => "[object Object]".to_owned(),
                Some(other) => service_kit::js::string(Some(other)),
            };
            log::warn(
                "branding image skipped (bad name, missing or listed twice)",
                json!({"file": slice_utf16(&shown, 120)}),
            );
            continue;
        };
        let full = dir.join(name);
        let meta = match std::fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(error) => {
                log::warn(
                    "branding image skipped (unreadable)",
                    json!({"file": name, "error": error.to_string()}),
                );
                continue;
            }
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
            log::warn(
                "branding image skipped (not a regular file, empty or over 25 MB)",
                json!({"file": name}),
            );
            continue;
        }
        let content_type = content_type_for(name);
        match read_head(&full) {
            Ok(head) if magic_matches(&head, content_type) => {}
            Ok(_) => {
                log::warn(
                    "branding image skipped (contents don't match the extension)",
                    json!({"file": name}),
                );
                continue;
            }
            Err(error) => {
                log::warn(
                    "branding image skipped (unreadable)",
                    json!({"file": name, "error": error.to_string()}),
                );
                continue;
            }
        }
        images.push(Image {
            file: name.to_owned(),
            credit: clean_credit(credit.as_ref()),
            content_type,
            size: meta.len(),
            modified: meta.modified().ok(),
        });
    }
    if config.contains_key("accent") && clean_accent(config.get("accent")).is_none() {
        log::warn("branding.json accent ignored: use #rgb or #rrggbb", json!({}));
    }
    State {
        accent: clean_accent(config.get("accent")),
        images,
    }
}

pub fn to_body(state: &State) -> Value {
    let backgrounds: Vec<Value> = state
        .images
        .iter()
        .map(|i| json!({"url": format!("{BRANDING_PREFIX}{}", i.file), "credit": i.credit}))
        .collect();
    json!({"backgrounds": backgrounds, "accent": state.accent})
}

/// Re-reads the folder at most every 10 s.
pub struct Branding {
    dir: Option<PathBuf>,
    now: crate::auth::Clock,
    state: Mutex<(State, Option<i64>)>,
}

impl Branding {
    pub fn new(dir: Option<PathBuf>, now: crate::auth::Clock) -> Self {
        Self {
            dir,
            now,
            state: Mutex::new((State::default(), None)),
        }
    }

    pub fn current(&self) -> State {
        let now = (self.now)();
        let mut guard = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.1.is_none_or(|at| now - at >= RESCAN_MS) {
            guard.0 = scan(self.dir.as_deref());
            guard.1 = Some(now);
        }
        guard.0.clone()
    }

    pub fn body(&self) -> Value {
        to_body(&self.current())
    }

    /// Exact name lookup against the current scan.
    pub fn image(&self, name: &str) -> Option<(Image, PathBuf)> {
        let dir = self.dir.as_ref()?;
        if !is_safe_image_name(name) {
            return None;
        }
        let image = self.current().images.into_iter().find(|i| i.file == name)?;
        let full = dir.join(&image.file);
        Some((image, full))
    }
}

#[cfg(test)]
mod tests {
    //! test/branding-news.test.ts, the branding half
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    // Synthetic image bytes: just the right signature plus filler. No real artwork anywhere in the tests.
    fn jpeg() -> Vec<u8> {
        [&[0xff, 0xd8, 0xff, 0xe0][..], &[1; 60]].concat()
    }
    fn png() -> Vec<u8> {
        [&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a][..], &[2; 60]].concat()
    }
    fn webp() -> Vec<u8> {
        [&b"RIFF"[..], &[0x40, 0, 0, 0], b"WEBP", &[3; 52]].concat()
    }

    fn clock() -> crate::auth::Clock {
        Arc::new(|| 0)
    }

    #[test]
    fn image_names() {
        for good in ["a.jpg", "Harbour-Dusk_2.JPEG", "x.png", "bg.v2.webp"] {
            assert!(is_safe_image_name(good), "{good}");
        }
        let long = format!("{}.jpg", "x".repeat(100));
        for bad in [
            "../a.jpg",
            "a/b.jpg",
            "a\\b.jpg",
            ".a.jpg",
            "a..jpg",
            "a.gif",
            "a.svg",
            "a.html",
            "a.jpg.exe",
            "a b.jpg",
            "",
            "branding.json",
            long.as_str(),
        ] {
            assert!(!is_safe_image_name(bad), "{bad}");
        }
    }

    #[test]
    fn signatures_must_match_the_extension() {
        assert!(magic_matches(&jpeg(), "image/jpeg"));
        assert!(magic_matches(&png(), "image/png"));
        assert!(magic_matches(&webp(), "image/webp"));
        assert!(!magic_matches(&png(), "image/jpeg"));
        assert!(!magic_matches(b"<html>", "image/png"));
        assert!(!magic_matches(&[], "image/webp"));
    }

    #[test]
    fn accent_and_credit_cleaning() {
        assert_eq!(clean_accent(Some(&json!("#C8A24A"))).as_deref(), Some("#c8a24a"));
        assert_eq!(clean_accent(Some(&json!(" #abc "))).as_deref(), Some("#abc"));
        for bad in [
            json!("red"),
            json!("#abcd"),
            json!("#12345g"),
            json!("url(x)"),
            json!(5),
            Value::Null,
        ] {
            assert_eq!(clean_accent(Some(&bad)), None, "{bad}");
        }
        assert_eq!(
            clean_credit(Some(&json!("  Photo\nby   a friend "))).as_deref(),
            Some("Photo by a friend")
        );
        assert_eq!(clean_credit(Some(&json!(""))), None);
        assert_eq!(clean_credit(Some(&json!(7))), None);
        assert_eq!(clean_credit(Some(&json!("x".repeat(500)))).map(|c| c.len()), Some(200));
    }

    #[test]
    fn no_branding_folder_means_no_art() {
        let branding = Branding::new(None, clock());
        assert_eq!(branding.body(), json!({"backgrounds": [], "accent": null}));
        assert!(branding.image("a.jpg").is_none());
    }

    #[test]
    fn without_branding_json_every_valid_image_is_listed_in_name_order() {
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str| dir.path().join(name);
        std::fs::write(at("b.png"), png()).unwrap();
        std::fs::write(at("a.jpg"), jpeg()).unwrap();
        std::fs::write(at("c.webp"), webp()).unwrap();
        std::fs::write(at("fake.jpg"), "<script>alert(1)</script>").unwrap();
        std::fs::write(at("notes.txt"), "hello").unwrap();
        std::fs::write(at("empty.png"), "").unwrap();
        std::fs::create_dir(at("sub.jpg")).unwrap();

        let state = scan(Some(dir.path()));
        assert_eq!(
            state.images.iter().map(|i| i.file.as_str()).collect::<Vec<_>>(),
            ["a.jpg", "b.png", "c.webp"]
        );
        assert_eq!(state.images[1].content_type, "image/png");
        assert_eq!(state.accent, None);

        let branding = Branding::new(Some(dir.path().into()), clock());
        assert_eq!(
            branding.body(),
            json!({
                "backgrounds": [
                    {"url": "/content/v1/branding/a.jpg", "credit": null},
                    {"url": "/content/v1/branding/b.png", "credit": null},
                    {"url": "/content/v1/branding/c.webp", "credit": null},
                ],
                "accent": null,
            })
        );
        assert_eq!(branding.image("a.jpg").map(|(_, full)| full), Some(at("a.jpg")));
        for refused in ["fake.jpg", "notes.txt", "../a.jpg"] {
            assert!(branding.image(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn branding_json_sets_order_credits_and_the_accent_and_can_t_reach_outside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), jpeg()).unwrap();
        std::fs::write(dir.path().join("b.png"), png()).unwrap();
        std::fs::write(
            dir.path().join("branding.json"),
            json!({
                "accent": "#D4A24C",
                "backgrounds": [
                    {"file": "b.png", "credit": "Photo by a friend"},
                    "a.jpg",
                    {"file": "../../secret.jpg", "credit": "nope"},
                    {"file": "missing.jpg"},
                    "a.jpg",
                ],
            })
            .to_string(),
        )
        .unwrap();
        let (state, lines) = crate::log::tests::captured(|| scan(Some(dir.path())));
        assert_eq!(
            state
                .images
                .iter()
                .map(|i| (i.file.as_str(), i.credit.as_deref()))
                .collect::<Vec<_>>(),
            [("b.png", Some("Photo by a friend")), ("a.jpg", None)]
        );
        assert_eq!(state.accent.as_deref(), Some("#d4a24c"));
        assert!(lines.len() >= 3, "skipped entries are logged");
    }

    #[test]
    fn a_broken_branding_json_or_a_missing_folder_never_throws() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), jpeg()).unwrap();
        std::fs::write(dir.path().join("branding.json"), "{ nope").unwrap();
        let state = scan(Some(dir.path()));
        assert_eq!(
            state.images.iter().map(|i| i.file.as_str()).collect::<Vec<_>>(),
            ["a.jpg"]
        );
        assert!(scan(Some(&dir.path().join("does-not-exist"))).images.is_empty());
    }

    #[test]
    fn the_folder_is_re_read_after_the_rescan_interval() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicI64::new(0));
        let read = now.clone();
        let branding = Branding::new(Some(dir.path().into()), Arc::new(move || read.load(Ordering::SeqCst)));
        let count = |b: &Branding| b.body()["backgrounds"].as_array().unwrap().len();
        assert_eq!(count(&branding), 0);
        std::fs::write(dir.path().join("a.jpg"), jpeg()).unwrap();
        now.fetch_add(5_000, Ordering::SeqCst);
        assert_eq!(count(&branding), 0, "cached");
        now.fetch_add(5_000, Ordering::SeqCst);
        assert_eq!(count(&branding), 1, "rescanned");
    }
}
