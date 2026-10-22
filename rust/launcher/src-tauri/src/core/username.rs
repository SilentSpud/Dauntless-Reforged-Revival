//! Usernames and pasted account keys (row L12): `src/shared/username.ts`, answer for answer. Both run
//! `test/cases/username.json`.

use super::text::{is_js_space, js_length, js_trim};

pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 16;

/// Usernames (roadmap 1.6): 3-16 characters, letters A-Z/a-z, digits and underscore. Uniqueness
/// (case-insensitive) is the server's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Account keys go on the game's command line (`-AUTH_PASSWORD=<key>`), so only plain characters are
/// accepted. Keys made by the server look like `UUK_<48 hex>`.
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

/// `/Key:\s*(UUK_[0-9a-fA-F]{48})(?![0-9a-fA-F])/`: a server-made key in a backup file that lost its
/// line breaks.
fn made_key(text: &str) -> Option<&str> {
    for (at, _) in text.match_indices("Key:") {
        let rest = text[at + 4..].trim_start_matches(is_js_space);
        let Some(hex) = rest.strip_prefix("UUK_") else {
            continue;
        };
        let length = hex.len() - hex.trim_start_matches(|c: char| c.is_ascii_hexdigit()).len();
        if length == 48 {
            return Some(&rest[..52]);
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
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Cases {
        check: Vec<Check>,
        plausible_key: Vec<Plausible>,
        extract_key: Vec<Extract>,
    }

    #[derive(Deserialize)]
    struct Check {
        input: serde_json::Value,
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
    fn the_shared_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/username.json");
        let cases: Cases = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
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
}
