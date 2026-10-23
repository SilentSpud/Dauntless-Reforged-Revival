//! Usernames and pasted account keys (row L12): `protocol::account` (RS1), which reads
//! `src/shared/username.ts` answer for answer. Reforged's registration rule, also for stored usernames
//! (RD22); keys a Reforged server makes are `RRK_<48 hex>`. Both sides run
//! `crates/protocol/tests/cases/username.json`.

#[allow(unused_imports, reason = "the controller (RA13) and the keystore (RA8) use the rest")]
pub use protocol::account::{
    ACCOUNT_KEY_PREFIX, USERNAME_MAX, USERNAME_MIN, UsernameCheck, check_username, extract_account_key,
    is_plausible_account_key, is_valid_username,
};

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
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/protocol/tests/cases/username.json"
        );
        let cases: Cases = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for case in cases.check {
            // The page may send a non-string, which TypeScript reads as empty
            let result = case.input.as_str().map_or(UsernameCheck::Empty, check_username);
            assert_eq!(result.as_str(), case.result, "{:?}", case.input);
            assert_eq!(
                case.input.as_str().is_some_and(is_valid_username),
                case.result == "ok",
                "{:?}",
                case.input
            );
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

    #[test]
    fn server_made_keys_are_reforged_keys() {
        let key = format!("{ACCOUNT_KEY_PREFIX}{}", "ab".repeat(24));
        assert!(is_plausible_account_key(&key));
        assert_eq!(
            extract_account_key(&format!("Server: X Key: {key} Saved")),
            Some(key.as_str())
        );
    }
}
