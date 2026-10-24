//! Everything the admin API reads from outside, checked here: the bot's request (headers, body and
//! the arguments it names) and the Director's instance list. The fuzz target
//! (`rust/fuzz/fuzz_targets/admin_request.rs`; docs/security/admin-api.md) drives these functions.

use hyper::HeaderMap;
use serde_json::Value;

use crate::hmac::same_secret;
use crate::upstream::Operator;

/// The biggest request body read: a query and its variables, never more.
pub const MAX_BODY: usize = 16 * 1024;
pub const OPERATOR_HEADER: &str = "x-reforged-operator-id";
/// What reaches a ban's reason, which the player reads in the launcher.
pub const MAX_REASON: usize = 400;
/// A timeout ends within a year; longer is a ban.
pub const MAX_TIMEOUT_MS: i64 = 365 * 24 * 60 * 60 * 1000;

/// Headers that mean the request came through a proxy (the gateway among them), which nothing on
/// this machine has any reason to put in front of the admin API.
const FORWARDED: [&str; 5] = ["forwarded", "via", "x-real-ip", "x-reforged-gateway", "x-forwarded-for"];

/// Why a request is refused before any GraphQL is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// 403: through a proxy
    Forwarded,
    /// 401: no key or the wrong one
    Unauthenticated,
    /// 400: an operator id that is not 1-32 digits
    BadOperator,
    /// 400: a body that is not one GraphQL request
    BadBody,
    /// 415: not `application/json`
    NotJson,
}

impl Refusal {
    pub fn status(self) -> u16 {
        match self {
            Self::Forwarded => 403,
            Self::Unauthenticated => 401,
            Self::BadOperator | Self::BadBody => 400,
            Self::NotJson => 415,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Forwarded => "FORWARDED",
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::BadOperator => "BAD_OPERATOR",
            Self::BadBody => "BAD_REQUEST",
            Self::NotJson => "NOT_JSON",
        }
    }
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    // A repeated header is refused, so a proxy and a client never disagree about which one counts
    if values.next().is_some() {
        return Err(());
    }
    first.to_str().map(Some).map_err(|_| ())
}

/// The checks before the body is read: not forwarded, the API key, and the operator id (`0`, the
/// host, when the caller names none).
pub fn check_headers(headers: &HeaderMap, api_key: &str) -> Result<Operator, Refusal> {
    if headers.keys().any(|name| {
        let name = name.as_str();
        FORWARDED.contains(&name) || name.starts_with("x-forwarded-")
    }) {
        return Err(Refusal::Forwarded);
    }
    let authorization = one_header(headers, "authorization").map_err(|()| Refusal::Unauthenticated)?;
    let given = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(Refusal::Unauthenticated)?;
    if !same_secret(given, api_key) {
        return Err(Refusal::Unauthenticated);
    }
    match one_header(headers, OPERATOR_HEADER).map_err(|()| Refusal::BadOperator)? {
        None => Ok(Operator::host()),
        Some(id) => Operator::parse(id).ok_or(Refusal::BadOperator),
    }
}

pub fn is_json(headers: &HeaderMap) -> bool {
    one_header(headers, "content-type").ok().flatten().is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
    })
}

/// One GraphQL request (`{query, operationName?, variables?}`); a batch is refused.
pub fn parse_body(body: &[u8]) -> Result<async_graphql::Request, Refusal> {
    if body.len() > MAX_BODY {
        return Err(Refusal::BadBody);
    }
    serde_json::from_slice(body).map_err(|_| Refusal::BadBody)
}

/// A backend account id: what `/admin/v1/players?ids=` accepts, so it is also safe in a path.
pub fn is_account_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A player's name as the bot gives it: the launcher's username rule, after trimming.
pub fn username(given: &str) -> Option<&str> {
    let name = given.trim();
    protocol::account::is_valid_username(name).then_some(name)
}

/// A reason as the player will read it: one line, 3 to [`MAX_REASON`] characters.
pub fn reason(given: &str) -> Option<String> {
    let one_line: String = given.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let text = one_line.trim();
    (3..=MAX_REASON)
        .contains(&text.chars().count())
        .then(|| text.to_owned())
}

/// When a timeout ends, in ms since 1970: `minutes` from `now`, or an RFC 3339 time. Exactly one
/// is given, and the end is in the future and within [`MAX_TIMEOUT_MS`].
pub fn timeout_end(minutes: Option<i64>, until: Option<&str>, now: i64) -> Result<i64, &'static str> {
    let end = match (minutes, until) {
        (Some(minutes), None) => minutes
            .checked_mul(60_000)
            .and_then(|ms| now.checked_add(ms))
            .ok_or("minutes is out of range")?,
        (None, Some(until)) => chrono::DateTime::parse_from_rfc3339(until.trim())
            .map_err(|_| "until must be an RFC 3339 time, such as 2026-10-25T18:00:00Z")?
            .timestamp_millis(),
        _ => return Err("give either minutes or until"),
    };
    if end <= now {
        return Err("a timeout must end in the future");
    }
    if end - now > MAX_TIMEOUT_MS {
        return Err("a timeout ends within a year; use ban for longer");
    }
    Ok(end)
}

pub fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

// ------------------------------------------------------------------ the Director's instances

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceKind {
    City,
    Hunt,
    Dojo,
    Tutorial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub id: String,
    pub kind: InstanceKind,
    pub title: String,
    pub map: String,
    pub behemoth: Option<String>,
    pub players: u32,
    pub max_players: u32,
    pub player_ids: Vec<String>,
    pub started_at: Option<String>,
}

fn take(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// `"/Game/Maps/islands/foo/foo_persistent"` → `foo_persistent` (`ShortAssetName` in
/// `reforgedApiRules.ts`, so the bot and the launcher show the same names).
pub fn short_asset_name(asset: &str) -> String {
    let last = asset.rsplit('/').next().unwrap_or(asset);
    take(last.split('.').next().unwrap_or(last), 64)
}

/// `InstanceTitle` in `reforgedApiRules.ts`.
pub fn instance_title(kind: InstanceKind, behemoth: Option<&str>, map: &str) -> String {
    match kind {
        InstanceKind::City => "Ramsgate".into(),
        InstanceKind::Dojo => "Training Grounds".into(),
        InstanceKind::Tutorial => "Tutorial".into(),
        InstanceKind::Hunt => {
            if let Some(behemoth) = behemoth {
                let name = short_asset_name(behemoth);
                let name = name
                    .strip_prefix("BP_")
                    .or_else(|| name.strip_prefix("BPC_"))
                    .unwrap_or(&name);
                let name = name.strip_suffix("_C").unwrap_or(name);
                return take(&name.replace('_', " "), 64);
            }
            let name = short_asset_name(map);
            let cut = name.len().saturating_sub("_persistent".len());
            let name = match name.get(cut..) {
                Some(end) if end.eq_ignore_ascii_case("_persistent") => &name[..cut],
                _ => name.as_str(),
            };
            let title = take(&name.replace('_', " "), 64);
            if title.is_empty() { "Hunt".into() } else { title }
        }
    }
}

fn count(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map_or(0, |n| n.max(0.0).min(f64::from(u32::MAX)).floor() as u32)
}

/// The Director's `GET /api/instances`, checked field by field as `ParseDirectorInstances` does: an
/// odd entry is skipped, never an error. Players are listed for every kind, not only hunts.
pub fn parse_instances(raw: &Value) -> Vec<Instance> {
    let Some(entries) = raw.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .take(100)
        .filter_map(|entry| {
            let id = entry
                .get("id")?
                .as_str()
                .map(|id| take(id, 64))
                .filter(|id| !id.is_empty())?;
            let kind = match entry.get("kind")?.as_str()? {
                "city" => InstanceKind::City,
                "hunt" => InstanceKind::Hunt,
                "dojo" => InstanceKind::Dojo,
                "tutorial" => InstanceKind::Tutorial,
                _ => return None,
            };
            let map = entry
                .get("map")
                .and_then(Value::as_str)
                .map(|m| take(m, 160))
                .unwrap_or_default();
            let behemoth = entry.get("behemoth").and_then(Value::as_str).map(|b| take(b, 160));
            let player_ids = entry
                .get("playerIds")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .take(16)
                        .filter_map(Value::as_str)
                        .filter(|id| id.len() <= 128)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let started_at = entry
                .get("startedAt")
                .and_then(Value::as_str)
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| iso(t.timestamp_millis()));
            Some(Instance {
                title: instance_title(kind, behemoth.as_deref(), &map),
                map: short_asset_name(&map),
                behemoth: behemoth.as_deref().map(short_asset_name),
                players: count(entry.get("players")),
                max_players: count(entry.get("maxPlayers")),
                id,
                kind,
                player_ids,
                started_at,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;
    use serde_json::json;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn a_request_needs_the_key_and_no_proxy_in_between() {
        let bearer = format!("Bearer {KEY}");
        assert_eq!(
            check_headers(&headers(&[("authorization", &bearer)]), KEY).map(|o| o.id().to_owned()),
            Ok("0".into())
        );
        assert_eq!(
            check_headers(&headers(&[("authorization", &bearer), (OPERATOR_HEADER, "42")]), KEY)
                .map(|o| o.id().to_owned()),
            Ok("42".into())
        );
        for (pairs, refusal) in [
            (vec![], Refusal::Unauthenticated),
            (vec![("authorization", KEY)], Refusal::Unauthenticated),
            (vec![("authorization", "Bearer nope")], Refusal::Unauthenticated),
            (
                vec![("authorization", bearer.as_str()), ("authorization", bearer.as_str())],
                Refusal::Unauthenticated,
            ),
            (
                vec![("authorization", bearer.as_str()), ("x-forwarded-for", "1.2.3.4")],
                Refusal::Forwarded,
            ),
            (
                vec![("authorization", bearer.as_str()), ("x-forwarded-host", "a")],
                Refusal::Forwarded,
            ),
            (
                vec![("authorization", bearer.as_str()), ("x-reforged-gateway", "s")],
                Refusal::Forwarded,
            ),
            (
                vec![("authorization", bearer.as_str()), ("forwarded", "for=a")],
                Refusal::Forwarded,
            ),
            (
                vec![("authorization", bearer.as_str()), (OPERATOR_HEADER, "abc")],
                Refusal::BadOperator,
            ),
        ] {
            assert_eq!(
                check_headers(&headers(&pairs), KEY).map(|_| ()),
                Err(refusal),
                "{pairs:?}"
            );
        }
    }

    #[test]
    fn the_body_is_one_json_request() {
        assert!(is_json(&headers(&[(
            "content-type",
            "application/json; charset=utf-8"
        )])));
        assert!(!is_json(&headers(&[("content-type", "text/plain")])));
        assert!(!is_json(&headers(&[])));
        assert!(parse_body(br#"{"query":"{ serverStatus { name } }"}"#).is_ok());
        assert_eq!(parse_body(br#"[{"query":"{ a }"}]"#).map(|_| ()), Err(Refusal::BadBody));
        assert_eq!(parse_body(b"nope").map(|_| ()), Err(Refusal::BadBody));
        let big = format!(r#"{{"query":"{}"}}"#, "a".repeat(MAX_BODY));
        assert_eq!(parse_body(big.as_bytes()).map(|_| ()), Err(Refusal::BadBody));
    }

    #[test]
    fn names_ids_and_reasons() {
        assert_eq!(username(" Slayer_01 "), Some("Slayer_01"));
        for bad in ["ab", "bad name", "x/../y", &"a".repeat(17)] {
            assert_eq!(username(bad), None, "{bad}");
        }
        assert!(is_account_id("0b9d6c1e-2f4a-4c5d-8e7f-0123456789ab"));
        assert!(!is_account_id("a/b") && !is_account_id("") && !is_account_id(&"a".repeat(65)));
        assert_eq!(reason(" spamming\nchat ").as_deref(), Some("spamming chat"));
        assert_eq!(reason("no"), None);
        assert_eq!(reason(&"x".repeat(MAX_REASON + 1)), None);
    }

    #[test]
    fn a_timeout_ends_in_the_future_and_within_a_year() {
        let now = 1_800_000_000_000;
        assert_eq!(timeout_end(Some(30), None, now), Ok(now + 30 * 60_000));
        // now is 2027-01-15T08:00:00Z
        assert_eq!(
            timeout_end(None, Some("2027-01-16T08:00:00Z"), now),
            Ok(now + 86_400_000)
        );
        assert!(timeout_end(None, Some("2027-01-15T08:00:00Z"), now).is_err());
        assert!(timeout_end(Some(0), None, now).is_err());
        assert!(timeout_end(Some(-5), None, now).is_err());
        assert!(timeout_end(Some(i64::MAX), None, now).is_err());
        assert!(timeout_end(Some(366 * 24 * 60), None, now).is_err());
        assert!(timeout_end(Some(5), Some("2027-01-16T08:00:00Z"), now).is_err());
        assert!(timeout_end(None, None, now).is_err());
        assert!(timeout_end(None, Some("tomorrow"), now).is_err());
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn director_instances_are_checked_field_by_field() {
        let instances = parse_instances(&json!([
            {"id": "city-8777", "kind": "city", "map": "/Game/Maps/ramsgate/ramsgate_01_persistent", "behemoth": null,
             "players": 3, "maxPlayers": 0, "playerIds": ["u3"], "startedAt": "2026-10-01T10:00:00.000Z"},
            {"id": "hunt-8770", "kind": "hunt", "map": "/Game/Maps/islands/a/IslandA_persistent",
             "behemoth": "/Game/Behemoths/BP_Skarn.BP_Skarn_C", "players": 2.7, "maxPlayers": 4,
             "playerIds": ["u1", "u2", 7], "startedAt": "nope"},
            {"id": "hunt-8769", "kind": "hunt", "map": "/Game/Maps/islands/b/Island_B_Persistent", "players": -1},
            {"id": "x", "kind": "spaceship"},
            {"kind": "hunt"},
            "junk",
            null
        ]));
        assert_eq!(instances.len(), 3);
        assert_eq!(instances[0].title, "Ramsgate");
        assert_eq!(instances[0].map, "ramsgate_01_persistent");
        assert_eq!(instances[0].player_ids, ["u3"]);
        assert_eq!(instances[0].started_at.as_deref(), Some("2026-10-01T10:00:00Z"));
        assert_eq!(instances[1].title, "Skarn");
        assert_eq!(instances[1].behemoth.as_deref(), Some("BP_Skarn"));
        assert_eq!((instances[1].players, instances[1].max_players), (2, 4));
        assert_eq!(instances[1].player_ids, ["u1", "u2"]);
        assert_eq!(instances[1].started_at, None);
        assert_eq!(instances[2].title, "Island B");
        assert_eq!(instances[2].players, 0);
        assert!(parse_instances(&json!({"not": "a list"})).is_empty());
    }
}
