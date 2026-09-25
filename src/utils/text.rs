/// Terminals send line breaks in pastes as `\r` (or `\r\n`); turns them into `\n`.
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The first line of `text`, for single-line inputs.
pub fn first_line(text: &str) -> &str {
    text.split(['\n', '\r']).next().unwrap_or_default()
}

/// Standard base64 with padding, for OSC 52 clipboard writes.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|index| chunk.get(index).copied().unwrap_or(0));
        let group = u32::from(a) << 16 | u32::from(b) << 8 | u32::from(c);

        for index in 0..4 {
            if index <= chunk.len() {
                let sextet = (group >> (18 - 6 * index)) & 0x3f;
                encoded.push(ALPHABET[sextet as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"Hello, world!\n"), "SGVsbG8sIHdvcmxkIQo=");
    }

    #[test]
    fn normalizes_every_line_break_style() {
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn first_line_stops_at_any_line_break() {
        assert_eq!(first_line("run\nsave"), "run");
        assert_eq!(first_line("run\r\nsave"), "run");
        assert_eq!(first_line("run"), "run");
        assert_eq!(first_line(""), "");
    }
}
