//! The shared tables in `tests/cases`: Dauntless Revived's launcher tables (`test/cases` at e1c5fec), with
//! Reforged's scheme and key prefix put in and the answers written by Reforged's TypeScript
//! (`tests/cases/generate.js`). The launcher's page runs the same TypeScript, so both sides answer alike.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use protocol::account::{UsernameCheck, check_username, extract_account_key, is_plausible_account_key};
use protocol::invite::{Invite, parse_invite};
use protocol::status::{ServerStatus, parse_server_status};
use serde::Deserialize;
use serde_json::Value;

fn table<T: for<'a> Deserialize<'a>>(name: &str) -> T {
    let path = format!("{}/tests/cases/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[derive(Deserialize)]
struct Invites {
    accept: Vec<Accept>,
    reject: Vec<Reject>,
}

#[derive(Deserialize)]
struct Accept {
    input: String,
    invite: Invite,
}

#[derive(Deserialize)]
struct Reject {
    input: String,
    error: String,
}

#[test]
fn invites() {
    let cases: Invites = table("invite.json");
    assert!(cases.accept.len() > 30 && cases.reject.len() > 150);
    assert!(cases.reject.iter().filter(|case| case.error == "revived").count() > 30);
    for case in cases.accept {
        assert_eq!(parse_invite(&case.input), Ok(case.invite), "{:?}", case.input);
    }
    for case in cases.reject {
        assert_eq!(
            parse_invite(&case.input).map_err(|error| error.as_str()),
            Err(case.error.as_str()),
            "{:?}",
            case.input
        );
    }
}

#[derive(Deserialize)]
struct Status {
    input: Value,
    expected: Option<ServerStatus>,
    /// `JSON.stringify` of the TypeScript answer
    text: String,
}

#[test]
fn statuses() {
    let cases: Vec<Status> = table("status.json");
    assert!(cases.len() > 90);
    for case in cases {
        let parsed = parse_server_status(&case.input);
        assert_eq!(parsed, case.expected, "{}", case.input);
        // And the same JSON text the TypeScript hands the page
        assert_eq!(serde_json::to_string(&parsed).unwrap(), case.text, "{}", case.input);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usernames {
    check: Vec<Check>,
    plausible_key: Vec<Plausible>,
    extract_key: Vec<Extract>,
}

#[derive(Deserialize)]
struct Check {
    input: Value,
    result: String,
}

#[derive(Deserialize)]
struct Plausible {
    input: String,
    ok: bool,
}

#[derive(Deserialize)]
struct Extract {
    input: String,
    key: Option<String>,
}

#[test]
fn usernames_and_keys() {
    let cases: Usernames = table("username.json");
    for case in cases.check {
        // The page may send a non-string, which TypeScript reads as empty
        let result = case.input.as_str().map_or(UsernameCheck::Empty, check_username);
        assert_eq!(result.as_str(), case.result, "{:?}", case.input);
    }
    for case in cases.plausible_key {
        assert_eq!(is_plausible_account_key(&case.input), case.ok, "{:?}", case.input);
    }
    for case in cases.extract_key {
        assert_eq!(
            extract_account_key(&case.input),
            case.key.as_deref(),
            "{:?}",
            case.input
        );
    }
}
