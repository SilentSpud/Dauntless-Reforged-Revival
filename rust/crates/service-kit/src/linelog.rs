//! The log of the content server and the gateway (their `src/log.ts`): one JSON object per line,
//! `{t, level, msg, ...fields}`, on stdout (stderr for warnings and errors), so the host can grep or
//! load it. Not pino's format: those two services never used pino, and the host's tools read these
//! lines as they are. Nothing here ever receives a key, token or secret: callers pass account ids
//! and redacted paths, never headers.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 10,
    Info = 20,
    Warn = 30,
    Error = 40,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

type Sink = Box<dyn Fn(&str, Level) + Send + Sync>;

struct Logger {
    threshold: Level,
    sink: Option<Sink>,
}

fn logger() -> &'static Mutex<Logger> {
    static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();
    LOGGER.get_or_init(|| {
        let threshold = match std::env::var("LOG_LEVEL").unwrap_or_default().to_lowercase().as_str() {
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => Level::Info,
        };
        Mutex::new(Logger { threshold, sink: None })
    })
}

/// Tests capture the log instead of printing it.
pub fn set_sink(sink: Option<Sink>) {
    logger().lock().unwrap_or_else(std::sync::PoisonError::into_inner).sink = sink;
}

pub fn set_level(level: Level) {
    logger()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .threshold = level;
}

/// `Log`: `fields` must be a JSON object (anything else is left out).
pub fn log(level: Level, msg: &str, fields: Value) {
    let guard = logger().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if level < guard.threshold {
        return;
    }
    let mut line = Map::new();
    line.insert(
        "t".into(),
        json!(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
    );
    line.insert("level".into(), json!(level.as_str()));
    line.insert("msg".into(), json!(msg));
    if let Value::Object(fields) = fields {
        for (name, value) in fields {
            line.insert(name, value);
        }
    }
    let text = crate::jsjson::stringify(&Value::Object(line));
    if let Some(sink) = &guard.sink {
        sink(&text, level);
        return;
    }
    drop(guard);
    if level >= Level::Warn {
        let _ = writeln!(std::io::stderr(), "{text}");
    } else {
        let _ = writeln!(std::io::stdout(), "{text}");
    }
}

pub fn info(msg: &str, fields: Value) {
    log(Level::Info, msg, fields);
}

pub fn warn(msg: &str, fields: Value) {
    log(Level::Warn, msg, fields);
}

pub fn error(msg: &str, fields: Value) {
    log(Level::Error, msg, fields);
}

pub fn debug(msg: &str, fields: Value) {
    log(Level::Debug, msg, fields);
}

/// One line per key at most every `interval_ms`, so a flood (a scanner, a helper that is down)
/// cannot fill the disk (`LogThrottle`).
pub struct Throttle {
    interval_ms: i64,
    max_keys: usize,
    last: Mutex<std::collections::HashMap<String, (i64, u64)>>,
}

impl Throttle {
    pub fn new(interval_ms: i64) -> Self {
        Self {
            interval_ms,
            max_keys: 10_000,
            last: Mutex::default(),
        }
    }

    /// `Some(lines held back since the last one let through)`, or `None` to stay quiet.
    pub fn allow(&self, key: &str, now: i64) -> Option<u64> {
        let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, suppressed)) = last.get_mut(key)
            && now - *at < self.interval_ms
        {
            *suppressed += 1;
            return None;
        }
        let suppressed = last.get(key).map_or(0, |(_, s)| *s);
        if last.len() >= self.max_keys {
            last.clear();
        }
        last.insert(key.to_owned(), (now, 0));
        Some(suppressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_throttle_counts_what_it_held_back() {
        let throttle = Throttle::new(1000);
        assert_eq!(throttle.allow("a", 0), Some(0));
        assert_eq!(throttle.allow("a", 10), None);
        assert_eq!(throttle.allow("a", 20), None);
        assert_eq!(throttle.allow("b", 20), Some(0));
        assert_eq!(throttle.allow("a", 1000), Some(2));
    }
}
