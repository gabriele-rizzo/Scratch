use crossterm::event::{Event, KeyCode, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};

use super::{ACCENT, CommandSpec, SUBTLE, TEXT};

/// Keys (or a command) and what they do.
pub type HelpEntry = (String, String);

pub struct HelpSection {
    pub title: &'static str,
    pub entries: Vec<HelpEntry>,
}

impl HelpSection {
    pub fn new(title: &'static str, entries: &[(&str, &str)]) -> Self {
        Self {
            title,
            entries: entries
                .iter()
                .map(|(keys, action)| (keys.to_string(), action.to_string()))
                .collect(),
        }
    }

    /// A section listing `commands`, so the help always matches what exists.
    pub fn commands<T>(commands: &[CommandSpec<T>]) -> Self {
        let entries = commands
            .iter()
            .map(|command| {
                let usage = format!("{} {}", command.name, command.args);
                let action = match command.keys {
                    "" => command.description.to_string(),
                    keys => format!("{} ({keys})", command.description),
                };
                (usage.trim_end().to_string(), action)
            })
            .collect();

        Self {
            title: "Commands",
            entries,
        }
    }
}

/// A scrollable overlay listing keys and commands.
pub struct Help {
    sections: Vec<HelpSection>,
    scroll: usize,
    /// Rows visible when last drawn, for paging.
    viewport: usize,
}

impl Help {
    pub fn new(sections: Vec<HelpSection>) -> Self {
        Self {
            sections,
            scroll: 0,
            viewport: 0,
        }
    }

    /// Scrolls, or returns `true` when the overlay should close.
    pub fn handle(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) => return true,
                KeyCode::Char('q' | '?') => return true,
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.viewport.max(1)),
                KeyCode::PageDown => self.scroll += self.viewport.max(1),
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = usize::MAX,
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => self.scroll += 3,
                _ => {}
            },
            _ => {}
        }
        false
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let key_width = self
            .sections
            .iter()
            .flat_map(|section| &section.entries)
            .map(|(keys, _)| keys.chars().count())
            .max()
            .unwrap_or(0)
            .min(24);

        let mut lines = Vec::new();
        for (index, section) in self.sections.iter().enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::styled(
                section.title,
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));

            for (keys, action) in &section.entries {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {keys:key_width$}  "),
                        Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(action.clone(), Style::new().fg(SUBTLE)),
                ]));
            }
        }
        lines
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let lines = self.lines();

        let width = area.width.saturating_sub(4).min(88);
        let height = (lines.len() as u16 + 4).min(area.height.saturating_sub(2));
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .padding(Padding::new(2, 2, 1, 1))
            .title(Span::styled(
                " Help ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom({
                let mut hints = super::hints(&[("esc", "close"), ("↑↓", "scroll")]);
                hints.push(Span::raw(" "));
                Line::from(hints).right_aligned()
            });

        let inner = block.inner(area);
        self.viewport = inner.height as usize;
        let max = lines.len().saturating_sub(self.viewport);
        self.scroll = self.scroll.min(max);

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines.clone())
                .block(block)
                .scroll((self.scroll as u16, 0)),
            area,
        );

        super::scrollbar(
            frame,
            area.inner(Margin::new(0, 1)),
            lines.len(),
            self.viewport,
            self.scroll,
            ACCENT,
        );
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use tuimon::ScreenAction;

    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn lists_commands_with_their_usage_and_keys() {
        fn nothing(_: &mut (), _: &str) -> ScreenAction {
            ScreenAction::None
        }
        const COMMANDS: &[CommandSpec<()>] = &[
            CommandSpec {
                name: "run",
                args: "[args]",
                description: "Run the file",
                keys: "ctrl+r",
                paths: false,
                run: nothing,
            },
            CommandSpec {
                name: "back",
                args: "",
                description: "Go back",
                keys: "",
                paths: false,
                run: nothing,
            },
        ];

        let section = HelpSection::commands(COMMANDS);
        assert_eq!(
            section.entries,
            [
                (
                    "run [args]".to_string(),
                    "Run the file (ctrl+r)".to_string()
                ),
                ("back".to_string(), "Go back".to_string()),
            ]
        );
    }

    #[test]
    fn closes_on_the_usual_keys_and_scrolls_otherwise() {
        let mut help = Help::new(vec![HelpSection::new("Keys", &[("a", "b")])]);

        for code in [
            KeyCode::Esc,
            KeyCode::Char('q'),
            KeyCode::Char('?'),
            KeyCode::F(1),
        ] {
            assert!(help.handle(&key(code)));
        }
        assert!(!help.handle(&key(KeyCode::Down)));
        assert_eq!(help.scroll, 1);
    }
}
