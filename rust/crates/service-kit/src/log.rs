//! Logs in pino's JSON format (MIGRATION_PLAN.md M24): one object per line with pino's numeric
//! `level`, `time` in milliseconds, `pid`, `hostname`, the event's fields, then `msg`. The Discord
//! bot, the owner dashboard and the operators' log searches read these lines, so the field names stay
//! pino's. An `err` field recorded as an error becomes `{"type", "message"}` like pino's error
//! serializer. tracing has no fatal level: an `error!` with `fatal = true` is logged at pino's 60
//! (see [`fatal!`](crate::fatal)).
//!
//! With `pretty`, lines are human-readable instead, as `pino-pretty` gives the Node services
//! outside production.

use std::fmt;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// Logs an error at pino's `fatal` level.
#[macro_export]
macro_rules! fatal {
    ($($arg:tt)+) => { ::tracing::error!(fatal = true, $($arg)+) };
}

/// `LOG_LEVEL` as pino reads it: trace, debug, info (the default), warn, error, fatal or silent.
pub fn parse_level(text: Option<&str>) -> Result<LevelFilter, String> {
    match text.map(str::trim).unwrap_or("info") {
        "" | "info" => Ok(LevelFilter::INFO),
        "trace" => Ok(LevelFilter::TRACE),
        "debug" => Ok(LevelFilter::DEBUG),
        "warn" => Ok(LevelFilter::WARN),
        "error" | "fatal" => Ok(LevelFilter::ERROR),
        "silent" => Ok(LevelFilter::OFF),
        other => Err(format!(
            "LOG_LEVEL {other:?} is not one of trace, debug, info, warn, error, fatal, silent"
        )),
    }
}

/// Installs the global logger; call once, at start.
pub fn init(level: LevelFilter, pretty: bool) {
    let registry = tracing_subscriber::registry().with(level);
    let installed = if pretty {
        registry
            .with(tracing_subscriber::fmt::layer().compact().with_target(false))
            .try_init()
    } else {
        registry.with(PinoLayer::new(std::io::stdout)).try_init()
    };
    // A second init (tests) keeps the first logger.
    drop(installed);
}

fn pino_level(level: &Level, fatal: bool) -> u8 {
    match *level {
        Level::TRACE => 10,
        Level::DEBUG => 20,
        Level::INFO => 30,
        Level::WARN => 40,
        Level::ERROR if fatal => 60,
        Level::ERROR => 50,
    }
}

/// Writes each event as one pino JSON line to the writer `make` returns.
pub struct PinoLayer<W> {
    make: W,
    pid: u32,
    hostname: String,
}

impl<W> PinoLayer<W> {
    pub fn new(make: W) -> Self {
        Self {
            make,
            pid: std::process::id(),
            hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        }
    }
}

#[derive(Default)]
struct Fields {
    fields: Map<String, Value>,
    msg: Option<String>,
    fatal: bool,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, Value::String(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::String(value.to_owned()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "fatal" {
            self.fatal = value;
        } else {
            self.insert(field, value.into());
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let mut err = Map::new();
        err.insert("type".into(), "Error".into());
        err.insert("message".into(), value.to_string().into());
        self.insert(field, Value::Object(err));
    }
}

impl Fields {
    fn insert(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.msg = Some(match value {
                Value::String(text) => text,
                other => other.to_string(),
            });
        } else {
            self.fields.insert(field.name().to_owned(), value);
        }
    }
}

impl<S, W, O> Layer<S> for PinoLayer<W>
where
    S: Subscriber,
    W: Fn() -> O + 'static,
    O: Write,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut line = Map::new();
        line.insert(
            "level".into(),
            pino_level(event.metadata().level(), fields.fatal).into(),
        );
        line.insert("time".into(), time.into());
        line.insert("pid".into(), self.pid.into());
        line.insert("hostname".into(), self.hostname.clone().into());
        for (key, value) in fields.fields {
            line.insert(key, value);
        }
        if let Some(msg) = fields.msg {
            line.insert("msg".into(), msg.into());
        }
        let mut text = Value::Object(line).to_string();
        text.push('\n');
        // Nothing sensible to do when the log can't be written.
        let _ = (self.make)().write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::ErrorKind::Other)?
                .extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn lines(log: impl FnOnce()) -> Vec<Value> {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::registry().with(PinoLayer::new(move || writer.clone()));
        tracing::subscriber::with_default(subscriber, log);
        let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[test]
    fn lines_have_pinos_fields_in_pinos_order() {
        let lines = lines(|| {
            tracing::info!("Clear Skies, Slayer.");
            tracing::warn!(userId = "UID-1", "Unhandled progression request");
            crate::fatal!("Could not listen on 127.0.0.1:61000");
        });
        let keys: Vec<&str> = lines[1].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["level", "time", "pid", "hostname", "userId", "msg"]);
        assert_eq!(lines[0]["level"], 30);
        assert_eq!(lines[0]["msg"], "Clear Skies, Slayer.");
        assert_eq!(lines[1]["level"], 40);
        assert_eq!(lines[2]["level"], 60);
        assert!(lines[2].get("fatal").is_none());
        assert!(lines[0]["time"].as_u64().unwrap() > 1_700_000_000_000);
    }

    #[test]
    fn errors_are_serialized_like_pinos() {
        let error = std::io::Error::other("port in use");
        let lines = lines(|| tracing::warn!(err = &error as &dyn std::error::Error, "Could not bind"));
        assert_eq!(
            lines[0]["err"],
            serde_json::json!({"type": "Error", "message": "port in use"})
        );
    }

    #[test]
    fn log_levels_are_pinos() {
        assert_eq!(parse_level(None), Ok(LevelFilter::INFO));
        assert_eq!(parse_level(Some("silent")), Ok(LevelFilter::OFF));
        assert_eq!(parse_level(Some("fatal")), Ok(LevelFilter::ERROR));
        assert!(parse_level(Some("verbose")).is_err());
    }
}
