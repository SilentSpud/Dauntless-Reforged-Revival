//! The gateway's traffic capture (MIGRATION_PLAN.md RD19, RX3; docs/security/gateway.md): request and
//! answer bodies, headers, targets and chat frames from the network, redacted before they are
//! written. None of it may panic; no account key or token-shaped text from the input survives in the
//! output; and the chat frame reader gives the same messages however the bytes are split.
#![no_main]

use arbitrary::Arbitrary;
use gateway::capture::{
    FrameReader, redact_capture_body, redact_capture_headers, redact_capture_text, redact_chat_frame,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    body: &'a [u8],
    content_type: Option<&'a str>,
    extra: u16,
    max: u16,
    headers: Vec<(&'a [u8], &'a [u8])>,
    frames: &'a [u8],
    split: u16,
}

/// Account keys and whole tokens: what must never be written.
fn secrets(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')) {
        if (word.starts_with("RRK_") || word.starts_with("UUK_"))
            && word.len() > 4
            && word[4..].bytes().all(|b| b.is_ascii_alphanumeric())
        {
            found.push(word.to_owned());
        }
        let parts: Vec<&str> = word.split('.').collect();
        if parts.len() == 3 && parts[0].len() > 3 && parts[0].starts_with("eyJ") && parts.iter().all(|p| !p.is_empty())
        {
            found.push(word.to_owned());
        }
    }
    found
}

fuzz_target!(|input: Input<'_>| {
    let text = String::from_utf8_lossy(input.body);
    let redacted = redact_capture_text(&text);
    for secret in secrets(&text) {
        assert!(!redacted.contains(&secret), "{secret} survived");
    }
    let _ = redact_chat_frame(&text);
    let total = input.body.len() as u64 + u64::from(input.extra % 2);
    let _ = redact_capture_body(input.body, input.content_type, total, usize::from(input.max));

    let mut headers = HeaderMap::new();
    for (name, value) in input.headers.iter().take(32) {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name), HeaderValue::from_bytes(value)) {
            headers.append(name, value);
        }
    }
    let _ = redact_capture_headers(&headers);

    let max = usize::from(input.max);
    let whole = FrameReader::new(max).feed(input.frames);
    let at = usize::from(input.split).min(input.frames.len());
    let mut reader = FrameReader::new(max);
    let mut split = reader.feed(&input.frames[..at]);
    split.extend(reader.feed(&input.frames[at..]));
    assert_eq!(whole, split);
});
