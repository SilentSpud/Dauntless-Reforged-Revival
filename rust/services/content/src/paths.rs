//! The raw request target of GET /content/v1/files/<path> as a manifest key (`src/paths.ts`).
//!
//! Nothing clever happens here on purpose: the caller looks the result up in an exact-match map built
//! from the manifest and opens the manifest's own path, so a request can only ever name a file the
//! manifest lists. This refuses the shapes that have no business in a request at all, so they are
//! answered with 400 and logged instead of quietly missing the map.

pub const FILES_PREFIX: &str = "/content/v1/files/";

/// `decodeURIComponent`: every %XX decoded, and the result must be UTF-8.
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encoded "/", "\", NUL and "." ("%2e" is how "../" usually gets smuggled past naive checks).
fn has_encoded_forbidden(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["%2f", "%5c", "%00", "%2e"].iter().any(|e| lower.contains(e))
}

/// Anything below 0x20, DEL, and characters no manifest path holds but that mean something to Windows.
fn has_forbidden_char(text: &str) -> bool {
    text.chars()
        .any(|c| (c as u32) < 0x20 || c == '\x7f' || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
}

/// `ParseFileRequestPath`: the manifest key, or why the target was refused.
pub fn parse_file_request_path(raw_target: &str) -> Result<String, &'static str> {
    let raw_path = raw_target.split('?').next().unwrap_or("");
    let Some(rest) = raw_path.strip_prefix(FILES_PREFIX) else {
        return Err("prefix");
    };
    if rest.is_empty() {
        return Err("empty");
    }
    if rest.encode_utf16().count() > 1024 {
        return Err("too_long");
    }
    if rest.contains('\\') || rest.contains('\0') {
        return Err("raw_forbidden_char");
    }
    if has_encoded_forbidden(rest) {
        return Err("encoded_separator");
    }
    let decoded = decode_uri_component(rest).ok_or("bad_encoding")?;
    if has_forbidden_char(&decoded) {
        return Err("forbidden_char");
    }
    // Still percent signs after one decode means double encoding. No manifest path has one.
    if decoded.contains('%') {
        return Err("double_encoding");
    }
    if decoded.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return Err("bad_segment");
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "/content/v1/files/";

    #[test]
    fn plain_manifest_paths_parse_to_themselves() {
        for good in [
            "Version.txt",
            "Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe",
            "Engine/Content/Internationalization/icudt53l/brkitr/root.res",
            "EasyAntiCheat/Licenses/CCO-1.0.txt",
        ] {
            assert_eq!(parse_file_request_path(&format!("{P}{good}")), Ok(good.to_owned()));
        }
        assert_eq!(
            parse_file_request_path(&format!("{P}Version.txt?x=1")),
            Ok("Version.txt".into())
        );
        assert_eq!(
            parse_file_request_path(&format!("{P}Archon/Binaries/Win64/Dauntless%2DWin64%2DShipping.exe")),
            Ok("Archon/Binaries/Win64/Dauntless-Win64-Shipping.exe".into())
        );
    }

    #[test]
    fn traversal_and_smuggling_shapes_are_refused() {
        for (rest, reason) in [
            ("../../Windows/win.ini", "bad_segment"),
            ("Archon/../../x", "bad_segment"),
            ("./Version.txt", "bad_segment"),
            ("Archon//Version.txt", "bad_segment"),
            ("Archon/", "bad_segment"),
            ("..%2f..%2fWindows/win.ini", "encoded_separator"),
            ("..%2F..%2FWindows/win.ini", "encoded_separator"),
            ("%2e%2e/%2e%2e/x", "encoded_separator"),
            ("%2E%2E/x", "encoded_separator"),
            ("Archon%5c..%5cx", "encoded_separator"),
            ("Version.txt%00.png", "encoded_separator"),
            ("Archon\\..\\x", "raw_forbidden_char"),
            ("Version.txt\0", "raw_forbidden_char"),
            ("%252e%252e/x", "double_encoding"),
            ("%25", "double_encoding"),
            ("C:/Windows/win.ini", "forbidden_char"),
            ("Version.txt::$DATA", "forbidden_char"),
            ("Version.txt%3A%3A$DATA", "forbidden_char"),
            ("a%0ab", "forbidden_char"),
            ("a%7fb", "forbidden_char"),
            ("%E0%A4%A", "bad_encoding"),
            ("", "empty"),
        ] {
            assert_eq!(parse_file_request_path(&format!("{P}{rest}")), Err(reason), "{rest:?}");
        }
        assert!(parse_file_request_path("/content/v1/files").is_err());
        assert!(parse_file_request_path("/content/v1/manifest").is_err());
        assert!(parse_file_request_path("//content/v1/files/Version.txt").is_err());
        assert!(parse_file_request_path(&format!("{P}{}b", "a/".repeat(600))).is_err());
    }
}
