/// Truncate a byte slice to at most `max_bytes`, landing on a valid UTF-8
/// character boundary, then convert to `String` (lossy).
///
/// If the cut lands in the middle of a multi-byte character the incomplete
/// leading bytes are skipped so no replacement character (U+FFFD) appears at
/// the start.
pub fn tail_str_lossy(data: &[u8], max_bytes: usize) -> String {
    let mut start = data.len().saturating_sub(max_bytes);
    while start < data.len() && is_utf8_continuation(data[start]) {
        start += 1;
    }
    String::from_utf8_lossy(&data[start..]).to_string()
}

/// Truncate a `&str` to at most `max_bytes` from the front, snapping to a
/// valid UTF-8 character boundary so the result is always valid.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && is_utf8_continuation(s.as_bytes()[end]) {
        end -= 1;
    }
    &s[..end]
}

#[inline]
fn is_utf8_continuation(b: u8) -> bool {
    b >= 0x80 && b < 0xC0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_ascii() {
        let data = b"hello world";
        assert_eq!(tail_str_lossy(data, 5), "world");
    }

    #[test]
    fn tail_utf8_boundary() {
        let data = "你好".as_bytes();
        assert_eq!(tail_str_lossy(data, 4), "好");
        assert_eq!(tail_str_lossy(data, 3), "好");
        assert_eq!(tail_str_lossy(data, 100), "你好");
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_str("hello", 3), "hel");
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_utf8_boundary() {
        let s = "你好世界";
        assert_eq!(truncate_str(s, 7), "你好");
        assert_eq!(truncate_str(s, 6), "你好");
        assert_eq!(truncate_str(s, 1), "");
    }
}
