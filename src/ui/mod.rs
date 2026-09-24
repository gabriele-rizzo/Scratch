mod theme;
pub use theme::*;

mod input;
pub use input::*;

mod command_bar;
pub use command_bar::*;

mod toast;
pub use toast::*;

use std::time::{Duration, Instant};

use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// Spinner frame for an animation that began at `since`.
pub fn spinner(since: Instant) -> &'static str {
    let frame = since.elapsed().as_millis() / SPINNER_INTERVAL.as_millis();
    SPINNER[frame as usize % SPINNER.len()]
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
