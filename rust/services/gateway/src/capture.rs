//! Traffic capture for the shadow replay (MIGRATION_PLAN.md RD19, RB2), from Dauntless Revived's
//! Node `capture.ts`: every request and answer as one redacted JSON line, and the game's chat
//! frames, so the recorded play can be replayed against the Node and the Rust backends and their
//! answers compared before the cutover.
//!
//! Off unless `CAPTURE_DIR` is set. What is written is personal data (usernames, addresses, play
//! history): files are private (0600 where the file system has modes), one per day, and deleted
//! after `CAPTURE_RETENTION_DAYS` (default 14). Nothing secret is ever written:
//! - a login token becomes `<token:UID-...>` (its subject, read without the signature), so the
//!   replay can sign a fresh one for the same account;
//! - an account key (`RRK_...`, or DR's `UUK_...`) becomes `<key:sha256 of the key>`, so the
//!   replay can swap in a key of its own for that account;
//! - the game-server key becomes `<gskey>`; every other secret-looking value becomes `<redacted>`;
//! - the gateway's own secrets are cut from every line, wherever an answer might echo them.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::task::{Context, Poll};

use http::HeaderMap;
use regex::{Captures, Regex};
use serde_json::{Map, Value, json};
use service_kit::env::Env;
use service_kit::js;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::cert::iso;
use crate::feed::Clock;
use crate::policy::GAMESERVER_KEY_HEADER;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOptions {
    pub dir: PathBuf,
    pub retention_days: i64,
    pub max_body_bytes: usize,
    /// The file name prefix and the `src` of every line (`gateway`)
    pub source: String,
}

/// `LoadCaptureOptions`: `None` when `CAPTURE_DIR` is not set.
pub fn load_capture_options(env: &Env, source: &str) -> Result<Option<CaptureOptions>, String> {
    let dir = env.raw("CAPTURE_DIR").unwrap_or("").trim();
    if dir.is_empty() {
        return Ok(None);
    }
    let int = |name: &str, default: i64, min: i64, max: i64| -> Result<i64, String> {
        let raw = env.raw(name).unwrap_or("").trim();
        if raw.is_empty() {
            return Ok(default);
        }
        let value = js::number(Some(&json!(raw)));
        if value.fract() != 0.0 || value < min as f64 || value > max as f64 {
            return Err(format!(
                "{name} must be a whole number from {min} to {max} (got {})",
                json!(raw)
            ));
        }
        Ok(value as i64)
    };
    Ok(Some(CaptureOptions {
        dir: std::path::absolute(dir).unwrap_or_else(|_| PathBuf::from(dir)),
        retention_days: int("CAPTURE_RETENTION_DAYS", 14, 1, 90)?,
        max_body_bytes: int("CAPTURE_MAX_BODY_BYTES", 256 * 1024, 0, 16 * 1024 * 1024)? as usize,
        source: source.to_owned(),
    }))
}

// ---- Redaction ----

#[allow(clippy::unwrap_used)] // constant patterns
fn pattern(text: &str) -> Regex {
    Regex::new(text).unwrap()
}

static ACCOUNT_KEY: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?:RRK|UUK)_[0-9A-Za-z]+"));
static JWT: LazyLock<Regex> = LazyLock::new(|| pattern(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"));
static LONG_HEX: LazyLock<Regex> = LazyLock::new(|| pattern(r"[0-9a-fA-F]{40,}"));
static LONG_TOKEN: LazyLock<Regex> = LazyLock::new(|| pattern(r"[A-Za-z0-9_+/=-]{100,}"));
static HELD: LazyLock<Regex> = LazyLock::new(|| pattern("\u{1}([0-9]+)\u{1}"));
/// JSON fields and form fields whose values are secrets whatever they look like
static SECRET_NAME: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i)(key|token|secret|pass|auth|jwt|sig|cred|recovery|invite|cookie)"));
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| pattern(r"^<(token|key):[^>]+>$"));
static AUTH: LazyLock<Regex> = LazyLock::new(|| pattern(r"(<auth\b[^>]*>)([^<]*)(</auth>)"));
static PASSWORD: LazyLock<Regex> = LazyLock::new(|| pattern(r"(<password\b[^>]*>)[^<]*(</password>)"));
static NAME: LazyLock<Regex> = LazyLock::new(|| pattern(r"^[A-Za-z0-9_-]{1,64}$"));

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `Buffer.from(text, "base64")` (and `"base64url"`): both alphabets, other characters skipped,
/// stops at the first `=`.
pub fn decode_base64_lenient(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out
}

/// The `userId` (or `sub`) of a token, read without checking the signature.
fn token_subject(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let claims: Value = serde_json::from_str(&String::from_utf8_lossy(&decode_base64_lenient(payload))).ok()?;
    let present = |name: &str| claims.get(name).filter(|v| !v.is_null());
    let subject = present("userId").or_else(|| present("sub"))?.as_str()?;
    NAME.is_match(subject).then(|| subject.to_owned())
}

/// `RedactCaptureText`: tokens and keys become placeholders the replay can substitute; other long
/// secrets are cut.
pub fn redact_capture_text(text: &str) -> String {
    // Placeholders go in last, so the generic rules never cut the hash inside <key:...>
    let mut held: Vec<String> = Vec::new();
    let mut hold = |placeholder: String| {
        held.push(placeholder);
        format!("\u{1}{}\u{1}", held.len() - 1)
    };
    let text = text.replace('\u{1}', "?");
    let text = JWT.replace_all(&text, |found: &Captures<'_>| {
        hold(token_subject(&found[0]).map_or_else(|| "<token>".to_owned(), |s| format!("<token:{s}>")))
    });
    let text = ACCOUNT_KEY.replace_all(&text, |found: &Captures<'_>| {
        hold(format!("<key:{}>", sha256_hex(&found[0])))
    });
    let text = LONG_HEX.replace_all(&text, "<redacted>");
    let text = LONG_TOKEN.replace_all(&text, "<redacted>");
    HELD.replace_all(&text, |found: &Captures<'_>| {
        found[1]
            .parse::<usize>()
            .ok()
            .and_then(|index| held.get(index))
            .cloned()
            .unwrap_or_default()
    })
    .into_owned()
}

fn redact_named(name: &str, value: &str) -> String {
    let redacted = redact_capture_text(value);
    if !SECRET_NAME.is_match(name) {
        return redacted;
    }
    // A secret field keeps its placeholder when it is a token or a key (the replay needs those)
    if PLACEHOLDER.is_match(&redacted) {
        redacted
    } else {
        "<redacted>".into()
    }
}

/// `RedactCaptureJson`: strings redacted by the name of the field that holds them.
pub fn redact_capture_json(value: &Value, name: &str) -> Value {
    match value {
        Value::String(text) => Value::String(redact_named(name, text)),
        Value::Array(items) => Value::Array(items.iter().map(|item| redact_capture_json(item, name)).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| (key.clone(), redact_capture_json(item, key)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `RedactCaptureHeaders`: header values kept for the replay, the authorization and the account key
/// as placeholders, the game-server key as `<gskey>`, cookies and the gateway's own header dropped.
/// A header sent more than once is joined with ", " (as Node reads most of them).
pub fn redact_capture_headers(headers: &HeaderMap) -> Value {
    let mut out = Map::new();
    for name in headers.keys() {
        let name = name.as_str();
        if matches!(name, "cookie" | "set-cookie" | "x-reforged-gateway") {
            continue;
        }
        let text = headers
            .get_all(name)
            .iter()
            .map(|v| v.as_bytes().iter().map(|b| char::from(*b)).collect::<String>())
            .collect::<Vec<_>>()
            .join(", ");
        let value = if name == GAMESERVER_KEY_HEADER {
            "<gskey>".to_owned()
        } else if name == "authorization" {
            redact_capture_text(&text)
        } else {
            let short = name
                .strip_prefix("x-reforged-")
                .or_else(|| name.strip_prefix("x-mysticparadox-"))
                .unwrap_or(name);
            redact_named(short, &text)
        };
        out.insert(name.to_owned(), Value::String(value));
    }
    Value::Object(out)
}

/// `RedactCaptureBody`: JSON (redacted by field), a form (redacted by field), text (redacted), or
/// only the size of binary, oversized or cut bodies. `body` is what was kept of `total` bytes.
pub fn redact_capture_body(body: &[u8], content_type: Option<&str>, total: u64, max: usize) -> Value {
    if total == 0 {
        return Value::Null;
    }
    if total > max as u64 || (body.len() as u64) < total {
        return json!({ "omitted": total });
    }
    let kind = content_type.unwrap_or("").to_lowercase();
    let text = String::from_utf8_lossy(body);
    let opens = text.trim_start_matches(js::is_space).starts_with(['[', '{']);
    if (kind.contains("json") || opens)
        && let Ok(parsed) = serde_json::from_str::<Value>(&text)
    {
        return json!({ "json": redact_capture_json(&parsed, "") });
    }
    if kind.contains("x-www-form-urlencoded") {
        let form: Vec<Value> = url::form_urlencoded::parse(text.as_bytes())
            .map(|(name, value)| json!([name, redact_named(&name, &value)]))
            .collect();
        return json!({ "form": form });
    }
    let controls = text
        .chars()
        .any(|c| matches!(c, '\u{0}'..='\u{8}' | '\u{e}'..='\u{1f}'));
    if !controls && std::str::from_utf8(body).is_ok() {
        return json!({ "text": redact_capture_text(&text) });
    }
    json!({ "binary": total })
}

/// `RedactChatFrame`: a SASL PLAIN login (base64 of "\0<account>\0<token>") becomes
/// `<plain:<account>>`, so the replay can log the same account in with a fresh token; a legacy
/// password element is cut; the rest is redacted like any text.
pub fn redact_chat_frame(frame: &str) -> String {
    let logins = AUTH.replace_all(frame, |found: &Captures<'_>| {
        let decoded = String::from_utf8_lossy(&decode_base64_lenient(found[2].trim())).into_owned();
        let account = decoded.split('\0').nth(1).filter(|a| NAME.is_match(a));
        let inner = account.map_or_else(|| "<redacted>".to_owned(), |a| format!("<plain:{a}>"));
        format!("{}{inner}{}", &found[1], &found[3])
    });
    let logins = PASSWORD.replace_all(&logins, "${1}<redacted>${2}");
    redact_capture_text(&logins)
}

/// `RedactCaptureTarget`: the request target with its secrets replaced (paths carry tokens on some
/// routes).
pub fn redact_capture_target(target: &str) -> String {
    redact_capture_text(target)
}

// ---- Chat frames ----

/// Reads the WebSocket frames going one way through the gateway's chat tunnel, without changing
/// them, and hands back each whole text or binary message (as the chat reads them: UTF-8).
/// Control frames are skipped; a message over `max` bytes is reported by its size only, and a
/// compressed one (RSV1) as such.
pub struct FrameReader {
    max: usize,
    head: Vec<u8>,
    /// Payload bytes still to come in the current frame
    left: u64,
    mask: Option<[u8; 4]>,
    at: usize,
    control: bool,
    fin: bool,
    message: Vec<u8>,
    total: u64,
    compressed: bool,
    in_message: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMessage {
    Text(String),
    Omitted(u64),
    Compressed(u64),
}

impl FrameReader {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            head: Vec::with_capacity(14),
            left: 0,
            mask: None,
            at: 0,
            control: false,
            fin: false,
            message: Vec::new(),
            total: 0,
            compressed: false,
            in_message: false,
        }
    }

    /// The header length once enough of it is in, and the payload length.
    fn header(&self) -> Option<(usize, u64)> {
        let second = *self.head.get(1)?;
        let masked = second & 0x80 != 0;
        let (extra, length) = match second & 0x7f {
            126 => (
                2,
                u64::from(u16::from_be_bytes([*self.head.get(2)?, *self.head.get(3)?])),
            ),
            127 => {
                let bytes: [u8; 8] = self.head.get(2..10)?.try_into().ok()?;
                (8, u64::from_be_bytes(bytes))
            }
            short => (0, u64::from(short)),
        };
        let size = 2 + extra + if masked { 4 } else { 0 };
        (self.head.len() >= size).then_some((size, length))
    }

    fn start_frame(&mut self, size: usize, length: u64, out: &mut Vec<ChatMessage>) {
        let first = self.head[0];
        let opcode = first & 0x0f;
        self.fin = first & 0x80 != 0;
        self.control = opcode >= 8;
        self.mask = (self.head[1] & 0x80 != 0).then(|| {
            let key = &self.head[size - 4..size];
            [key[0], key[1], key[2], key[3]]
        });
        self.at = 0;
        self.left = length;
        if !self.control && opcode != 0 {
            // A new message (a data frame where a message was still open is a protocol error:
            // the old one is dropped)
            self.in_message = true;
            self.message.clear();
            self.total = 0;
            self.compressed = first & 0x40 != 0;
        }
        self.head.clear();
        if length == 0 {
            self.end_frame(out);
        }
    }

    fn end_frame(&mut self, out: &mut Vec<ChatMessage>) {
        if self.control || !self.fin || !self.in_message {
            return;
        }
        self.in_message = false;
        let message = std::mem::take(&mut self.message);
        out.push(if self.compressed {
            ChatMessage::Compressed(self.total)
        } else if self.total > self.max as u64 {
            ChatMessage::Omitted(self.total)
        } else {
            ChatMessage::Text(String::from_utf8_lossy(&message).into_owned())
        });
    }

    /// Feeds the next bytes; returns the messages they completed.
    pub fn feed(&mut self, mut data: &[u8]) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        while !data.is_empty() {
            if self.left == 0 {
                self.head.push(data[0]);
                data = &data[1..];
                if let Some((size, length)) = self.header() {
                    self.start_frame(size, length, &mut out);
                }
                continue;
            }
            let take = usize::try_from(self.left).map_or(data.len(), |left| left.min(data.len()));
            let (chunk, rest) = data.split_at(take);
            data = rest;
            self.left -= take as u64;
            if !self.control && self.in_message {
                self.total += take as u64;
                let room = (self.max + 1).saturating_sub(self.message.len()).min(take);
                for (offset, byte) in chunk[..room].iter().enumerate() {
                    let key = self.mask.map_or(0, |mask| mask[(self.at + offset) % 4]);
                    self.message.push(byte ^ key);
                }
            }
            self.at += take;
            if self.left == 0 {
                self.end_frame(&mut out);
            }
        }
        out
    }
}

// ---- Writing ----

struct Open {
    day: String,
    file: Option<File>,
    broken: bool,
}

/// `CaptureWriter`: appends records to `<dir>/<source>-<yyyy-mm-dd>.jsonl` (UTC days) and removes
/// files older than the retention. A write error stops the capture with one warning; the gateway
/// itself never fails for it. Lines are small and written whole, under a lock.
pub struct CaptureWriter {
    pub options: CaptureOptions,
    secrets: Vec<String>,
    warn: Box<dyn Fn(&str) + Send + Sync>,
    now: Clock,
    open: Mutex<Open>,
}

impl CaptureWriter {
    pub fn new(options: CaptureOptions, warn: Box<dyn Fn(&str) + Send + Sync>, now: Clock) -> Result<Self, String> {
        create_private_dir(&options.dir).map_err(|e| format!("{}: {e}", options.dir.display()))?;
        let writer = Self {
            options,
            secrets: Vec::new(),
            warn,
            now,
            open: Mutex::new(Open {
                day: String::new(),
                file: None,
                broken: false,
            }),
        };
        writer.prune();
        Ok(writer)
    }

    /// The gateway's own secrets are cut from every line, wherever an answer might echo them.
    pub fn add_secret(&mut self, secret: Option<&str>) {
        if let Some(secret) = secret.filter(|s| s.len() >= 8) {
            let quoted = Value::String(secret.to_owned()).to_string();
            self.secrets.push(quoted[1..quoted.len() - 1].to_owned());
        }
    }

    fn timestamp(&self) -> String {
        iso(chrono::DateTime::from_timestamp_millis((self.now)()).unwrap_or_default())
    }

    pub fn write(&self, record: Map<String, Value>) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.broken {
            return;
        }
        let t = self.timestamp();
        let day = t[..10].to_owned();
        if day != open.day {
            open.day = day.clone();
            let file = self.options.dir.join(format!("{}-{day}.jsonl", self.options.source));
            match append_private(&file) {
                Ok(handle) => open.file = Some(handle),
                Err(error) => {
                    open.broken = true;
                    (self.warn)(&format!("traffic capture stopped: {error}"));
                    return;
                }
            }
            self.prune();
        }
        let mut line = Map::new();
        line.insert("t".into(), json!(t));
        line.insert("src".into(), json!(self.options.source));
        line.extend(record);
        let mut text = Value::Object(line).to_string();
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "<redacted>");
        }
        text.push('\n');
        let written = open.file.as_mut().map(|file| file.write_all(text.as_bytes()));
        if let Some(Err(error)) = written {
            open.broken = true;
            (self.warn)(&format!("traffic capture stopped: {error}"));
        }
    }

    pub fn prune(&self) {
        let cutoff = (self.now)() - self.options.retention_days * 86_400_000;
        let oldest = iso(chrono::DateTime::from_timestamp_millis(cutoff).unwrap_or_default());
        let oldest = &oldest[..10];
        let Ok(entries) = std::fs::read_dir(&self.options.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(day) = capture_file_day(name)
                && day < oldest
            {
                // Next time, if it fails now
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// The day of a capture file, `^[a-z]+-(\d{4}-\d{2}-\d{2})\.jsonl$`.
fn capture_file_day(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".jsonl")?;
    let (source, day) = stem.split_at_checked(stem.len().checked_sub(10)?)?;
    let source = source.strip_suffix('-')?;
    let shape = day.bytes().enumerate().all(|(i, b)| {
        if i == 4 || i == 7 {
            b == b'-'
        } else {
            b.is_ascii_digit()
        }
    });
    (!source.is_empty() && source.bytes().all(|b| b.is_ascii_lowercase()) && shape).then_some(day)
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

fn append_private(file: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)
}

/// `BodyTap`: keeps up to `max` bytes of a body's chunks, counting the rest.
#[derive(Debug, Default)]
pub struct BodyTap {
    max: usize,
    kept: Vec<u8>,
    pub total: u64,
}

impl BodyTap {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            kept: Vec::new(),
            total: 0,
        }
    }

    pub fn add(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        let room = self.max.saturating_sub(self.kept.len()).min(chunk.len());
        self.kept.extend_from_slice(&chunk[..room]);
    }

    pub fn body(&self) -> &[u8] {
        &self.kept
    }
}

pub type SharedTap = Arc<Mutex<BodyTap>>;

// ---- In the gateway ----

/// The gateway's capture: the writer and the numbers that tie records together (`conn` per
/// connection, in the order they first have something recorded; `seq` per record).
pub struct Capture {
    writer: CaptureWriter,
    next_connection: AtomicU64,
    next_request: AtomicU64,
}

/// A connection's number, given the first time it is needed.
pub type ConnectionId = Arc<OnceLock<u64>>;

impl Capture {
    pub fn new(writer: CaptureWriter) -> Self {
        Self {
            writer,
            next_connection: AtomicU64::new(1),
            next_request: AtomicU64::new(1),
        }
    }

    pub fn max_body_bytes(&self) -> usize {
        self.writer.options.max_body_bytes
    }

    pub fn connection(&self, id: &ConnectionId) -> u64 {
        *id.get_or_init(|| self.next_connection.fetch_add(1, Ordering::Relaxed))
    }

    pub fn next_seq(&self) -> u64 {
        self.next_request.fetch_add(1, Ordering::Relaxed)
    }

    pub fn write(&self, record: Value) {
        if let Value::Object(fields) = record {
            self.writer.write(fields);
        }
    }
}

/// Which way a chat frame went: `in` from the game, `out` from the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

/// One side of the chat tunnel, read through a [`FrameReader`]: every whole message read from it
/// is recorded. The bytes themselves pass unchanged.
pub struct ChatTap<S> {
    inner: S,
    reader: FrameReader,
    capture: Arc<Capture>,
    conn: u64,
    direction: Direction,
}

impl<S> ChatTap<S> {
    pub fn new(inner: S, capture: Arc<Capture>, conn: u64, direction: Direction) -> Self {
        Self {
            inner,
            reader: FrameReader::new(capture.max_body_bytes()),
            capture,
            conn,
            direction,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for ChatTap<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let this = &mut *self;
            let dir = match this.direction {
                Direction::In => "in",
                Direction::Out => "out",
            };
            for message in this.reader.feed(&buf.filled()[before..]) {
                let record = match message {
                    ChatMessage::Text(text) => {
                        json!({ "kind": "chat", "conn": this.conn, "dir": dir, "text": redact_chat_frame(&text) })
                    }
                    ChatMessage::Omitted(size) => {
                        json!({ "kind": "chat", "conn": this.conn, "dir": dir, "omitted": size })
                    }
                    ChatMessage::Compressed(size) => {
                        json!({ "kind": "chat", "conn": this.conn, "dir": dir, "compressed": size })
                    }
                };
                this.capture.write(record);
            }
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ChatTap<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: Value) -> String {
        let part = |value: &Value| {
            const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let bytes = value.to_string().into_bytes();
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk
                    .iter()
                    .enumerate()
                    .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
                for i in 0..=chunk.len() {
                    out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
                }
            }
            out
        };
        format!(
            "{}.{}.{}",
            part(&json!({ "alg": "RS256", "typ": "JWT" })),
            part(&claims),
            "s".repeat(43)
        )
    }

    fn token() -> String {
        jwt(json!({ "userId": "UID-capture-a", "epoch": 0 }))
    }

    const KEY: &str = "RRK_0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f";
    const GS_KEY: &str = "ffffffffffffffffffffffffffffffffffffffffffffffff";

    fn key_hash() -> String {
        sha256_hex(KEY)
    }

    #[test]
    fn turns_a_token_into_its_subject_and_a_key_into_its_hash() {
        assert_eq!(
            redact_capture_text(&format!("bearer {}", token())),
            "bearer <token:UID-capture-a>"
        );
        assert_eq!(
            redact_capture_text(&format!("key={KEY}&x=1")),
            format!("key=<key:{}>&x=1", key_hash())
        );
        let legacy = "UUK_0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f";
        assert_eq!(redact_capture_text(legacy), format!("<key:{}>", sha256_hex(legacy)));
        assert_eq!(redact_capture_text(&jwt(json!({ "nobody": true }))), "<token>");
        assert_eq!(redact_capture_text(&jwt(json!({ "sub": "abc" }))), "<token:abc>");
        assert_eq!(redact_capture_text(&jwt(json!({ "userId": "a b" }))), "<token>");
        assert_eq!(
            redact_capture_text(&format!("secret {} here", "ab".repeat(30))),
            "secret <redacted> here"
        );
        assert_eq!(
            redact_capture_text("\u{1}1\u{1}"),
            "?1?",
            "a forged placeholder marker is defused"
        );
    }

    #[test]
    fn keeps_headers_for_the_replay_without_their_secrets() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("authorization", format!("bearer {}", token())),
            ("x-reforged-user-api-key", KEY.to_owned()),
            (GAMESERVER_KEY_HEADER, GS_KEY.to_owned()),
            ("x-reforged-admin-key", "admin-key-0123".to_owned()),
            ("x-mysticparadox-metrics-token", "m".repeat(20)),
            ("cookie", "a=b".to_owned()),
            ("x-reforged-gateway", "secret".to_owned()),
            ("user-agent", "Dauntless/1.14.7".to_owned()),
            ("content-type", "application/json".to_owned()),
        ] {
            headers.append(name, value.parse().unwrap());
        }
        assert_eq!(
            redact_capture_headers(&headers),
            json!({
                "authorization": "bearer <token:UID-capture-a>",
                "x-reforged-user-api-key": format!("<key:{}>", key_hash()),
                GAMESERVER_KEY_HEADER: "<gskey>",
                "x-reforged-admin-key": "<redacted>",
                "x-mysticparadox-metrics-token": "<redacted>",
                "user-agent": "Dauntless/1.14.7",
                "content-type": "application/json",
            })
        );
    }

    #[test]
    fn redacts_secret_named_fields_in_json_and_forms() {
        let body = json!({
            "Username": "Alex",
            "InviteCode": "ABCD-EFGH",
            "Password": "hunter2",
            "UUK": KEY,
            "nested": [{ "access_token": token(), "note": "x" }],
        });
        assert_eq!(
            redact_capture_json(&body, ""),
            json!({
                "Username": "Alex",
                "InviteCode": "<redacted>",
                "Password": "<redacted>",
                "UUK": format!("<key:{}>", key_hash()),
                "nested": [{ "access_token": "<token:UID-capture-a>", "note": "x" }],
            })
        );
        let form = format!("grant_type=password&username=Alex&password={KEY}");
        assert_eq!(
            redact_capture_body(
                form.as_bytes(),
                Some("application/x-www-form-urlencoded"),
                form.len() as u64,
                1024
            ),
            json!({ "form": [["grant_type", "password"], ["username", "Alex"], ["password", format!("<key:{}>", key_hash())]] })
        );
    }

    #[test]
    fn keeps_text_and_only_the_size_of_binary_oversized_or_cut_bodies() {
        let text = format!("hello {}", token());
        assert_eq!(
            redact_capture_body(text.as_bytes(), Some("text/plain"), text.len() as u64, 1024),
            json!({ "text": "hello <token:UID-capture-a>" })
        );
        assert_eq!(
            redact_capture_body(&[0, 1, 2, 3], Some("application/octet-stream"), 4, 1024),
            json!({ "binary": 4 })
        );
        assert_eq!(
            redact_capture_body(b"abc", Some("text/plain"), 5000, 1024),
            json!({ "omitted": 5000 })
        );
        assert_eq!(redact_capture_body(b"", None, 0, 1024), Value::Null);
        // JSON without a JSON content type, and a JSON type with a body that is not JSON
        assert_eq!(
            redact_capture_body(b" {\"a\":1}", None, 8, 1024),
            json!({ "json": { "a": 1 } })
        );
        assert_eq!(
            redact_capture_body(b"20000", Some("application/json"), 5, 1024),
            json!({ "json": 20000 })
        );
        assert_eq!(
            redact_capture_body(b"{nope", Some("application/json"), 5, 1024),
            json!({ "text": "{nope" })
        );
        assert_eq!(
            redact_capture_body(&[0xff, 0x41], None, 2, 1024),
            json!({ "binary": 2 })
        );
    }

    #[test]
    fn chat_logins_keep_the_account_and_lose_the_token() {
        let plain = |text: &str| {
            let bytes = text.as_bytes();
            const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk
                    .iter()
                    .enumerate()
                    .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
                for i in 0..4 {
                    out.push(if i <= chunk.len() {
                        char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize])
                    } else {
                        '='
                    });
                }
            }
            out
        };
        let account = "UID-chat-a";
        let auth = format!(
            "<auth mechanism=\"PLAIN\">{}</auth>",
            plain(&format!("\0{account}\0{}", token()))
        );
        assert_eq!(
            redact_chat_frame(&auth),
            format!("<auth mechanism=\"PLAIN\"><plain:{account}></auth>")
        );
        assert_eq!(
            redact_chat_frame("<auth mechanism=\"PLAIN\">!!!</auth>"),
            "<auth mechanism=\"PLAIN\"><redacted></auth>"
        );
        assert_eq!(
            redact_chat_frame("<query><password>hunter2</password></query>"),
            "<query><password><redacted></password></query>"
        );
        assert_eq!(
            redact_chat_frame(&format!("<message>{}</message>", token())),
            "<message><token:UID-capture-a></message>"
        );
    }

    #[test]
    fn is_off_without_capture_dir_and_refuses_a_bad_retention() {
        assert_eq!(load_capture_options(&Env::from_pairs([]), "gateway"), Ok(None));
        let on = load_capture_options(&Env::from_pairs([("CAPTURE_DIR", "x")]), "gateway")
            .unwrap()
            .unwrap();
        assert_eq!((on.retention_days, on.max_body_bytes), (14, 256 * 1024));
        assert_eq!(on.source, "gateway");
        assert!(on.dir.is_absolute());
        let bad = load_capture_options(
            &Env::from_pairs([("CAPTURE_DIR", "x"), ("CAPTURE_RETENTION_DAYS", "0")]),
            "gateway",
        );
        assert!(bad.unwrap_err().contains("CAPTURE_RETENTION_DAYS"));
    }

    #[test]
    fn writes_one_private_file_per_day_and_removes_the_ones_past_the_retention() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(std::sync::atomic::AtomicI64::new(
            js::date_parse("2026-10-20T23:59:59Z").unwrap(),
        ));
        std::fs::write(dir.path().join("gateway-2026-10-01.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "kept").unwrap();
        let clock = now.clone();
        let options = CaptureOptions {
            dir: dir.path().to_path_buf(),
            retention_days: 14,
            max_body_bytes: 1024,
            source: "gateway".into(),
        };
        let mut writer = CaptureWriter::new(
            options,
            Box::new(|_| {}),
            Arc::new(move || clock.load(std::sync::atomic::Ordering::Relaxed)),
        )
        .unwrap();
        writer.add_secret(Some("the-gateway-secret-\"quoted\""));
        writer.add_secret(Some("short"));
        let record = |value: Value| value.as_object().unwrap().clone();
        writer.write(record(json!({ "kind": "http", "seq": 1 })));
        now.fetch_add(2000, std::sync::atomic::Ordering::Relaxed);
        writer.write(record(
            json!({ "kind": "http", "seq": 2, "echo": "the-gateway-secret-\"quoted\"", "s": "short" }),
        ));
        let mut files: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(
            files,
            ["gateway-2026-10-20.jsonl", "gateway-2026-10-21.jsonl", "notes.txt"]
        );
        let text = std::fs::read_to_string(dir.path().join("gateway-2026-10-21.jsonl")).unwrap();
        let line: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            line,
            json!({ "t": "2026-10-21T00:00:01.000Z", "src": "gateway", "kind": "http", "seq": 2, "echo": "<redacted>", "s": "short" })
        );
        assert!(text.starts_with(r#"{"t":"#), "t and src come first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("gateway-2026-10-21.jsonl"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(capture_file_day("gateway-2026-10-01.jsonl"), Some("2026-10-01"));
        for odd in [
            "gateway-2026-10-1.jsonl",
            "-2026-10-01.jsonl",
            "Gate-2026-10-01.jsonl",
            "x.jsonl",
            ".jsonl",
        ] {
            assert_eq!(capture_file_day(odd), None, "{odd}");
        }
    }

    #[test]
    fn a_write_error_stops_the_capture_with_one_warning() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the day's file should be
        let day = iso(chrono::Utc::now());
        std::fs::create_dir(dir.path().join(format!("gateway-{}.jsonl", &day[..10]))).unwrap();
        let warnings = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = warnings.clone();
        let options = CaptureOptions {
            dir: dir.path().to_path_buf(),
            retention_days: 14,
            max_body_bytes: 1024,
            source: "gateway".into(),
        };
        let writer = CaptureWriter::new(
            options,
            Box::new(move |m| sink.lock().unwrap().push(m.to_owned())),
            Arc::new(crate::now_ms),
        )
        .unwrap();
        writer.write(Map::new());
        writer.write(Map::new());
        let warnings = warnings.lock().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("traffic capture stopped"));
    }

    fn frame(opcode: u8, fin: bool, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let bit = if mask.is_some() { 0x80 } else { 0 };
        match payload.len() {
            n if n < 126 => out.push(bit | n as u8),
            n if n < 65536 => {
                out.push(bit | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(bit | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        if let Some(key) = mask {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        } else {
            out.extend_from_slice(payload);
        }
        out
    }

    #[test]
    fn reads_chat_messages_out_of_websocket_frames() {
        let key = Some([1, 2, 3, 4]);
        let mut stream = frame(1, true, b"<presence/>", key);
        stream.extend(frame(9, true, b"ping", key));
        stream.extend(frame(1, false, b"<message>he", key));
        stream.extend(frame(10, true, b"", key));
        stream.extend(frame(0, true, b"llo</message>", key));
        stream.extend(frame(2, true, b"", None));
        let long = "x".repeat(70_000);
        stream.extend(frame(1, true, long.as_bytes(), None));
        stream.extend(frame(1, true, &[b'y'; 300], None));
        let mut compressed = frame(1, true, b"zz", None);
        compressed[0] |= 0x40;
        stream.extend(compressed);
        stream.extend(frame(8, true, &[3, 232], key));

        let expected = vec![
            ChatMessage::Text("<presence/>".into()),
            ChatMessage::Text("<message>hello</message>".into()),
            ChatMessage::Text(String::new()),
            ChatMessage::Text(long.clone()),
            ChatMessage::Text("y".repeat(300)),
            ChatMessage::Compressed(2),
        ];
        // All at once, and a byte at a time
        assert_eq!(FrameReader::new(100_000).feed(&stream), expected);
        let mut reader = FrameReader::new(100_000);
        let mut bytewise = Vec::new();
        for byte in &stream {
            bytewise.extend(reader.feed(std::slice::from_ref(byte)));
        }
        assert_eq!(bytewise, expected);
        // Over the cap: the size only
        let capped = FrameReader::new(256).feed(&frame(1, true, &[b'y'; 300], key));
        assert_eq!(capped, [ChatMessage::Omitted(300)]);
    }
}
