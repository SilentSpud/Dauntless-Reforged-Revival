//! The live server status, `GET /reforged/api/ServerStatus`: `parseServerStatus` in
//! `legacy/ReforgedLauncher/src/shared/status.ts`, answer for answer. Dauntless Revived's contract plus the
//! game build, the state of the host's game data and chat. The answer comes from the network, so every
//! field is checked and cleaned before the page sees it.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::text::{collapse_spaces, is_unsafe_char, js_length};

pub const STATUS_PATH: &str = "/reforged/api/ServerStatus";

const MAX_PLAYERS_LISTED: usize = 500;
const MAX_INSTANCES_LISTED: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum PlayerWhere {
    Menu,
    City,
    Hunt,
    Dojo,
    Tutorial,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum InstanceKind {
    City,
    Hunt,
    Dojo,
    Tutorial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "UPPERCASE")]
pub enum RegistrationMode {
    Open,
    Invitecode,
    None,
}

/// Which game data the host's services run on. `Synthetic`: the shipped examples, enough to start the
/// services but not to play; `Missing`: the host has not generated it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum GameDataKind {
    Real,
    Synthetic,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StatusPlayer {
    pub name: String,
    #[serde(rename = "where")]
    pub location: PlayerWhere,
    pub instance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatusInstance {
    pub id: String,
    pub kind: InstanceKind,
    pub title: String,
    pub map: String,
    pub behemoth: Option<String>,
    pub players: u32,
    pub max_players: u32,
    /// ISO string, or null when the server sent something unreadable
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StatusGame {
    /// At most 64 characters
    pub build: String,
    /// 0 to 100,000,000, or null when the server sent none or something unreadable
    pub changelist: Option<u32>,
}

/// In the TypeScript's field order, so the page gets the same JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub name: String,
    pub online: bool,
    pub version: String,
    pub commit: String,
    /// Only https URLs survive parsing
    pub source_url: Option<String>,
    pub registration: Option<RegistrationMode>,
    pub players_online: u32,
    pub players: Vec<StatusPlayer>,
    pub instances: Vec<StatusInstance>,
    pub uptime_seconds: u32,
    /// Null when the server names no build
    pub game: Option<StatusGame>,
    pub game_data: GameDataKind,
    /// True only when the server says so
    pub chat: bool,
    /// The port of a separate content server, which only Dauntless Revived's servers in private mode
    /// named. Kept in the contract; Reforged's launcher does not read it (RD5).
    pub content_port: Option<u16>,
    /// The server hid who is online (playersOnline 0, no players, no instances): its answer to a
    /// launcher without an account key there, or with one it did not accept.
    pub limited: bool,
}

/// Control and bidi characters removed, JavaScript white space runs made one space, trimmed, empty
/// refused, and at most `max` code points kept.
pub fn clean_text(value: Option<&Value>, max: usize) -> Option<String> {
    let raw = value?.as_str()?;
    let text = collapse_spaces(raw.chars().filter(|c| !is_unsafe_char(*c)));
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

fn game_data(value: Option<&Value>) -> GameDataKind {
    match value.and_then(Value::as_str) {
        Some("real") => GameDataKind::Real,
        Some("synthetic") => GameDataKind::Synthetic,
        Some("missing") => GameDataKind::Missing,
        _ => GameDataKind::Unknown,
    }
}

fn game(value: Option<&Value>) -> Option<StatusGame> {
    let raw = value?.as_object()?;
    Some(StatusGame {
        build: clean_text(raw.get("build"), 64)?,
        changelist: clean_int(raw.get("changelist"), 0, 100_000_000).map(clamp_u32),
    })
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

/// A server's answer, checked and cleaned; None when it is not an object with a name.
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
        uptime_seconds: clamp_u32(clean_int(raw.get("uptimeSeconds"), 0, 100 * 365 * 24 * 3600).unwrap_or(0)),
        game: game(raw.get("game")),
        game_data: game_data(raw.get("gameData")),
        chat: raw.get("chat") == Some(&Value::Bool(true)),
        content_port,
        // Only an explicit true: an older server without the field lists everyone
        limited: raw.get("limited") == Some(&Value::Bool(true)),
    };
    // A limited answer lists nobody, whatever else came with it
    Some(if status.limited { limited_view(status) } else { status })
}

/// What a limited answer would show: the same server, nobody listed. Used when the launcher knows it
/// can no longer see the list (it just logged out) before the next poll says so.
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
