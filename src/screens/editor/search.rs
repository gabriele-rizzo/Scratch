//! Finding text in the output panel.

use ratatui::{
    style::Color,
    text::{Line, Span},
};

use crate::ui::{ACCENT, Input, ON_ACCENT, TEXT};

/// Background for matches other than the current one.
const MATCH: Color = Color::Rgb(0x4a, 0x3f, 0x2a);

/// Where a match is: an output line, by its position counted from the very first
/// line (so it stays put as old lines are dropped), and a range of characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub struct Search {
    pub input: Input,
    /// In output order.
    pub matches: Vec<Match>,
    pub current: usize,
    /// Whether the view should move to the current match on the next draw.
    pub jump: bool,
}

impl Search {
    /// Matches of the query in `text`, which is line number `line`.
    pub fn find(&self, line: usize, text: &str) -> Vec<Match> {
        find(self.input.value(), text)
            .into_iter()
            .map(|(start, end)| Match { line, start, end })
            .collect()
    }

    /// Moves to the next (or previous) match, wrapping around.
    pub fn step(&mut self, forward: bool) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }

        self.current = if forward {
            (self.current + 1) % count
        } else {
            (self.current + count - 1) % count
        };
        self.jump = true;
    }

    /// Forgets matches in lines before `first`, which were dropped.
    pub fn forget_before(&mut self, first: usize) {
        let dropped = self.matches.partition_point(|m| m.line < first);
        self.matches.drain(..dropped);
        self.current = self.current.saturating_sub(dropped);
    }

    /// The matches in line `line`.
    pub fn in_line(&self, line: usize) -> &[Match] {
        let start = self.matches.partition_point(|m| m.line < line);
        let end = self.matches.partition_point(|m| m.line <= line);
        &self.matches[start..end]
    }

    pub fn current(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }

    /// "3 of 17", "no matches", or nothing before anything is typed.
    pub fn counter(&self) -> String {
        match (self.input.is_empty(), self.matches.len()) {
            (true, _) => String::new(),
            (false, 0) => "no matches".to_string(),
            (false, count) => format!("{} of {count}", self.current + 1),
        }
    }
}

/// Character ranges of `query` in `text`: case-insensitive unless the query has
/// capitals.
pub fn find(query: &str, text: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }

    let case_sensitive = query.chars().any(char::is_uppercase);
    let (haystack, needle) = if case_sensitive {
        (text.to_string(), query.to_string())
    } else {
        (text.to_lowercase(), query.to_lowercase())
    };

    // Lowercasing can change lengths for a few characters; match exactly then, so
    // positions stay right.
    let (haystack, needle) = if haystack.chars().count() == text.chars().count() {
        (haystack, needle)
    } else {
        (text.to_string(), query.to_string())
    };

    let length = needle.chars().count();
    haystack
        .match_indices(&needle)
        .map(|(byte, _)| {
            let start = haystack[..byte].chars().count();
            (start, start + length)
        })
        .collect()
}

/// `line` with `matches` highlighted, the current one (if among them) standing out.
pub fn highlight(line: &Line<'static>, matches: &[Match], current: Option<Match>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut position = 0;

    for span in &line.spans {
        for c in span.content.chars() {
            let style = match matches
                .iter()
                .find(|m| m.start <= position && position < m.end)
            {
                Some(m) if Some(*m) == current => span.style.fg(ON_ACCENT).bg(ACCENT),
                Some(_) => span.style.fg(TEXT).bg(MATCH),
                None => span.style,
            };

            match spans.last_mut() {
                Some(last) if last.style == style => last.content.to_mut().push(c),
                _ => spans.push(Span::styled(c.to_string(), style)),
            }
            position += 1;
        }
    }

    Line::from(spans).style(line.style)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn finds_every_occurrence_by_character() {
        assert_eq!(find("an", "banana"), [(1, 3), (3, 5)]);
        assert_eq!(find("é", "café é"), [(3, 4), (5, 6)]);
        assert!(find("", "anything").is_empty());
    }

    #[test]
    fn capitals_make_it_case_sensitive() {
        assert_eq!(find("error", "Error error"), [(0, 5), (6, 11)]);
        assert_eq!(find("Error", "Error error"), [(0, 5)]);
    }

    #[test]
    fn stepping_wraps_around() {
        let mut search = Search {
            matches: vec![
                Match {
                    line: 0,
                    start: 0,
                    end: 1,
                },
                Match {
                    line: 1,
                    start: 0,
                    end: 1,
                },
            ],
            ..Search::default()
        };

        search.step(true);
        assert_eq!(search.current, 1);
        search.step(true);
        assert_eq!(search.current, 0);
        search.step(false);
        assert_eq!(search.current, 1);
    }

    #[test]
    fn forgets_dropped_lines_and_keeps_the_current_match() {
        let mut search = Search {
            matches: (0..5)
                .map(|line| Match {
                    line,
                    start: 0,
                    end: 1,
                })
                .collect(),
            current: 3,
            ..Search::default()
        };

        search.forget_before(2);
        assert_eq!(search.matches.len(), 3);
        assert_eq!(search.current().unwrap().line, 3);
        assert_eq!(search.in_line(4).len(), 1);
        assert!(search.in_line(1).is_empty());
    }

    #[test]
    fn highlighting_keeps_the_text_and_marks_the_current_match() {
        let line = Line::from(vec![Span::raw("ab"), Span::raw("cabc")]);
        let matches = [
            Match {
                line: 0,
                start: 0,
                end: 2,
            },
            Match {
                line: 0,
                start: 3,
                end: 5,
            },
        ];
        let highlighted = highlight(&line, &matches, Some(matches[1]));

        assert_eq!(text(&highlighted), "abcabc");
        assert_eq!(highlighted.spans[0].style.bg, Some(MATCH));
        assert_eq!(highlighted.spans[2].style.bg, Some(ACCENT));
        assert_eq!(highlighted.spans[2].content, "ab");
    }
}
