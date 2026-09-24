use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Splits `line` into rows at most `width` columns wide, breaking at any character
/// like a terminal does, and keeping each span's style.
pub fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line.width() <= width {
        return vec![line.clone()];
    }

    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;

    for span in &line.spans {
        let mut chunk = String::new();

        for c in span.content.chars() {
            let char_width = c.width().unwrap_or(0);

            if used + char_width > width && used > 0 {
                if !chunk.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut chunk), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut row)).style(line.style));
                used = 0;
            }

            chunk.push(c);
            used += char_width;
        }

        if !chunk.is_empty() {
            row.push(Span::styled(chunk, span.style));
        }
    }

    if !row.is_empty() {
        rows.push(Line::from(row).style(line.style));
    }

    rows
}

/// How many rows `wrap` splits `line` into, without allocating.
pub fn row_count(line: &Line, width: usize) -> usize {
    if width == 0 || line.width() <= width {
        return 1;
    }

    let mut rows = 1;
    let mut used = 0;

    for c in line.spans.iter().flat_map(|span| span.content.chars()) {
        let char_width = c.width().unwrap_or(0);

        if used + char_width > width && used > 0 {
            rows += 1;
            used = 0;
        }

        used += char_width;
    }

    rows
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Style};

    use super::*;

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn texts(rows: &[Line]) -> Vec<String> {
        rows.iter().map(text).collect()
    }

    #[test]
    fn short_lines_stay_whole() {
        let line = Line::from("hello");
        assert_eq!(texts(&wrap(&line, 10)), ["hello"]);
        assert_eq!(row_count(&line, 10), 1);
    }

    #[test]
    fn breaks_at_the_width_like_a_terminal() {
        let line = Line::from("abcdefghij");
        assert_eq!(texts(&wrap(&line, 4)), ["abcd", "efgh", "ij"]);
        assert_eq!(row_count(&line, 4), 3);
    }

    #[test]
    fn keeps_span_styles_across_breaks() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![Span::raw("ab"), Span::styled("cdef", red)]);
        let rows = wrap(&line, 3);

        assert_eq!(texts(&rows), ["abc", "def"]);
        assert_eq!(rows[0].spans[1].style, red);
        assert_eq!(rows[1].spans[0].style, red);
    }

    #[test]
    fn keeps_the_line_style() {
        let style = Style::new().fg(Color::Green);
        let rows = wrap(&Line::styled("abcdef", style), 3);
        assert!(rows.iter().all(|row| row.style == style));
    }

    #[test]
    fn never_splits_a_wide_character() {
        // Each of these takes 2 columns, so only one fits in 3.
        let line = Line::from("日本語");
        assert_eq!(texts(&wrap(&line, 3)), ["日", "本", "語"]);
        assert_eq!(row_count(&line, 3), 3);
    }

    #[test]
    fn row_count_matches_wrap() {
        let lines = [
            Line::from(""),
            Line::from("x".repeat(80)),
            Line::from(vec![Span::raw("a日b"), Span::raw("本c語d")]),
        ];

        for line in &lines {
            for width in 1..12 {
                assert_eq!(
                    row_count(line, width),
                    wrap(line, width).len(),
                    "width {width}"
                );
            }
        }
    }
}
