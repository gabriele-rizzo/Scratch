use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};

use super::{ACCENT, MUTED, SUBTLE, TEXT};

/// Single-line text input.
#[derive(Default)]
pub struct Input {
    value: String,
    /// Cursor position in chars.
    cursor: usize,
}

pub struct InputView<'a> {
    pub title: Line<'a>,
    pub prompt: &'a str,
    pub placeholder: &'a str,
    pub enabled: bool,
}

impl Input {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    pub fn set(&mut self, value: &str) {
        self.value = value.to_string();
        self.cursor = self.value.chars().count();
    }

    /// Display width of the text before the cursor.
    pub fn cursor_width(&self) -> u16 {
        let before: String = self.value.chars().take(self.cursor).collect();
        Span::raw(before).width() as u16
    }

    fn byte_index(&self, cursor: usize) -> usize {
        self.value
            .char_indices()
            .nth(cursor)
            .map_or(self.value.len(), |(index, _)| index)
    }

    /// Applies an editing key. Returns whether the value changed.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.value.chars().count();

        match key.code {
            KeyCode::Char('u') if ctrl => {
                let end = self.byte_index(self.cursor);
                self.value.drain(..end);
                self.cursor = 0;
                return end > 0;
            }
            KeyCode::Char('w') if ctrl => {
                let end = self.byte_index(self.cursor);
                let before = self.value[..end].trim_end();
                let start = before.rfind(' ').map_or(0, |index| index + 1);
                self.value.drain(start..end);
                self.cursor = self.value[..start].chars().count();
                return start < end;
            }
            KeyCode::Char(c) if !ctrl => {
                let index = self.byte_index(self.cursor);
                self.value.insert(index, c);
                self.cursor += 1;
                return true;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let index = self.byte_index(self.cursor);
                self.value.remove(index);
                return true;
            }
            KeyCode::Delete if self.cursor < len => {
                let index = self.byte_index(self.cursor);
                self.value.remove(index);
                return true;
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            _ => {}
        }

        false
    }

    /// Draws a rounded input box (3 rows tall) and places the cursor when enabled.
    pub fn render(&self, frame: &mut Frame, area: Rect, view: InputView) {
        let border = if view.enabled { SUBTLE } else { MUTED };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(border))
            .title(view.title);

        let prompt = Span::styled(
            format!(" {} ", view.prompt),
            Style::new()
                .fg(if view.enabled { ACCENT } else { MUTED })
                .bold(),
        );
        let prompt_width = prompt.width() as u16;

        let text = if self.value.is_empty() {
            Span::styled(view.placeholder, Style::new().fg(MUTED).italic())
        } else {
            Span::styled(self.value.as_str(), Style::new().fg(TEXT))
        };

        let inner = block.inner(area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![prompt, text])).block(block),
            area,
        );

        if view.enabled {
            let x = inner.x + prompt_width + self.cursor_width();
            frame.set_cursor_position(Position::new(x.min(inner.right()), inner.y));
        }
    }
}
