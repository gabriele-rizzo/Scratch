use std::time::{Duration, Instant};

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{ERROR, SUCCESS, TEXT};

const TOAST_DURATION: Duration = Duration::from_secs(3);
const ERROR_TOAST_DURATION: Duration = Duration::from_secs(6);

#[derive(Clone, Copy)]
pub enum ToastKind {
    Success,
    Error,
}

/// Short-lived status message.
pub struct Toast {
    kind: ToastKind,
    text: String,
    shown: Instant,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            shown: Instant::now(),
        }
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self::new(ToastKind::Success, text)
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::new(ToastKind::Error, text)
    }

    pub fn is_visible(&self) -> bool {
        let duration = match self.kind {
            ToastKind::Error => ERROR_TOAST_DURATION,
            ToastKind::Success => TOAST_DURATION,
        };

        self.shown.elapsed() < duration
    }

    pub fn line(&self) -> Line<'_> {
        let (icon, color) = match self.kind {
            ToastKind::Success => ("✓", SUCCESS),
            ToastKind::Error => ("✗", ERROR),
        };

        Line::from(vec![
            Span::styled(icon, Style::new().fg(color).bold()),
            Span::styled(format!(" {}", self.text), Style::new().fg(TEXT)),
        ])
    }
}
