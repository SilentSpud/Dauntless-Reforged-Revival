//! Registration (shared contract 2, `POST /undaunted/api/Register`): what the launcher sends and what the
//! metagame answers when the account was made. Refusals are `{error, message}` with a status code; the
//! launcher maps them (`hostapi.rs`), the metagame makes them (`accounts.rs`).

use serde::{Deserialize, Serialize};

/// The launcher's request. The metagame reads the body more leniently than this (any JSON, with Node's
/// checks), so a launcher that sends this shape is always understood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    #[serde(rename = "Username")]
    pub username: String,
    /// Empty where the server's registration is open
    #[serde(rename = "InviteCode")]
    pub invite_code: String,
}

/// The answer when the account was made: its key, shown to nobody else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    #[serde(rename = "UUK")]
    pub key: String,
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
            serde_json::to_string(&request).unwrap_or_default(),
            r#"{"Username":"Sam","InviteCode":"ABCD-EFGH-JKLM"}"#
        );
        let answer: Registered =
            serde_json::from_str(r#"{"UUK":"UUK_x"}"#).unwrap_or(Registered { key: String::new() });
        assert_eq!(answer.key, "UUK_x");
    }
}
