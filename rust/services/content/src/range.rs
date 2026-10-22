//! Single-range support for resumable downloads (`src/range.ts`, RFC 9110 section 14):
//!
//! - no Range header: the whole file (200)
//! - bytes=START-END: START..END, END clamped to the last byte (206)
//! - bytes=START-: START..last byte (206)
//! - bytes=-N: the last N bytes (206)
//! - anything else: 416 with "Content-Range: bytes */SIZE" (several ranges, other units, START past
//!   the end, END before START, bytes=-0, a zero-byte file, or text that is not a range at all)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

/// `ParseRange`.
pub fn parse_range(header: Option<&str>, size: u64) -> Range {
    let Some(header) = header else {
        return Range::Full;
    };
    let Some(spec) = service_kit::js::trim(header).strip_prefix("bytes=") else {
        return Range::Unsatisfiable;
    };
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return Range::Unsatisfiable;
    };
    let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
    if !digits(start_text) || !digits(end_text) {
        return Range::Unsatisfiable;
    }
    // 15 digits is already far past any file served; longer is garbage
    if start_text.len() > 15 || end_text.len() > 15 || (start_text.is_empty() && end_text.is_empty()) || size == 0 {
        return Range::Unsatisfiable;
    }
    let number = |t: &str| t.parse::<u64>().unwrap_or(u64::MAX);
    if start_text.is_empty() {
        let suffix = number(end_text);
        if suffix == 0 {
            return Range::Unsatisfiable;
        }
        return Range::Partial {
            start: size.saturating_sub(suffix),
            end: size - 1,
        };
    }
    let start = number(start_text);
    if start >= size {
        return Range::Unsatisfiable;
    }
    if end_text.is_empty() {
        return Range::Partial { start, end: size - 1 };
    }
    let end = number(end_text);
    if end < start {
        return Range::Unsatisfiable;
    }
    Range::Partial {
        start,
        end: end.min(size - 1),
    }
}

/// `IfRangeAllowsPartial`: resume only if the file is still the one the client started on.
pub fn if_range_allows_partial(if_range: Option<&str>, etag: &str) -> bool {
    if_range.is_none_or(|v| service_kit::js::trim(v) == etag)
}

/// `IfNoneMatchHits`: "*" or a list of tags, compared weakly (W/"x" matches "x").
pub fn if_none_match_hits(header: Option<&str>, etag: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    let trimmed = service_kit::js::trim(header);
    trimmed == "*"
        || trimmed.split(',').any(|tag| {
            let tag = service_kit::js::trim(tag);
            tag.strip_prefix("W/").unwrap_or(tag) == etag
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_parse_like_node() {
        let p = |start, end| Range::Partial { start, end };
        assert_eq!(parse_range(None, 100), Range::Full);
        assert_eq!(parse_range(Some("bytes=0-0"), 100), p(0, 0));
        assert_eq!(parse_range(Some("bytes=0-1048575"), 103_673_520), p(0, 1_048_575));
        assert_eq!(parse_range(Some("bytes=10-"), 100), p(10, 99));
        assert_eq!(parse_range(Some("bytes=99-"), 100), p(99, 99));
        assert_eq!(parse_range(Some("bytes=-10"), 100), p(90, 99));
        assert_eq!(parse_range(Some("bytes=-1000"), 100), p(0, 99));
        assert_eq!(parse_range(Some("bytes=50-5000"), 100), p(50, 99));
        assert_eq!(parse_range(Some(" bytes=1-2 "), 100), p(1, 2));
        for header in [
            "bytes=100-",
            "bytes=100-200",
            "bytes=5-4",
            "bytes=-0",
            "bytes=-",
            "bytes=",
            "bytes=a-b",
            "bytes=0-1,5-6",
            "items=0-1",
            "bytes 0-1",
            "bytes=-1-2",
            "bytes=1.5-2",
            "bytes=0x10-",
            "bytes=9999999999999999-",
            "",
            "garbage",
        ] {
            assert_eq!(parse_range(Some(header), 100), Range::Unsatisfiable, "{header}");
        }
        assert_eq!(parse_range(Some("bytes=0-"), 0), Range::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-1"), 0), Range::Unsatisfiable);
        assert_eq!(parse_range(None, 0), Range::Full);
    }

    #[test]
    fn conditional_headers() {
        let tag = "\"abc\"";
        assert!(if_range_allows_partial(None, tag));
        assert!(if_range_allows_partial(Some("\"abc\""), tag));
        assert!(!if_range_allows_partial(Some("\"def\""), tag));
        assert!(!if_range_allows_partial(Some("W/\"abc\""), tag));
        assert!(!if_range_allows_partial(Some("Wed, 21 Oct 2015 07:28:00 GMT"), tag));
        assert!(!if_none_match_hits(None, tag));
        assert!(if_none_match_hits(Some("*"), tag));
        assert!(if_none_match_hits(Some("\"x\", \"abc\""), tag));
        assert!(if_none_match_hits(Some("W/\"abc\""), tag));
        assert!(!if_none_match_hits(Some("\"abcd\""), tag));
    }
}
