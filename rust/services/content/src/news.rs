//! The host's news for the launcher (`src/news.ts`), from `CONTENT_NEWS_FILE` (optional), JSON as
//! `{"items": [...]}` or just the array:
//!
//! ```json
//! { "items": [ { "date": "2026-09-21", "title": "Friends night on Friday", "body": "Plain text.\nNew lines are kept." } ] }
//! ```
//!
//! Bodies are plain text, nothing is interpreted. The file is re-read when it changes (checked at most
//! every 5 s); if an edit breaks it, the last good version keeps being served. `CONTENT_NEWS_URL` (a
//! raw.githubusercontent.com file) replaces it once fetched, re-fetched in the background at most once
//! a minute.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::branding::slice_utf16;
use crate::log;

const MAX_ITEMS: usize = 50;
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 10000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const RECHECK_MS: i64 = 5 * 1000;
const REMOTE_RECHECK_MS: i64 = 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub date: String,
    pub title: String,
    pub body: String,
}

/// `GitHubNewsUrl`: an HTTPS file URL on raw.githubusercontent.com, or an error that stops the start.
pub fn github_news_url(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let error = || "CONTENT_NEWS_URL must be a raw.githubusercontent.com HTTPS file URL".to_owned();
    let url = url::Url::parse(value).map_err(|_| error())?;
    if url.scheme() != "https"
        || url.host_str() != Some("raw.githubusercontent.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().split('/').count() < 5
    {
        return Err(error());
    }
    Ok(Some(url.as_str().to_owned()))
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

/// `ParseNews`: the items, newest first; a document of the wrong shape is an error, a bad item is
/// skipped with a warning.
pub fn parse(raw: &Value) -> Result<Vec<Item>, String> {
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
        items.push(Item {
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
    items.truncate(MAX_ITEMS);
    Ok(items)
}

fn to_json(items: &[Item]) -> Value {
    json!({"items": items.iter().map(|i| json!({"date": i.date, "title": i.title, "body": i.body})).collect::<Vec<_>>()})
}

/// A fetched news file: its items and its ETag.
type Fetched = (Vec<Item>, Option<String>);

#[derive(Default)]
struct State {
    items: Vec<Item>,
    signature: String,
    checked_at: Option<i64>,
    remote_checked_at: Option<i64>,
    remote_busy: bool,
    remote_items: Option<Vec<Item>>,
    etag: Option<String>,
}

pub struct News {
    file: Option<PathBuf>,
    remote_url: Option<String>,
    now: crate::auth::Clock,
    state: Mutex<State>,
}

impl News {
    pub fn new(file: Option<PathBuf>, remote_url: Option<String>, now: crate::auth::Clock) -> Self {
        Self {
            file,
            remote_url,
            now,
            state: Mutex::default(),
        }
    }

    /// `{items}`: the remote news once fetched, else the file's.
    pub fn body(self: &Arc<Self>) -> Value {
        self.refresh();
        let news = self.clone();
        tokio::spawn(async move { news.refresh_remote().await });
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        to_json(state.remote_items.as_ref().unwrap_or(&state.items))
    }

    /// One bounded background fetch per minute; downloads and launcher polling never wait on GitHub.
    pub async fn refresh_remote(&self) {
        let Some(url) = &self.remote_url else {
            return;
        };
        let etag = {
            let now = (self.now)();
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.remote_busy || state.remote_checked_at.is_some_and(|at| now - at < REMOTE_RECHECK_MS) {
                return;
            }
            state.remote_busy = true;
            state.remote_checked_at = Some(now);
            state.etag.clone()
        };
        let mut headers = Vec::new();
        if let Some(etag) = &etag {
            headers.push(("If-None-Match", etag.as_str()));
        }
        let result: Result<Option<Fetched>, String> = async {
            let reply = crate::client::get(url, &headers, Duration::from_secs(5), MAX_FILE_BYTES as usize).await?;
            if reply.status.as_u16() == 304 {
                return Ok(None);
            }
            if !reply.status.is_success() {
                return Err(format!("GitHub news HTTP {}", reply.status.as_u16()));
            }
            let body = reply.body.map_err(|_| "GitHub news exceeds 1 MB".to_owned())?;
            let text = String::from_utf8_lossy(&body);
            let raw: Value =
                serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).map_err(|e| e.to_string())?;
            let items = parse(&raw)?;
            let original = match &raw {
                Value::Array(list) => list.len(),
                other => other.get("items").and_then(Value::as_array).map_or(0, Vec::len),
            };
            if original > 0 && items.is_empty() {
                return Err("GitHub news contains no usable entries".into());
            }
            let etag = reply
                .headers
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            Ok(Some((items, etag)))
        }
        .await;
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.remote_busy = false;
        match result {
            Ok(None) => {}
            Ok(Some((items, etag))) => {
                state.remote_items = Some(items);
                state.etag = etag;
            }
            Err(error) => log::warn(
                "GitHub news unavailable; keeping last good news",
                json!({"error": error}),
            ),
        }
    }

    fn refresh(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let now = (self.now)();
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.checked_at.is_some_and(|at| now - at < RECHECK_MS) {
            return;
        }
        state.checked_at = Some(now);
        let shown = file.display().to_string();
        let Ok(meta) = std::fs::metadata(file) else {
            if state.signature != "missing" {
                log::warn("news file not found; serving no news", json!({"file": shown}));
                state.signature = "missing".into();
                state.items.clear();
            }
            return;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let signature = format!("{}:{modified}", meta.len());
        if signature == state.signature {
            return;
        }
        state.signature = signature;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            log::warn(
                "news file skipped: not a regular file or over 1 MB",
                json!({"file": shown}),
            );
            return;
        }
        let parsed = std::fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|t| {
                serde_json::from_str::<Value>(t.strip_prefix('\u{feff}').unwrap_or(&t)).map_err(|e| e.to_string())
            })
            .and_then(|v| parse(&v));
        match parsed {
            Ok(items) => {
                log::info("news loaded", json!({"file": shown, "items": items.len()}));
                state.items = items;
            }
            Err(error) => log::warn(
                "news file could not be read; keeping the last good version",
                json!({"file": shown, "error": error}),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    //! test/branding-news.test.ts (the news half) and test/github-news.test.ts
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    fn clock(now: &Arc<AtomicI64>) -> crate::auth::Clock {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn titles(body: &Value) -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["title"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn news_parsing_validation_cleaning_and_newest_first() {
        let items = parse(&json!({"items": [
            {"date": "2026-09-01", "title": "Older", "body": "line one\r\nline two"},
            {"date": "2026-09-21T18:00:00Z", "title": "  Newest\n title ", "body": "<b>not html</b>"},
            {"date": "not a date", "title": "skipped"},
            {"title": "no date"},
            {"date": "2026-09-10", "title": "   "},
            "not an object",
            {"date": "2026-09-10", "title": "No body"},
        ]}))
        .unwrap();
        let item = |date: &str, title: &str, body: &str| Item {
            date: date.into(),
            title: title.into(),
            body: body.into(),
        };
        assert_eq!(
            items,
            [
                item("2026-09-21T18:00:00.000Z", "Newest  title", "<b>not html</b>"),
                item("2026-09-10T00:00:00.000Z", "No body", ""),
                item("2026-09-01T00:00:00.000Z", "Older", "line one\nline two"),
            ]
        );
        assert_eq!(
            parse(&json!([{"date": "2026-01-01", "title": "bare array"}]))
                .unwrap()
                .len(),
            1
        );
        assert!(parse(&json!({"news": []})).is_err());
        assert!(parse(&json!("x")).is_err());
    }

    #[tokio::test]
    async fn news_file_none_missing_reload_on_change_last_good_kept_on_a_broken_edit() {
        let now = Arc::new(AtomicI64::new(0));
        assert_eq!(
            Arc::new(News::new(None, None, clock(&now))).body(),
            json!({"items": []})
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("news.json");
        let news = Arc::new(News::new(Some(file.clone()), None, clock(&now)));
        assert_eq!(news.body(), json!({"items": []}), "missing file");

        std::fs::write(
            &file,
            json!({"items": [{"date": "2026-09-21", "title": "First"}]}).to_string(),
        )
        .unwrap();
        now.fetch_add(6_000, Ordering::SeqCst);
        assert_eq!(titles(&news.body()), ["First"]);

        std::fs::write(&file, "{ broken json").unwrap();
        now.fetch_add(6_000, Ordering::SeqCst);
        assert_eq!(titles(&news.body()), ["First"], "last good version kept");

        std::fs::write(
            &file,
            json!([{"date": "2026-09-22", "title": "Second"}, {"date": "2026-09-21", "title": "First"}]).to_string(),
        )
        .unwrap();
        now.fetch_add(6_000, Ordering::SeqCst);
        assert_eq!(titles(&news.body()), ["Second", "First"]);
    }

    /// What the fake GitHub answers next: status, body, ETag. It records each request's If-None-Match
    /// and can hold its answer until released.
    #[derive(Default)]
    struct Fake {
        answer: Mutex<(u16, String, Option<String>)>,
        calls: AtomicUsize,
        if_none_match: Mutex<Option<String>>,
        gate: Option<Arc<tokio::sync::Semaphore>>,
    }

    async fn fake_github(fake: Arc<Fake>) -> String {
        let handler = move |request: axum::extract::Request| {
            let fake = fake.clone();
            async move {
                fake.calls.fetch_add(1, Ordering::SeqCst);
                *fake.if_none_match.lock().unwrap() = request
                    .headers()
                    .get("if-none-match")
                    .map(|v| v.to_str().unwrap().to_owned());
                if let Some(gate) = &fake.gate {
                    let _ = gate.acquire().await;
                }
                let (status, body, etag) = fake.answer.lock().unwrap().clone();
                let mut response = axum::response::Response::new(axum::body::Body::from(body));
                *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
                if let Some(etag) = etag {
                    response.headers_mut().insert("etag", etag.parse().unwrap());
                }
                response
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, axum::Router::new().fallback(handler)).await });
        // Any URL will do here: CONTENT_NEWS_URL is checked by the config
        format!("http://{address}/o/r/launcher-news/news.json")
    }

    fn item(date: &str, title: &str) -> Value {
        json!({"date": date, "title": title, "body": "Public notes only"})
    }

    #[tokio::test]
    async fn github_feed_sorts_newest_first_uses_the_etag_and_keeps_good_news_on_errors() {
        let fake = Arc::new(Fake::default());
        *fake.answer.lock().unwrap() = (
            200,
            json!({"items": [item("2026-10-01", "Old"), item("2026-10-02", "New")]}).to_string(),
            Some("\"v1\"".into()),
        );
        let url = fake_github(fake.clone()).await;
        let now = Arc::new(AtomicI64::new(0));
        let news = Arc::new(News::new(None, Some(url), clock(&now)));
        news.refresh_remote().await;
        assert_eq!(titles(&news.body())[0], "New");
        news.refresh_remote().await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        now.fetch_add(60_000, Ordering::SeqCst);
        *fake.answer.lock().unwrap() = (304, String::new(), None);
        let (_, lines) = crate::log::tests::captured_async(news.refresh_remote()).await;
        assert!(
            !lines.iter().any(|l| l.contains("GitHub news unavailable")),
            "a 304 is not an error"
        );
        assert_eq!(fake.if_none_match.lock().unwrap().as_deref(), Some("\"v1\""));
        assert_eq!(titles(&news.body()).len(), 2);
        for (status, body) in [
            (503, "oops".to_owned()),
            (200, "invalid JSON".to_owned()),
            (200, json!({"items": [{}]}).to_string()),
            (200, "x".repeat(1024 * 1024 + 1)),
        ] {
            now.fetch_add(60_000, Ordering::SeqCst);
            *fake.answer.lock().unwrap() = (status, body, None);
            news.refresh_remote().await;
            assert_eq!(titles(&news.body())[0], "New");
        }
        now.fetch_add(60_000, Ordering::SeqCst);
        *fake.answer.lock().unwrap() = (200, "{\"items\":[]}".into(), None);
        news.refresh_remote().await;
        assert_eq!(news.body(), json!({"items": []}));
    }

    #[test]
    fn remote_news_accepts_only_the_curated_github_file_host() {
        let url = "https://raw.githubusercontent.com/Vvoidddd/dauntless-revived/launcher-news/news.json";
        assert_eq!(github_news_url(Some(url)).unwrap().as_deref(), Some(url));
        let with_token = format!("{url}?token=secret");
        for bad in [
            "http://raw.githubusercontent.com/a/b/c/d",
            "https://example.com/feed",
            "https://raw.githubusercontent.com@127.0.0.1/a/b/c/d",
            with_token.as_str(),
            // And DR's own cases
            "https://raw.githubusercontent.com/o/r/news.json",
            "https://raw.githubusercontent.com:444/o/r/main/news.json",
            "https://raw.githubusercontent.com/o/r/main/news.json#x",
        ] {
            assert!(github_news_url(Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(github_news_url(None).unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_launcher_polls_share_one_background_download() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let fake = Arc::new(Fake {
            answer: Mutex::new((200, "{\"items\":[]}".into(), None)),
            gate: Some(gate.clone()),
            ..Fake::default()
        });
        let url = fake_github(fake.clone()).await;
        let now = Arc::new(AtomicI64::new(0));
        let news = Arc::new(News::new(None, Some(url), clock(&now)));
        let first = news.clone();
        let pending = tokio::spawn(async move { first.refresh_remote().await });
        while fake.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        news.body();
        news.body();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        gate.add_permits(10);
        pending.await.unwrap();
    }
}
