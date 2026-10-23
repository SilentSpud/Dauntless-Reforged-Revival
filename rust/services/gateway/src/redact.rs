//! Request targets as they go into the access log (`src/redact.ts`): never a login token, account key
//! or anything that looks like one. Some client routes carry a JWT in the path
//! (DELETE /account/api/oauth/sessions/kill/<token>), so the path is scrubbed too, not just the query.

use service_kit::js::decode_uri_component;

/// Query parameter names whose value is never logged (`/(key|token|...)/i`).
const SENSITIVE_NAMES: [&str; 10] = [
    "key", "token", "secret", "pass", "auth", "code", "session", "jwt", "sig", "cred",
];
const MAX_TARGET: usize = 400;

pub fn redact_text(text: &str) -> String {
    let out = redact_jwt(text);
    let out = redact_account_keys(&out);
    // /[0-9a-fA-F]{40,}/g, then /[\w-]{64,}/g (JavaScript's \w is ASCII only)
    let out = replace_runs(&out, |c| c.is_ascii_hexdigit(), 40);
    replace_runs(&out, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-', 64)
}

/// Every run of at least `min` characters that `inside` accepts becomes `<redacted>`.
fn replace_runs(text: &str, inside: impl Fn(char) -> bool, min: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= min {
            out.push_str("<redacted>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if inside(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `/RRK_[0-9A-Za-z]+/g` becomes `<key>` (Reforged's account keys; DR's were `UUK_`).
fn redact_account_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("RRK_") {
        let after = &rest[start + 4..];
        let length = after.bytes().take_while(u8::is_ascii_alphanumeric).count();
        if length == 0 {
            out.push_str(&rest[..start + 4]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str("<key>");
        rest = &after[length..];
    }
    out.push_str(rest);
    out
}

fn is_jwt_byte(byte: Option<&u8>) -> bool {
    byte.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

/// Every `eyJ<part>.<part>.<optional part>` becomes `<token>`.
fn redact_jwt(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while from < text.len() {
        let Some(start) = text[from..].find("eyJ").map(|i| from + i) else {
            out.push_str(&text[from..]);
            break;
        };
        let mut at = start + 3;
        while is_jwt_byte(bytes.get(at)) {
            at += 1;
        }
        let header_ok = at > start + 3 && bytes.get(at) == Some(&b'.');
        at += 1;
        let payload = at;
        if header_ok {
            while is_jwt_byte(bytes.get(at)) {
                at += 1;
            }
        }
        if !header_ok || at == payload || bytes.get(at) != Some(&b'.') {
            out.push_str(&text[from..start + 3]);
            from = start + 3;
            continue;
        }
        at += 1;
        while is_jwt_byte(bytes.get(at)) {
            at += 1;
        }
        out.push_str(&text[from..start]);
        out.push_str("<token>");
        from = at;
    }
    out
}

fn safe_decode(text: &str, plus_is_space: bool) -> String {
    let spaced;
    let input = if plus_is_space {
        spaced = text.replace('+', " ");
        spaced.as_str()
    } else {
        text
    };
    decode_uri_component(input).unwrap_or_else(|| text.to_owned())
}

/// Redacts the raw text, and if the percent-decoded text hides a token (for example "%2E" between
/// the parts of a JWT), logs the decoded text with that token removed instead.
fn redact_encoded(raw: &str, plus_is_space: bool) -> String {
    let decoded = safe_decode(raw, plus_is_space);
    if decoded != raw {
        let redacted = redact_text(&decoded);
        if redacted != decoded {
            return redacted;
        }
    }
    redact_text(raw)
}

fn is_sensitive(name: &str) -> bool {
    let lower = name.to_lowercase();
    SENSITIVE_NAMES.iter().any(|word| lower.contains(word))
}

/// Control characters would let a crafted path forge extra log lines in some viewers.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c <= '\u{1f}' || c == '\u{7f}' { '?' } else { c })
        .collect()
}

/// `text.slice(0, limit) + "..."` when longer than `limit` UTF-16 units, as JavaScript counts.
fn clip(text: String, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        return text;
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        units += c.len_utf16();
        if units > limit {
            break;
        }
        out.push(c);
    }
    out + "..."
}

pub fn redact_target(raw_url: &str) -> String {
    let (path, query) = match raw_url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (raw_url, None),
    };
    let mut out = redact_encoded(path, false);
    if let Some(query) = query {
        let pairs: Vec<String> = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                None => redact_encoded(pair, true),
                Some((name, _)) if is_sensitive(&safe_decode(name, true)) => {
                    format!("{}=<redacted>", redact_encoded(name, true))
                }
                Some((name, value)) => format!("{}={}", redact_encoded(name, true), redact_encoded(value, true)),
            })
            .collect();
        out.push('?');
        out.push_str(&pairs.join("&"));
    }
    clip(printable(&out), MAX_TARGET)
}

/// User agents are logged for abuse reports; trimmed and scrubbed the same way.
pub fn redact_user_agent(value: Option<&str>) -> Option<String> {
    value.map(|v| clip(printable(&redact_text(v)), 160))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up, token-shaped values (never real credentials)
    const FAKE_JWT: &str =
        "eyJhbGciOiJSUzI1NiJ9.eyJ1c2VySWQiOiJVSUQtdGVzdCJ9.c2lnbmF0dXJlLW5vdC1yZWFsLXNpZ25hdHVyZS1ub3QtcmVhbA";

    fn fake_key() -> String {
        format!("RRK_{}", "0123456789abcdef".repeat(3))
    }

    #[test]
    fn removes_a_jwt_from_the_path() {
        assert_eq!(
            redact_target(&format!("/account/api/oauth/sessions/kill/{FAKE_JWT}")),
            "/account/api/oauth/sessions/kill/<token>"
        );
        assert_eq!(redact_text("eyJ eyJa. eyJa.b eyJa.b. x"), "eyJ eyJa. eyJa.b <token> x");
    }

    #[test]
    fn removes_keys_hex_and_long_tokens() {
        assert_eq!(redact_target(&format!("/x/{}/y", fake_key())), "/x/<key>/y");
        assert_eq!(redact_target(&format!("/x/{}", "ab".repeat(24))), "/x/<redacted>");
        assert_eq!(redact_target(&format!("/x/{}", "Az_-".repeat(20))), "/x/<redacted>");
        let uid = "/progression/UID-00000000-0000-4000-8000-00000000f609";
        assert_eq!(redact_target(uid), uid);
    }

    #[test]
    fn query_parameters_by_name_and_shape() {
        assert_eq!(
            redact_target("/a?accountId=UID-1&key=abc&access_token=zzz&Code=1234"),
            "/a?accountId=UID-1&key=<redacted>&access_token=<redacted>&Code=<redacted>"
        );
        assert_eq!(
            redact_target(&format!("/a?x={FAKE_JWT}&Username=Slayer")),
            "/a?x=<token>&Username=Slayer"
        );
        assert_eq!(redact_target("/a?flag&b=%zz"), "/a?flag&b=%zz");
    }

    #[test]
    fn encoded_tokens_and_control_characters() {
        let encoded = FAKE_JWT.replace('.', "%2E");
        assert!(!redact_target(&format!("/k/{encoded}")).contains("eyJ"));
        assert!(!redact_target(&format!("/q?x={encoded}")).contains("eyJ"));
        assert_eq!(redact_target("/a\r\nforged"), "/a??forged");
        assert_eq!(redact_target(&format!("/{}", "a.".repeat(500))).len(), 403);
        assert_eq!(
            redact_user_agent(Some(&format!("UE4 {FAKE_JWT}"))).as_deref(),
            Some("UE4 <token>")
        );
        assert_eq!(redact_user_agent(None), None);
    }
}
