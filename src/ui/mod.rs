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
    symbols::scrollbar,
    text::Span,
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState},
};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// Spinner frame for an animation that began at `since`.
pub fn spinner(since: Instant) -> &'static str {
    let frame = since.elapsed().as_millis() / SPINNER_INTERVAL.as_millis();
    SPINNER[frame as usize % SPINNER.len()]
}

/// A slim scrollbar down `area` (usually just inside a panel's right border),
/// drawn only when `total` rows don't fit in the `visible` ones.
pub fn scrollbar(
    frame: &mut Frame,
    area: Rect,
    total: usize,
    visible: usize,
    offset: usize,
    thumb: Color,
) {
    if total <= visible {
        return;
    }

    let mut state = ScrollbarState::new(total - visible)
        .position(offset)
        .viewport_content_length(visible);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(scrollbar::Set {
            track: "│",
            thumb: "┃",
            begin: "",
            end: "",
        })
        .begin_symbol(None)
        .end_symbol(None)
        .track_style(Style::new().fg(MUTED))
        .thumb_style(Style::new().fg(thumb));

    frame.render_stateful_widget(bar, area, &mut state);
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
