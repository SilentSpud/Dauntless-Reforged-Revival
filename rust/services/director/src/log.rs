//! The Director's log: `service_kit::linelog` lines with the request's id while one is handled, as
//! the Node Director's pino mixin added it (`src/logger.ts`). The console and file copies pino-pretty
//! wrote are left to the host kit, which already keeps each service's output in `logs/`.

use serde_json::{Value, json};
use service_kit::linelog::{self, Level};

tokio::task_local! {
    pub static REQUEST_ID: String;
}

fn fields(error: Option<&str>) -> Value {
    let mut fields = serde_json::Map::new();
    if let Ok(id) = REQUEST_ID.try_with(Clone::clone) {
        fields.insert("requestId".into(), json!(id));
    }
    if let Some(error) = error {
        fields.insert("error".into(), json!(error));
    }
    Value::Object(fields)
}

pub fn info(message: &str) {
    linelog::log(Level::Info, message, fields(None));
}

pub fn warn(message: &str) {
    linelog::log(Level::Warn, message, fields(None));
}

pub fn error(message: &str) {
    linelog::log(Level::Error, message, fields(None));
}

/// A line with the error that caused it, as `logger.warn({ error }, message)`.
pub fn with_error(level: Level, message: &str, error: &str) {
    linelog::log(level, message, fields(Some(error)));
}

pub fn set_level(level: Option<&str>) {
    let level = match level.map(str::to_ascii_lowercase).as_deref() {
        Some("debug" | "trace") => Level::Debug,
        Some("warn") => Level::Warn,
        Some("error" | "fatal") => Level::Error,
        _ => Level::Info,
    };
    linelog::set_level(level);
}

/// The supervisors' and the resolver's log, into this one.
pub struct HuntLog;

impl crate::hunt::HuntLog for HuntLog {
    fn info(&self, message: &str) {
        info(message);
    }

    fn warn(&self, message: &str) {
        warn(message);
    }
}
