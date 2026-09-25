mod theme;
pub use theme::*;

mod input;
pub use input::*;

mod command_bar;
pub use command_bar::*;

mod toast;
pub use toast::*;

mod scroll;
pub use scroll::*;

mod confirm;
pub use confirm::*;

mod wrap;
pub use wrap::*;

mod help;
pub use help::*;

use std::time::{Duration, Instant};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// Spinner frame for an animation that began at `since`.
pub fn spinner(since: Instant) -> &'static str {
    let frame = since.elapsed().as_millis() / SPINNER_INTERVAL.as_millis();
    SPINNER[frame as usize % SPINNER.len()]
}

/// A slim scrollbar down the right edge of `area` (usually just inside a panel's
/// right border), drawn only when `total` rows don't fit in the `visible` ones.
pub fn scrollbar(
    frame: &mut Frame,
    area: Rect,
    total: usize,
    visible: usize,
    offset: usize,
    thumb: Color,
) {
    if total <= visible || area.width == 0 || area.height == 0 {
        return;
    }

    let (start, length) = thumb_span(usize::from(area.height), total, visible, offset);
    let x = area.right() - 1;

    for row in 0..area.height {
        let on_thumb = (start..start + length).contains(&usize::from(row));
        let (symbol, color) = if on_thumb {
            ("┃", thumb)
        } else {
            ("│", MUTED)
        };
        frame.buffer_mut()[(x, area.y + row)]
            .set_symbol(symbol)
            .set_style(Style::new().fg(color));
    }
}

/// Where a scrollbar's thumb goes on a `track` rows tall: its first row and its
/// length. The thumb only touches the ends when the view does, so it never looks
/// finished while there's still something to scroll.
fn thumb_span(track: usize, total: usize, visible: usize, offset: usize) -> (usize, usize) {
    let length = (visible * track + total / 2) / total;
    let length = length.clamp(1, track);
    let max_start = track - length;
    let max_offset = total - visible;
    let offset = offset.min(max_offset);

    let start = if offset == 0 {
        0
    } else if offset == max_offset {
        max_start
    } else {
        // In between, keep off both ends when there's room to.
        let start = (offset * max_start + max_offset / 2) / max_offset;
        let first = 1.min(max_start);
        let last = max_start.saturating_sub(1).max(first);
        start.clamp(first, last)
    };

    (start, length)
}

/// Keyboard hints like `esc commands · ctrl+c quit`.
pub fn hints(pairs: &[(&'static str, &'static str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    for (index, (key, action)) in pairs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(MUTED)));
        }

        spans.push(Span::styled(
            *key,
            Style::new().fg(SUBTLE).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {action}"), Style::new().fg(MUTED)));
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_touches_the_ends_only_at_the_ends() {
        for track in 3..30 {
            for visible in 1..40 {
                for total in visible + 1..80 {
                    let max_offset = total - visible;
                    let (_, length) = thumb_span(track, total, visible, 0);
                    let max_start = track - length;

                    assert_eq!(thumb_span(track, total, visible, 0).0, 0);
                    assert_eq!(thumb_span(track, total, visible, max_offset).0, max_start);

                    let mut previous = 0;
                    for offset in 1..max_offset {
                        let (start, _) = thumb_span(track, total, visible, offset);
                        let case = format!(
                            "track {track}, total {total}, visible {visible}, offset {offset}"
                        );

                        // Never at the bottom (or top) while there's more to scroll.
                        if max_start >= 2 {
                            assert!(start > 0 && start < max_start, "{case}");
                        }
                        assert!(start >= previous, "moved backwards: {case}");
                        previous = start;
                    }
                }
            }
        }
    }

    #[test]
    fn thumb_size_is_proportional() {
        assert_eq!(thumb_span(10, 20, 10, 0), (0, 5));
        assert_eq!(thumb_span(10, 100, 10, 0).1, 1);
        // One step from the end isn't the end.
        assert_eq!(thumb_span(10, 20, 10, 9), (4, 5));
        assert_eq!(thumb_span(10, 20, 10, 10), (5, 5));
    }
}
