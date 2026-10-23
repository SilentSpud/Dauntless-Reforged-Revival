//! A small logger that can never write an account key (row L29): `src/main/log.ts`. Every line goes
//! through `redact`, which removes registered secrets and anything shaped like a key, a
//! `-AUTH_PASSWORD=` argument or a key header. Lines go to `userData/logs/launcher.log`, which moves to
//! `launcher.old.log` past 2 MiB. Events from the libraries (`tracing`) and panics take the same way.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};

use regex::Regex;

use protocol::account::ACCOUNT_KEY_PREFIX;
use protocol::text::js_length;

/// JavaScript's `\s` (ECMAScript white space and line terminators), for the patterns below.
const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SINK: Mutex<Option<PathBuf>> = Mutex::new(None);

static ACCOUNT_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("RRK_[A-Za-z0-9_-]+").unwrap_or_else(|_| unreachable_regex()));
static AUTH_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"(?i)(-AUTH_PASSWORD=)[^{JS_SPACE}"']*"#)).unwrap_or_else(|_| unreachable_regex())
});
static KEY_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(x-reforged-user-api-key["']?[{JS_SPACE}]*[:=][{JS_SPACE}]*["']?)[^{JS_SPACE}"',}}]+"#
    ))
    .unwrap_or_else(|_| unreachable_regex())
});

/// The patterns above are constants; a pattern that matches nothing stands in if one ever failed.
fn unreachable_regex() -> Regex {
    Regex::new(r"[^\s\S]").unwrap_or_else(|_| std::process::abort())
}

fn secrets() -> MutexGuard<'static, Vec<String>> {
    SECRETS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn remember(value: &str) {
    let mut secrets = secrets();
    if !secrets.iter().any(|known| known == value) {
        secrets.push(value.to_owned());
    }
}

/// Registers a value that must never reach the log; a server-made key without its `RRK_` too.
pub fn add_secret(value: &str) {
    if js_length(value) >= 6 {
        remember(value);
    }
    if let Some(rest) = value.strip_prefix(ACCOUNT_KEY_PREFIX)
        && js_length(rest) >= 16
        && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    {
        remember(rest);
    }
}

pub fn remove_secret(value: &str) {
    let mut secrets = secrets();
    secrets.retain(|known| known != value && value.strip_prefix(ACCOUNT_KEY_PREFIX) != Some(known.as_str()));
}

pub fn redact(text: &str) -> String {
    let mut out = text.to_owned();
    for secret in secrets().iter() {
        out = out.replace(secret.as_str(), "<hidden>");
    }
    let out = ACCOUNT_KEY.replace_all(&out, "RRK_<hidden>");
    let out = AUTH_ARGUMENT.replace_all(&out, "${1}<hidden>");
    KEY_HEADER.replace_all(&out, "${1}<hidden>").into_owned()
}

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// Writes from now on to `<dir>/launcher.log`.
pub fn log_to_file(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    *SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(dir);
}

fn write(level: &str, message: &str) {
    let line = redact(&format!(
        "{} {level} {message}",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ")
    ));
    let sink = SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(dir) = sink.as_ref() else {
        eprintln!("{line}");
        return;
    };
    // Logging must never break the launcher
    let file = dir.join("launcher.log");
    if std::fs::metadata(&file).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&file, dir.join("launcher.old.log"));
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(out, "{line}");
    }
}

pub fn info(message: &str) {
    write("INFO", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}

pub fn error(message: &str) {
    write("ERROR", message);
}

/// `describeError`: an error as one masked line.
pub fn describe_error(error: &dyn std::error::Error) -> String {
    redact(&error.to_string())
}

/// Library events (`tracing`) at INFO and above, into the same masked log.
struct Layer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Layer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                } else {
                    self.0.push_str(&format!(" {}={value:?}", field.name()));
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let level = *event.metadata().level();
        let text = format!("{}: {}", event.metadata().target(), message.0);
        match level {
            tracing::Level::ERROR => error(&text),
            tracing::Level::WARN => warn(&text),
            tracing::Level::INFO => info(&text),
            _ => {}
        }
    }
}

/// Sends library events and panics to the log. Once, at start.
pub fn install() {
    use tracing_subscriber::layer::SubscriberExt as _;
    let subscriber = tracing_subscriber::registry().with(Layer);
    let _ = tracing::subscriber::set_global_default(subscriber);
    std::panic::set_hook(Box::new(|panic| error(&format!("panic: {panic}"))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Cases {
        secrets: Vec<String>,
        removed: String,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        line: String,
        redacted: String,
    }

    #[test]
    fn the_shared_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/log.json");
        let cases: Cases = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for secret in &cases.secrets {
            add_secret(secret);
        }
        let (last, before) = cases.cases.split_last().unwrap();
        for case in before {
            assert_eq!(redact(&case.line), case.redacted, "{:?}", case.line);
        }
        remove_secret(&cases.removed);
        assert_eq!(redact(&last.line), last.redacted);
    }

    #[test]
    fn a_registered_key_never_reaches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        log_to_file(dir.path().to_path_buf());
        let key = concat!("RRK_", "fedcba9876543210", "fedcba9876543210", "fedcba9876543210");
        add_secret(key);
        info(&format!("launching with -AUTH_PASSWORD={key} and {}", &key[4..]));
        tracing::subscriber::with_default(
            tracing_subscriber::layer::SubscriberExt::with(tracing_subscriber::registry(), Layer),
            || {
                tracing::warn!("library saw {}", &key[4..]);
            },
        );
        let text = std::fs::read_to_string(dir.path().join("launcher.log")).unwrap();
        assert!(!text.contains(&key[4..]), "{text}");
        assert!(text.contains(" INFO launching with -AUTH_PASSWORD=<hidden>"), "{text}");
        assert!(text.contains(" WARN "), "{text}");
        *SINK.lock().unwrap() = None;
    }
}
