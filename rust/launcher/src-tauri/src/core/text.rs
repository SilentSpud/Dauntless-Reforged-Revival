//! JavaScript's string rules, for the ports that must answer exactly as the TypeScript did
//! (`src/shared/text.ts` and the built-ins it leans on).

/// ECMAScript white space and line terminators: what `trim()` removes and `\s` matches. Not Rust's
/// `char::is_whitespace`, which adds U+0085 and leaves out U+FEFF.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// `String.prototype.trim`.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `String.prototype.length`: UTF-16 code units.
pub fn js_length(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `UNSAFE_CHAR`: C0 and C1 control characters, and the bidirectional overrides and isolates that
/// can make one text look like another.
pub fn is_unsafe_char(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// `Number(text)` for a string: trimmed, empty is 0, decimal with an optional exponent, `0x`/`0o`/`0b`
/// integers, `Infinity`; anything else is NaN.
pub fn js_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let radix = |prefixes: [&str; 2], base: u32| {
        prefixes
            .iter()
            .find_map(|prefix| text.strip_prefix(prefix))
            .map(|digits| {
                if digits.is_empty() {
                    return f64::NAN;
                }
                digits
                    .chars()
                    .try_fold(0.0f64, |n, c| {
                        c.to_digit(base).map(|d| n * f64::from(base) + f64::from(d))
                    })
                    .unwrap_or(f64::NAN)
            })
    };
    if let Some(n) = radix(["0x", "0X"], 16)
        .or_else(|| radix(["0o", "0O"], 8))
        .or_else(|| radix(["0b", "0B"], 2))
    {
        return n;
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Rust would also read "inf", "nan" and the like; JavaScript only digits, a point and an exponent
    let shaped = unsigned
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    if !shaped || !unsigned.bytes().next().is_some_and(|b| b.is_ascii_digit() || b == b'.') {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_like_javascript() {
        assert_eq!(js_trim("\u{feff}\u{a0} x \u{3000}\u{2028}"), "x");
        assert_eq!(js_trim("\u{85}x"), "\u{85}x");
        assert_eq!(js_length("a\u{1f600}é"), 4);
        assert!(is_unsafe_char('\u{85}') && is_unsafe_char('\u{202e}') && !is_unsafe_char('\u{200b}'));
    }

    #[test]
    fn numbers_like_javascript() {
        for (text, n) in [
            ("", 0.0),
            (" 720 ", 720.0),
            ("1e3", 1000.0),
            ("0x10", 16.0),
            (".5", 0.5),
            ("-2", -2.0),
            ("+3", 3.0),
            ("5.", 5.0),
        ] {
            assert_eq!(js_number(text), n, "{text:?}");
        }
        assert_eq!(js_number("-Infinity"), f64::NEG_INFINITY);
        for text in ["inf", "NaN", "1,5", "0x", "abc", "1 2", "e5"] {
            assert!(js_number(text).is_nan(), "{text:?}");
        }
    }
}
