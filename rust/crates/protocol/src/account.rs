//! Accounts: registration (`POST /reforged/api/Register`), usernames and account keys, as
//! `legacy/ReforgedLauncher/src/shared/username.ts` and `src/main/hostapi.ts` have them. Refusals are
//! `{error, message}` with a status code; the launcher maps them, the backend makes them.

use serde::{Deserialize, Serialize};

use crate::text::{is_js_space, js_length, js_trim};

pub const REGISTER_PATH: &str = "/reforged/api/Register";
/// Keys made by a Reforged server are `RRK_` and 48 hex characters.
pub const ACCOUNT_KEY_PREFIX: &str = "RRK_";
pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 16;

/// The launcher's request. The backend reads the body more leniently than this (any JSON, with its own
/// checks), so a launcher that sends this shape is always understood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    #[serde(rename = "Username")]
    pub username: String,
    /// Empty where the server's registration is open
    #[serde(rename = "InviteCode")]
    pub invite_code: String,
}

/// The answer when the account was made: its key, shown to nobody else. The field keeps the game's
/// name, `UUK`; the key in it starts with [`ACCOUNT_KEY_PREFIX`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    #[serde(rename = "UUK")]
    pub key: String,
}

/// Reforged's registration rule: 3-16 characters, letters A-Z/a-z, digits and underscore. Uniqueness
/// (case-insensitive) is the server's job. The launcher also keeps only stored usernames that pass
/// (RD22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum UsernameCheck {
    Ok,
    Empty,
    TooShort,
    TooLong,
    BadChars,
}

impl UsernameCheck {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Empty => "empty",
            Self::TooShort => "too_short",
            Self::TooLong => "too_long",
            Self::BadChars => "bad_chars",
        }
    }
}

pub fn check_username(input: &str) -> UsernameCheck {
    if input.is_empty() {
        UsernameCheck::Empty
    } else if !input.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        UsernameCheck::BadChars
    } else if input.len() < USERNAME_MIN {
        UsernameCheck::TooShort
    } else if input.len() > USERNAME_MAX {
        UsernameCheck::TooLong
    } else {
        UsernameCheck::Ok
    }
}

pub fn is_valid_username(input: &str) -> bool {
    check_username(input) == UsernameCheck::Ok
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Account keys go on the game's command line (`-AUTH_PASSWORD=<key>`), so only plain characters are
/// accepted. Keys made by a Reforged server look like `RRK_<48 hex>`.
pub fn is_plausible_account_key(input: &str) -> bool {
    (8..=128).contains(&input.len()) && input.chars().all(is_key_char)
}

/// Where JavaScript's `^` and `$` match in a multiline expression.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `/^\s*Key:\s*([A-Za-z0-9_-]{8,128})\s*$/m`: a "Key:" line, the first one that holds a key.
fn labelled_key(text: &str) -> Option<&str> {
    for (at, _) in text.match_indices("Key:") {
        // Only white space between the start of its line and "Key:"
        let before = text[..at].chars().rev().take_while(|c| !is_line_terminator(*c));
        if !before.clone().all(is_js_space) {
            continue;
        }
        let after = &text[at + 4..];
        let start = after.len() - after.trim_start_matches(is_js_space).len();
        let rest = &after[start..];
        let length = rest.len() - rest.trim_start_matches(is_key_char).len();
        if !(8..=128).contains(&length) {
            continue;
        }
        // Then white space up to the end of a line
        let tail = &rest[length..];
        let space = tail.len() - tail.trim_start_matches(is_js_space).len();
        if space == tail.len() || tail[..space].chars().any(is_line_terminator) {
            return Some(&rest[..length]);
        }
    }
    None
}

/// `/Key:\s*(RRK_[0-9a-fA-F]{48})(?![0-9a-fA-F])/`: a server-made key in a backup file that lost its
/// line breaks.
fn made_key(text: &str) -> Option<&str> {
    for (at, _) in text.match_indices("Key:") {
        let rest = text[at + 4..].trim_start_matches(is_js_space);
        let Some(hex) = rest.strip_prefix(ACCOUNT_KEY_PREFIX) else {
            continue;
        };
        let length = hex.len() - hex.trim_start_matches(|c: char| c.is_ascii_hexdigit()).len();
        if length == 48 {
            return Some(&rest[..ACCOUNT_KEY_PREFIX.len() + 48]);
        }
    }
    None
}

/// A pasted key may come with spaces or line breaks around it, or as the whole backup file.
pub fn extract_account_key(text: &str) -> Option<&str> {
    if js_length(text) > 8192 {
        return None;
    }
    if let Some(key) = labelled_key(text) {
        return Some(key);
    }
    let trimmed = js_trim(text);
    if is_plausible_account_key(trimmed) {
        return Some(trimmed);
    }
    made_key(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_node_used() {
        let request = RegisterRequest {
            username: "Sam".into(),
            invite_code: "ABCD-EFGH-JKLM".into(),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"Username":"Sam","InviteCode":"ABCD-EFGH-JKLM"}"#
        );
        let answer: Registered = serde_json::from_str(r#"{"UUK":"RRK_x"}"#).unwrap();
        assert_eq!(answer.key, "RRK_x");
    }
}
