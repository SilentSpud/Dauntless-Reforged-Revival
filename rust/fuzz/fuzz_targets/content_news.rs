//! The news document the content server passes on to every launcher (MIGRATION_PLAN.md RS2, RX3;
//! docs/security/content.md), from the host's file or fetched from GitHub. Whatever the bytes: no
//! panic, at most 50 items, newest first, each with a date the server wrote itself, a title of 1 to
//! 200 UTF-16 units without control characters, and a body of at most 10 000 units with none but
//! tabs and newlines.
#![no_main]

use content::input::parse_news;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    service_kit::linelog::set_level(service_kit::linelog::Level::Error);
    let text = String::from_utf8_lossy(data);
    let Ok(raw) = service_kit::jsjson::parse(&text) else {
        return;
    };
    let Ok(items) = parse_news(&raw) else {
        return;
    };
    assert!(items.len() <= 50);
    for pair in items.windows(2) {
        assert!(pair[0].date >= pair[1].date);
    }
    for item in &items {
        // The shape the server wrote (toISOString's, apart from how a year past 9999 is written)
        assert!(item.date.ends_with('Z') && item.date.contains('T'), "{}", item.date);
        let title_units: usize = item.title.chars().map(char::len_utf16).sum();
        assert!((1..=200).contains(&title_units));
        assert!(!item.title.chars().any(|c| (c as u32) < 0x20 || c == '\x7f'));
        assert!(item.body.chars().map(char::len_utf16).sum::<usize>() <= 10_000);
        assert!(
            !item
                .body
                .chars()
                .any(|c| matches!(c as u32, 0x00..=0x08 | 0x0b..=0x1f | 0x7f) || c == '\r')
        );
    }
});
