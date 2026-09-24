use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};

use super::{ACCENT, MUTED, SUBTLE, TEXT};

pub enum UnsavedChoice {
    Save,
    Discard,
    Cancel,
}

/// Modal asking what to do with unsaved changes before leaving the editor.
pub struct UnsavedPrompt {
    /// The file being left, e.g. `scratch.rs`.
    pub file: String,
}

impl UnsavedPrompt {
    pub fn handle(&self, key: KeyEvent) -> Option<UnsavedChoice> {
        match key.code {
            KeyCode::Char('s' | 'S') | KeyCode::Enter => Some(UnsavedChoice::Save),
            KeyCode::Char('d' | 'D') => Some(UnsavedChoice::Discard),
            KeyCode::Esc | KeyCode::Char('c' | 'C' | 'n' | 'N') => Some(UnsavedChoice::Cancel),
            _ => None,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(4).min(56);
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(7)])
            .flex(Flex::Center)
            .areas(area);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .padding(Padding::new(2, 2, 1, 1))
            .title(Span::styled(
                " Unsaved changes ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));

        let key = |key: &'static str| {
            Span::styled(key, Style::new().fg(TEXT).add_modifier(Modifier::BOLD))
        };
        let action = |action: &'static str| Span::styled(action, Style::new().fg(SUBTLE));
        let gap = || Span::styled("   ", Style::new().fg(MUTED));

        let body = vec![
            Line::from(vec![
                Span::styled("Save changes to ", Style::new().fg(TEXT)),
                Span::styled(
                    self.file.as_str(),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" before leaving?", Style::new().fg(TEXT)),
            ]),
            Line::default(),
            Line::from(vec![
                key("s"),
                action(" save"),
                gap(),
                key("d"),
                action(" discard"),
                gap(),
                key("esc"),
                action(" cancel"),
            ]),
        ];

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
            area,
        );
    }
}
