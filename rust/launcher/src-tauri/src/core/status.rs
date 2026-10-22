//! Live server status (shared contract 3, `GET /undaunted/api/ServerStatus`), row L12:
//! `parseServerStatus` in `src/shared/status.ts`, answer for answer. The answer comes from the
//! network, so every field is checked and cleaned before the page sees it. Both run
//! `test/cases/status.json`.
#![cfg_attr(
    not(test),
    allow(dead_code, reason = "the host API client (L21) reads ServerStatus with it")
)]

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use protocol::status::{InstanceKind, PlayerWhere, RegistrationMode, ServerStatus, StatusInstance, StatusPlayer};
use serde_json::{Map, Value};

use super::text::{is_js_space, is_unsafe_char, js_length};

const MAX_PLAYERS_LISTED: usize = 500;
const MAX_INSTANCES_LISTED: usize = 100;

/// Control and bidi characters removed, JavaScript white space runs made one space, trimmed, empty
/// refused, and at most `max` code points kept.
pub fn clean_text(value: Option<&Value>, max: usize) -> Option<String> {
    let raw = value?.as_str()?;
    let mut text = String::with_capacity(raw.len());
    let mut in_space = false;
    for c in raw.chars().filter(|c| !is_unsafe_char(*c)) {
        if is_js_space(c) {
            if !in_space {
                text.push(' ');
            }
            in_space = true;
        } else {
            text.push(c);
            in_space = false;
        }
    }
    let text = text.trim_matches(' ');
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(max).collect())
}

/// A finite number, rounded down and held within `min..=max`.
fn clean_int(value: Option<&Value>, min: i64, max: i64) -> Option<i64> {
    let n = value?.as_number()?.as_f64()?;
    if !n.is_finite() {
        return None;
    }
    let n = n.floor();
    Some(if n < min as f64 {
        min
    } else if n > max as f64 {
        max
    } else {
        n as i64
    })
}

fn clamp_u32(n: i64) -> u32 {
    u32::try_from(n).unwrap_or(0)
}

/// An https URL without credentials, as the URL parser writes it back (`new URL(v).toString()`).
pub fn clean_https_url(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    if js_length(text) > 512 {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    // The same limits as the link check in links.ts, so a Source button never does nothing
    (url.as_str().len() <= 1024).then(|| url.to_string())
}

fn digits(text: &str, count: usize) -> Option<(u32, &str)> {
    let head = text.get(..count)?;
    if !head.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &text[count..]))
}

/// `Date.parse` of the ECMAScript date-time string format: `YYYY[-MM[-DD]]` (or a signed six-digit
/// year), then optionally `THH:mm[:ss[.fff]]` and `Z` or `±HH:mm`. A date alone is UTC; a date and time
/// without an offset is local time. Milliseconds since 1970, or None. The other formats a browser's
/// own parser accepts are refused: servers send `toISOString()`. So are years past chrono's ±262143
/// (JavaScript reaches ±275760).
fn parse_iso(text: &str) -> Option<i64> {
    let (year, rest) = match text.as_bytes().first()? {
        sign @ (b'+' | b'-') => {
            let (n, rest) = digits(&text[1..], 6)?;
            let year = i32::try_from(n).ok()?;
            if *sign == b'-' && year == 0 {
                return None;
            }
            (if *sign == b'-' { -year } else { year }, rest)
        }
        _ => {
            let (n, rest) = digits(text, 4)?;
            (i32::try_from(n).ok()?, rest)
        }
    };
    let (month, rest) = match rest.strip_prefix('-') {
        Some(rest) => digits(rest, 2)?,
        None => (1, rest),
    };
    let (day, rest) = match (rest.strip_prefix('-'), text.len() - rest.len() >= 7) {
        (Some(rest), true) => digits(rest, 2)?,
        _ => (1, rest),
    };
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    if rest.is_empty() {
        return Some(date.and_time(NaiveTime::MIN).and_utc().timestamp_millis());
    }

    let rest = rest.strip_prefix('T')?;
    let (hour, rest) = digits(rest, 2)?;
    let (minute, mut rest) = digits(rest.strip_prefix(':')?, 2)?;
    let (mut second, mut millis) = (0, 0);
    if let Some(after) = rest.strip_prefix(':') {
        (second, rest) = digits(after, 2)?;
        if let Some(after) = rest.strip_prefix('.') {
            let count = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if count == 0 {
                return None;
            }
            let mut fraction = after[..count.min(3)].to_owned();
            while fraction.len() < 3 {
                fraction.push('0');
            }
            millis = fraction.parse().ok()?;
            rest = &after[count..];
        }
    }
    // 24:00 is the end of the day, and nothing past it
    let end_of_day = hour == 24 && minute == 0 && second == 0 && millis == 0;
    if !end_of_day && (hour > 23 || minute > 59 || second > 59) {
        return None;
    }
    let time = NaiveTime::from_hms_milli_opt(if end_of_day { 0 } else { hour }, minute, second, millis)?;
    let mut at = NaiveDateTime::new(date, time);
    if end_of_day {
        at += chrono::Duration::days(1);
    }

    let offset_minutes = match rest {
        "Z" => Some(0),
        "" => None,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, after) = digits(&rest[1..], 2)?;
            let (minutes, after) = digits(after.strip_prefix(':')?, 2)?;
            if !after.is_empty() || hours > 23 || minutes > 59 {
                return None;
            }
            Some(sign * i64::from(hours * 60 + minutes))
        }
    };
    match offset_minutes {
        Some(offset) => Some(at.and_utc().timestamp_millis() - offset * 60_000),
        None => Some(Local.from_local_datetime(&at).earliest()?.timestamp_millis()),
    }
}

/// `new Date(ms).toISOString()`.
fn to_iso_string(millis: i64) -> Option<String> {
    let at: DateTime<Utc> = DateTime::from_timestamp_millis(millis)?;
    let year = at.format("%Y").to_string().parse::<i32>().ok()?;
    let rest = at.format("-%m-%dT%H:%M:%S%.3fZ");
    Some(if (0..=9999).contains(&year) {
        format!("{year:04}{rest}")
    } else {
        format!("{}{:06}{rest}", if year < 0 { '-' } else { '+' }, year.unsigned_abs())
    })
}

/// ECMAScript dates reach 8.64e15 ms either side of 1970.
const MAX_DATE_MS: i64 = 8_640_000_000_000_000;

/// `new Date(Date.parse(text)).toISOString()`, or None when the date cannot be read.
pub fn iso_date(text: &str) -> Option<String> {
    let millis = parse_iso(text)?;
    if millis.abs() > MAX_DATE_MS {
        return None;
    }
    to_iso_string(millis)
}

fn clean_iso(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    if js_length(text) > 64 {
        return None;
    }
    iso_date(text)
}

fn player_where(value: Option<&Value>) -> PlayerWhere {
    match value.and_then(Value::as_str) {
        Some("menu") => PlayerWhere::Menu,
        Some("city") => PlayerWhere::City,
        Some("hunt") => PlayerWhere::Hunt,
        Some("dojo") => PlayerWhere::Dojo,
        Some("tutorial") => PlayerWhere::Tutorial,
        _ => PlayerWhere::Unknown,
    }
}

fn instance_kind(value: Option<&Value>) -> Option<InstanceKind> {
    match value?.as_str()? {
        "city" => Some(InstanceKind::City),
        "hunt" => Some(InstanceKind::Hunt),
        "dojo" => Some(InstanceKind::Dojo),
        "tutorial" => Some(InstanceKind::Tutorial),
        _ => None,
    }
}

fn player(raw: &Map<String, Value>) -> Option<StatusPlayer> {
    Some(StatusPlayer {
        name: clean_text(raw.get("name"), 32)?,
        location: player_where(raw.get("where")),
        instance: clean_text(raw.get("instance"), 64),
    })
}

fn instance(raw: &Map<String, Value>) -> Option<StatusInstance> {
    let id = clean_text(raw.get("id"), 64)?;
    let kind = instance_kind(raw.get("kind"))?;
    Some(StatusInstance {
        id,
        kind,
        title: clean_text(raw.get("title"), 64).unwrap_or_default(),
        map: clean_text(raw.get("map"), 96).unwrap_or_default(),
        behemoth: clean_text(raw.get("behemoth"), 64),
        players: clamp_u32(clean_int(raw.get("players"), 0, 1000).unwrap_or(0)),
        max_players: clamp_u32(clean_int(raw.get("maxPlayers"), 0, 1000).unwrap_or(0)),
        started_at: clean_iso(raw.get("startedAt")),
    })
}

fn objects(value: Option<&Value>, max: usize) -> impl Iterator<Item = &Map<String, Value>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(move |list| list.iter().take(max))
        .filter_map(Value::as_object)
}

pub fn parse_server_status(raw: &Value) -> Option<ServerStatus> {
    let raw = raw.as_object()?;
    let name = clean_text(raw.get("name"), 64)?;
    let players: Vec<StatusPlayer> = objects(raw.get("players"), MAX_PLAYERS_LISTED)
        .filter_map(player)
        .collect();
    let instances: Vec<StatusInstance> = objects(raw.get("instances"), MAX_INSTANCES_LISTED)
        .filter_map(instance)
        .collect();
    let registration = match raw.get("registration").and_then(Value::as_str) {
        Some("OPEN") => Some(RegistrationMode::Open),
        Some("INVITECODE") => Some(RegistrationMode::Invitecode),
        Some("NONE") => Some(RegistrationMode::None),
        _ => None,
    };
    let content_port = clean_int(raw.get("contentPort"), 0, 65535)
        .filter(|port| *port >= 1)
        .and_then(|port| u16::try_from(port).ok());
    let players_online = clean_int(raw.get("playersOnline"), 0, 100_000)
        .map_or_else(|| u32::try_from(players.len()).unwrap_or(u32::MAX), clamp_u32);
    let status = ServerStatus {
        name,
        online: raw.get("online") == Some(&Value::Bool(true)),
        version: clean_text(raw.get("version"), 48).unwrap_or_default(),
        commit: clean_text(raw.get("commit"), 64).unwrap_or_default(),
        source_url: clean_https_url(raw.get("sourceUrl")),
        registration,
        players_online,
        players,
        instances,
        content_port,
        uptime_seconds: clamp_u32(clean_int(raw.get("uptimeSeconds"), 0, 100 * 365 * 24 * 3600).unwrap_or(0)),
        // Only an explicit true: an older server without the field lists everyone
        limited: raw.get("limited") == Some(&Value::Bool(true)),
    };
    // A limited answer lists nobody, whatever else came with it
    Some(if status.limited { limited_view(status) } else { status })
}

/// What a limited answer would show: the same server, nobody listed.
pub fn limited_view(status: ServerStatus) -> ServerStatus {
    ServerStatus {
        players_online: 0,
        players: Vec::new(),
        instances: Vec::new(),
        limited: true,
        ..status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Case {
        input: Value,
        expected: Option<ServerStatus>,
    }

    #[test]
    fn the_shared_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/status.json");
        let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(cases.len() > 60);
        for case in cases {
            assert_eq!(parse_server_status(&case.input), case.expected, "{}", case.input);
        }
    }

    #[test]
    fn dates_far_out() {
        assert_eq!(
            clean_iso(Some(&Value::from("+200000-09-13T00:00:00Z"))).as_deref(),
            Some("+200000-09-13T00:00:00.000Z")
        );
        assert_eq!(
            clean_iso(Some(&Value::from("-000001-01-01T00:00:00Z"))).as_deref(),
            Some("-000001-01-01T00:00:00.000Z")
        );
        assert_eq!(clean_iso(Some(&Value::from("-000000-01-01T00:00:00Z"))), None);
        assert_eq!(
            clean_iso(Some(&Value::from("2026-09-21T24:00:00Z"))).as_deref(),
            Some("2026-09-22T00:00:00.000Z")
        );
        assert_eq!(clean_iso(Some(&Value::from("2026-02-30T00:00:00Z"))), None);
    }
}
