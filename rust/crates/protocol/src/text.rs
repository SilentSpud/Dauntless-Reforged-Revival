//! JavaScript's string rules, for the ports that must answer exactly as the TypeScript did
//! (`legacy/ReforgedLauncher/src/shared/text.ts` and the built-ins it leans on).

/// ECMAScript white space and line terminators: what `trim()` removes and `\s` matches. Not Rust's
/// `char::is_whitespace`, which adds U+0085 and leaves out U+FEFF.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
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

/// Runs of JavaScript white space made one space: `.replace(/\s+/g, " ")`.
pub(crate) fn collapse_spaces(text: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    let mut in_space = false;
    for c in text {
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
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
        assert_eq!(collapse_spaces(" a \t\n b ".chars()), " a b ");
    }
}
