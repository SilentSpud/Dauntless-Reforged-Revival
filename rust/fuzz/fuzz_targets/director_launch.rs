//! What decides a game server's launch (docs/security/director.md): the backend's matchmaking body
//! (game mode, travel arguments, hunt id, party), the hunt tables it resolves against, and the line
//! the runtime prints when it is ready. None of it may panic; a resolved manifest names a /Game/ map
//! and a party of one to four; the command line splits back into exactly the arguments given, so a
//! map or Behemoth from the client's travel URL can never become an argument of its own; a readiness
//! line counts only for its own launch.
#![no_main]

use director::gameservers::{Launch, arg_value, launch_args, length, nonblank};
use director::hunt::{self, HuntResolver, ResolverOptions, Tables};
use libfuzzer_sys::arbitrary::{self, Arbitrary};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

#[derive(Debug, Arbitrary)]
struct Input<'a> {
    tables: &'a [u8],
    body: &'a [u8],
    game_args: &'a str,
    pick_last: bool,
    now: i64,
    launch_id: &'a str,
    port: u16,
    line: &'a str,
}

fn object(value: Option<&Value>) -> serde_json::Map<String, Value> {
    value.and_then(Value::as_object).cloned().unwrap_or_default()
}

/// The MSVC runtime's split of the arguments after the program (no `""`-in-quotes form: quote() never
/// writes one inside a quoted argument).
fn split(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    // The program: quoted whole.
    if chars.first() == Some(&'"') {
        i = 1;
        while i < chars.len() && chars[i] != '"' {
            i += 1;
        }
        i += 1;
    }
    let mut args = Vec::new();
    loop {
        while i < chars.len() && matches!(chars[i], ' ' | '\t') {
            i += 1;
        }
        if i >= chars.len() {
            return args;
        }
        let (mut arg, mut quoted) = (String::new(), false);
        while i < chars.len() && (quoted || !matches!(chars[i], ' ' | '\t')) {
            let mut backslashes = 0;
            while i < chars.len() && chars[i] == '\\' {
                backslashes += 1;
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                arg.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes % 2 == 1 {
                    arg.push('"');
                } else {
                    quoted = !quoted;
                }
                i += 1;
            } else {
                arg.extend(std::iter::repeat_n('\\', backslashes));
                if i < chars.len() && (quoted || !matches!(chars[i], ' ' | '\t')) {
                    arg.push(chars[i]);
                    i += 1;
                }
            }
        }
        args.push(arg);
    }
}

fuzz_target!(|input: Input<'_>| {
    let tables: Value = serde_json::from_slice(input.tables).unwrap_or(Value::Null);
    let mut sources = vec![("matchmaker_hunts_table".to_owned(), object(tables.get("matchmaker")))];
    sources.push(("arena_easy_matchmaker_hunts".to_owned(), object(tables.get("arena"))));
    let mut options = ResolverOptions::new(Tables {
        player_rows: object(tables.get("player")),
        matchmaker_sources: sources,
    });
    let pick_last = input.pick_last;
    options.random_int = Box::new(move |min, max| if pick_last { max.saturating_sub(1).max(min) } else { min });
    let now = input.now;
    options.now = Box::new(move || now);
    let resolver = HuntResolver::new(options);

    let body: Value = serde_json::from_slice(input.body).unwrap_or(Value::Null);
    let (hunt_id, expected) = (body.get("HuntId"), body.get("ExpectedPlayers"));
    let _ = nonblank(hunt_id);
    let _ = length(expected);
    for result in [
        resolver.resolve_manifest(hunt_id, expected),
        hunt::resolve_player_hosted_manifest(&resolver, hunt_id, expected),
    ] {
        if let Ok(manifest) = result {
            assert!(manifest["mapPath"].as_str().is_some_and(|m| m.starts_with("/Game/")));
            let party = manifest["expectedPlayerString"].as_str().unwrap_or("");
            assert!((1..=4).contains(&expected.and_then(Value::as_array).map_or(0, Vec::len)), "{party}");
        }
    }
    if let Some(query) = tables.get("query") {
        let tags: Vec<Value> = tables.get("tags").and_then(Value::as_array).cloned().unwrap_or_default();
        let _ = hunt::evaluate_tag_query(query, &tags);
    }
    let _ = hunt::audit_coverage(&resolver);
    let _ = hunt::validate_tables(&resolver);

    // A hunt from travel arguments: the map before the first '?', the Behemoth from ?MonsterClass=.
    let map = input.game_args.split('?').next().unwrap_or("");
    let behemoth = arg_value(input.game_args, "MonsterClass");
    let matchmaker = arg_value(input.game_args, "HuntID");
    let args = launch_args(&Launch {
        key: "gameserver-key",
        port: input.port,
        map,
        behemoth,
        matchmaker_hunt_id: matchmaker,
        players: "NO_EXPECTED_PLAYERS",
        my_ip: Some("100.64.0.1"),
        log_cmds: "LogLoad Off",
    });
    assert_eq!(args.len(), 14);
    assert_eq!(args[2], map);
    if !args.iter().any(|a| a.contains('\0')) {
        let line = director::cmdline::command_line(r"C:\Game\Dauntless-Win64-Shipping.exe", &args);
        assert_eq!(split(&line), args, "{line}");
    }

    if director::process::readiness(input.line, input.launch_id, input.port) == Some(true) {
        assert!(!input.launch_id.is_empty() && input.line.contains(input.launch_id));
    }
});
