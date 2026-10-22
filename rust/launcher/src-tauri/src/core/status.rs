//! Live server status (`GET /reforged/api/ServerStatus`), row L12: `protocol::status` (RS1), which reads
//! `parseServerStatus` in `src/shared/status.ts` answer for answer. The answer comes from the network, so
//! every field is checked and cleaned before the page sees it. Both sides run
//! `crates/protocol/tests/cases/status.json`.

#[allow(unused_imports, reason = "the controller (RA13) uses the rest")]
pub use protocol::status::{STATUS_PATH, ServerStatus, clean_https_url, clean_text, iso_date, parse_server_status};

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    struct Case {
        input: Value,
        expected: Option<ServerStatus>,
        /// `JSON.stringify` of the TypeScript answer
        text: String,
    }

    #[test]
    fn the_shared_table() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/protocol/tests/cases/status.json"
        );
        let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(cases.len() > 90);
        for case in cases {
            let parsed = parse_server_status(&case.input);
            assert_eq!(parsed, case.expected, "{}", case.input);
            // The JSON the page receives over the bridge
            assert_eq!(serde_json::to_string(&parsed).unwrap(), case.text, "{}", case.input);
        }
    }

    #[test]
    fn dates_far_out() {
        assert_eq!(
            iso_date("+200000-09-13T00:00:00Z").as_deref(),
            Some("+200000-09-13T00:00:00.000Z")
        );
        assert_eq!(
            iso_date("-000001-01-01T00:00:00Z").as_deref(),
            Some("-000001-01-01T00:00:00.000Z")
        );
        assert_eq!(iso_date("-000000-01-01T00:00:00Z"), None);
        assert_eq!(
            iso_date("2026-09-21T24:00:00Z").as_deref(),
            Some("2026-09-22T00:00:00.000Z")
        );
        assert_eq!(iso_date("2026-02-30T00:00:00Z"), None);
    }

    #[test]
    fn text_is_cleaned_as_the_page_expects() {
        assert_eq!(
            clean_text(Some(&Value::from(" a\u{202e}\t\u{3000}b ")), 32).as_deref(),
            Some("a b")
        );
        assert_eq!(clean_text(Some(&Value::from("\u{7}")), 32), None);
        assert_eq!(clean_text(Some(&Value::from(5)), 32), None);
        assert_eq!(clean_text(Some(&Value::from("abcdef")), 3).as_deref(), Some("abc"));
    }
}
