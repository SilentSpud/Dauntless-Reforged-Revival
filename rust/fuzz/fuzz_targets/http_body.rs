//! Request bodies and query strings (MIGRATION_PLAN.md M8, X9): every service reads JSON and form bodies
//! with `service_kit::body` before routing, and decodes URL components with Node's rules. None of it may
//! panic, and a body that parses must survive the JSON round trip the services answer with.
#![no_main]

use libfuzzer_sys::fuzz_target;
use service_kit::body::{parse_form, parse_json};
use service_kit::js::{date_parse, decode_uri_component};
use service_kit::jsjson;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(value) = parse_json(&text) {
        let written = jsjson::stringify(&value);
        assert!(jsjson::parse(&written).is_ok(), "the services' own JSON must read back");
    }
    if let Ok(value) = parse_form(&text) {
        let _ = jsjson::stringify(&value);
    }
    if let Some(decoded) = decode_uri_component(&text) {
        // Decoding only ever shortens: %XX becomes one byte
        assert!(decoded.len() <= text.len());
    }
    let _ = date_parse(&text);
});
