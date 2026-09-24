/// Terminals send line breaks in pastes as `\r` (or `\r\n`); turns them into `\n`.
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The first line of `text`, for single-line inputs.
pub fn first_line(text: &str) -> &str {
    text.split(['\n', '\r']).next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
