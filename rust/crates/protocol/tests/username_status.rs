//! Dauntless Revived's `username-status.test.ts` (e1c5fec), the cases that still apply, with Reforged's
//! key prefix; plus the status fields Reforged added, as its `pinned.test.ts` and `controller.test.ts`
//! read them from a fake server (`test/fakes.ts`).

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use protocol::account::{check_username, extract_account_key, is_plausible_account_key, is_valid_username};
use protocol::status::{
    GameDataKind, InstanceKind, PlayerWhere, RegistrationMode, StatusGame, limited_view, parse_server_status,
};
use serde_json::{Value, json};

const RLO: char = '\u{202e}';
const BELL: char = '\u{7}';

#[test]
fn username_rules() {
    for (name, result) in [
        ("Slayer_01", "ok"),
        ("abc", "ok"),
        (&*"a".repeat(16), "ok"),
        ("", "empty"),
        ("ab", "too_short"),
        (&*"a".repeat(17), "too_long"),
        ("bad name", "bad_chars"),
        ("näyttö", "bad_chars"),
        ("x-y", "bad_chars"),
        ("<script>", "bad_chars"),
        ("MiauStyleZzツ", "bad_chars"),
    ] {
        assert_eq!(check_username(name).as_str(), result, "{name:?}");
    }
    assert!(is_valid_username("Slayer_01") && !is_valid_username("MiauStyleZzツ"));
}

#[test]
fn account_keys() {
    let key = format!("RRK_{}", "ab".repeat(24));
    assert!(is_plausible_account_key(&key));
    assert!(!is_plausible_account_key("short"));
    assert!(!is_plausible_account_key("RRK_has space"));
    assert!(!is_plausible_account_key("RRK_\"quote\""));
    assert_eq!(extract_account_key(&format!("  {key}\r\n")), Some(key.as_str()));
    assert_eq!(
        extract_account_key(&format!(
            "Dauntless Reforged Revival account key\r\n\r\nServer: X\r\nKey: {key}\r\n\r\nKeep this file private."
        )),
        Some(key.as_str())
    );
    // A backup file pasted into a one-line field: only a server-made key's exact shape is found
    assert_eq!(
        extract_account_key(&format!("Server: X Key: {key} Keep this file private.")),
        Some(key.as_str())
    );
    assert_eq!(extract_account_key("not a key at all"), None);
}

fn full_status() -> Value {
    json!({
        "name": "Alex's Ramsgate",
        "online": true,
        "version": "1.2.0",
        "commit": "a4cb7cf",
        "sourceUrl": "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival",
        "registration": "INVITECODE",
        "playersOnline": 3,
        "players": [
            { "name": "Aurora", "where": "city", "instance": "8777" },
            { "name": "Borealis", "where": "hunt", "instance": "8770" },
            { "name": "Cirrus", "where": "somewhere-new", "instance": null },
        ],
        "instances": [
            { "id": "8770", "kind": "hunt", "title": "Hunt: Shrike", "map": "Hunt_Shrike", "behemoth": "Shrike", "players": 1, "maxPlayers": 4, "startedAt": "2026-09-21T10:00:00Z" },
            { "id": "8777", "kind": "city", "title": "Ramsgate", "map": "Ramsgate", "behemoth": null, "players": 1, "maxPlayers": 100, "startedAt": "2026-09-21T08:00:00.000Z" },
            { "id": "8776", "kind": "dojo", "title": "Training Dojo", "map": "Dojo", "behemoth": null, "players": 0, "maxPlayers": 1, "startedAt": "garbage" },
        ],
        "uptimeSeconds": 12345,
        "game": { "build": "rel-1.14.7-Archon", "changelist": 647472, "executableSha256": null },
        "gameData": "real",
        "chat": true,
    })
}

fn with(mut value: Value, key: &str, field: Value) -> Value {
    value[key] = field;
    value
}

#[test]
fn parses_the_full_contract() {
    let s = parse_server_status(&full_status()).unwrap();
    assert_eq!(s.name, "Alex's Ramsgate");
    assert!(s.online);
    assert_eq!(s.registration, Some(RegistrationMode::Invitecode));
    assert_eq!(s.players_online, 3);
    assert_eq!(s.players.len(), 3);
    assert_eq!(s.players[2].location, PlayerWhere::Unknown);
    assert_eq!(s.instances.len(), 3);
    assert_eq!(s.instances[2].started_at, None);
    assert_eq!(s.instances[1].started_at.as_deref(), Some("2026-09-21T08:00:00.000Z"));
    assert_eq!(s.instances[0].kind, InstanceKind::Hunt);
    assert_eq!(s.content_port, None);
    assert_eq!(
        s.source_url.as_deref(),
        Some("https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival")
    );
    assert!(!s.limited);
    // Reforged's fields
    assert_eq!(
        s.game,
        Some(StatusGame {
            build: "rel-1.14.7-Archon".into(),
            changelist: Some(647472)
        })
    );
    assert_eq!(s.game_data, GameDataKind::Real);
    assert!(s.chat);
    let synthetic = parse_server_status(&with(full_status(), "gameData", json!("synthetic"))).unwrap();
    assert_eq!(synthetic.game_data, GameDataKind::Synthetic);
}

#[test]
fn rejects_or_cleans_hostile_input() {
    assert_eq!(parse_server_status(&Value::Null), None);
    assert_eq!(parse_server_status(&json!([])), None);
    assert_eq!(parse_server_status(&json!({ "online": true })), None); // no name
    let s = parse_server_status(&json!({
        "name": format!("  Evil{RLO}{BELL} Server  "),
        "online": "yes",
        "sourceUrl": "javascript:alert(1)",
        "registration": "WHATEVER",
        "playersOnline": -5,
        "players": [{ "name": "<img src=x onerror=alert(1)>", "where": "city" }, "junk", { "where": "hunt" }],
        "instances": [{ "id": "x", "kind": "raid", "title": "t" }, { "id": "y", "kind": "hunt", "players": 1e9, "maxPlayers": "4" }],
        "contentPort": 70000,
        "uptimeSeconds": "long",
        "game": { "build": 7 },
        "gameData": "fake",
        "chat": "yes",
    }))
    .unwrap();
    assert_eq!(s.name, "Evil Server");
    assert!(!s.online);
    assert_eq!(s.source_url, None);
    assert_eq!(s.registration, None);
    assert_eq!(s.players_online, 0);
    assert_eq!(s.players.len(), 1);
    assert_eq!(s.players[0].name, "<img src=x onerror=alert(1)>"); // kept as text; the UI never renders HTML
    assert_eq!(s.instances.len(), 1);
    assert_eq!(s.instances[0].players, 1000);
    assert_eq!(s.instances[0].max_players, 0);
    assert_eq!(s.content_port, Some(65535));
    assert_eq!(s.uptime_seconds, 0);
    assert_eq!(s.game, None);
    assert_eq!(s.game_data, GameDataKind::Unknown);
    assert!(!s.chat);
    let only = |field: &str, value: Value| parse_server_status(&json!({ "name": "x", field: value })).unwrap();
    assert_eq!(only("contentPort", json!(0)).content_port, None);
    assert_eq!(only("contentPort", Value::Null).content_port, None);
    assert_eq!(only("sourceUrl", json!("http://example.com")).source_url, None);
    assert_eq!(only("sourceUrl", json!("https://u:p@example.com")).source_url, None);
    // A build without a changelist is still named
    assert_eq!(
        only("game", json!({ "build": "rel" })).game,
        Some(StatusGame {
            build: "rel".into(),
            changelist: None
        })
    );
}

#[test]
fn a_full_answer_is_not_limited_and_neither_is_an_older_servers_without_the_field() {
    assert!(
        !parse_server_status(&with(full_status(), "limited", json!(false)))
            .unwrap()
            .limited
    );
    assert!(!parse_server_status(&full_status()).unwrap().limited);
    for odd in [json!("true"), json!(1), Value::Null, json!({}), json!([true])] {
        let s = parse_server_status(&with(full_status(), "limited", odd.clone())).unwrap();
        assert!(!s.limited, "{odd}");
        assert_eq!(s.players.len(), 3, "only an explicit true hides the list");
    }
}

#[test]
fn a_limited_answer_keeps_the_servers_details_and_lists_nobody() {
    let mut limited = full_status();
    limited["playersOnline"] = json!(0);
    limited["players"] = json!([]);
    limited["instances"] = json!([]);
    limited["limited"] = json!(true);
    let s = parse_server_status(&limited).unwrap();
    assert!(s.limited);
    assert!(s.online, "the server is still online");
    assert_eq!((s.players_online, s.players.len(), s.instances.len()), (0, 0, 0));
    assert_eq!(
        (s.name.as_str(), s.version.as_str(), s.commit.as_str()),
        ("Alex's Ramsgate", "1.2.0", "a4cb7cf")
    );
    assert_eq!(
        (s.registration, s.uptime_seconds),
        (Some(RegistrationMode::Invitecode), 12345)
    );
    assert_eq!(s.game_data, GameDataKind::Real);

    // A server that says limited but sends a list anyway: nothing of it is shown
    let odd = parse_server_status(&with(full_status(), "limited", json!(true))).unwrap();
    assert_eq!(
        (odd.limited, odd.players_online, odd.players.len(), odd.instances.len()),
        (true, 0, 0, 0)
    );
}

#[test]
fn limited_view_hides_the_list_of_a_status_the_launcher_can_no_longer_see() {
    let s = parse_server_status(&full_status()).unwrap();
    let v = limited_view(s.clone());
    assert_eq!(
        (v.limited, v.players_online, v.players.len(), v.instances.len()),
        (true, 0, 0, 0)
    );
    assert_eq!(
        (&v.name, v.online, v.registration, &v.game),
        (&s.name, s.online, s.registration, &s.game)
    );
    assert_eq!(s.players.len(), 3, "the original is left alone");
}

#[test]
fn caps_list_sizes() {
    let players: Vec<Value> = (0..2000)
        .map(|i| json!({ "name": format!("p{i}"), "where": "menu" }))
        .collect();
    assert_eq!(
        parse_server_status(&json!({ "name": "x", "players": players }))
            .unwrap()
            .players
            .len(),
        500
    );
}
