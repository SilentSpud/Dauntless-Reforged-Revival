//! `String.prototype.localeCompare` as Node's ICU answers it with the default locale (MIGRATION_PLAN.md
//! SB1): the Node services sort names and ids with it, and the replies list them in that order.
//!
//! ICU's root collation, which byte order is not: whitespace, then punctuation, then symbols, then
//! digits, then letters (so `_` sorts before `0` and both before `a`), letters compared without case
//! first, and only when everything else is equal does lower case come before upper case. The table
//! below is the order Node gives the printable ASCII characters (checked in the tests); controls are
//! ignored, as ICU ignores them. Characters outside ASCII sort after `z` by code point, which ICU
//! doesn't do; the Node services only compare ASCII ids and names this way.

use std::cmp::Ordering;

/// Printable ASCII in ICU's primary order; letters stand for both cases.
const ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789abcdefghijklmnopqrstuvwxyz";

fn primary(c: char) -> Option<u32> {
    if c.is_ascii_control() {
        return None;
    }
    let folded = c.to_ascii_lowercase();
    match ORDER.find(folded) {
        Some(index) => Some(index as u32),
        None => Some(ORDER.len() as u32 + c as u32),
    }
}

/// `a.localeCompare(b)` as an ordering.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let primaries = |s: &str| s.chars().filter_map(primary).collect::<Vec<_>>();
    match primaries(a).cmp(&primaries(b)) {
        Ordering::Equal => {}
        other => return other,
    }
    // Tertiary level: at the first difference in case, lower case first.
    let cases = |s: &str| {
        s.chars()
            .filter(|c| !c.is_ascii_control())
            .map(|c| c.is_ascii_uppercase())
            .collect::<Vec<_>>()
    };
    cases(a).cmp(&cases(b))
}

#[cfg(test)]
mod tests {
    use super::locale_compare;

    // The order Node sorts these in with (a, b) => a.localeCompare(b).
    #[test]
    fn sorts_like_node() {
        let node = [
            "",
            "a",
            "a_1",
            "a-1",
            "a,b",
            "a!b",
            "a.b",
            "a(b",
            "a1",
            "ab",
            "aB",
            "Ab",
            "AB",
            "b",
            "B",
            "f3b2c1d0-1111-4aaa-8bbb-00000000000",
            "f3b2c1d0-1111-4aaa-8bbb-000000000001",
            "F3B2C1D0-1111-4aaa-8bbb-000000000001",
            "season a",
            "Season_A",
            "season_free_any",
            "season_premium_any",
            "season-a",
            "season09b_premium",
            "seasona",
            "UID-a",
            "UID-B",
            "uid-c",
            "x~",
            "x$",
            "x9",
        ];
        let mut shuffled: Vec<&str> = node.iter().rev().copied().collect();
        shuffled.rotate_left(7);
        shuffled.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(shuffled, node);
    }
}
