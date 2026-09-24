use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState},
};

use super::{ACCENT, Input, InputView, MUTED, SUBTLE, SURFACE, TEXT};

const MAX_SUGGESTIONS: usize = 6;

pub struct CommandSpec {
    pub name: &'static str,
    /// Argument hint, e.g. `[path]`. Empty when the command takes none.
    pub args: &'static str,
    pub description: &'static str,
    /// Keyboard shortcut for the command, if any, e.g. `ctrl+r`.
    pub keys: &'static str,
}

pub enum CommandEvent {
    None,
    Cancel,
    Submit { name: &'static str, args: String },
    Unknown(String),
}

/// `:` prompt with fuzzy-prefix suggestions for a screen's commands.
pub struct CommandBar {
    specs: &'static [CommandSpec],
    input: Input,
    selected: usize,
}

impl CommandBar {
    pub fn new(specs: &'static [CommandSpec]) -> Self {
        Self {
            specs,
            input: Input::default(),
            selected: 0,
        }
    }

    /// The typed text, ignoring a leading `:` (the prompt already shows one).
    fn text(&self) -> &str {
        self.input
            .value()
            .trim_start()
            .trim_start_matches(':')
            .trim_start()
    }

    fn query(&self) -> (&str, &str) {
        let value = self.text();
        match value.split_once(char::is_whitespace) {
            Some((name, args)) => (name, args.trim()),
            None => (value, ""),
        }
    }

    fn suggestions(&self) -> Vec<&'static CommandSpec> {
        let (name, _) = self.query();
        let typing_args = self.text().contains(char::is_whitespace);

        self.specs
            .iter()
            .filter(|spec| {
                if typing_args {
                    spec.name == name
                } else {
                    spec.name.starts_with(&name.to_lowercase())
                }
            })
            .collect()
    }

    fn complete(&mut self, spec: &CommandSpec) {
        if spec.args.is_empty() {
            self.input.set(spec.name);
        } else {
            self.input.set(&format!("{} ", spec.name));
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.input.insert(crate::utils::first_line(text));
        self.selected = 0;
    }

    pub fn handle(&mut self, key: KeyEvent) -> CommandEvent {
        let suggestions = self.suggestions();

        match key.code {
            KeyCode::Esc => return CommandEvent::Cancel,
            KeyCode::Backspace if self.input.is_empty() => return CommandEvent::Cancel,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(suggestions.len().saturating_sub(1))
            }
            KeyCode::Tab => {
                if let Some(spec) = suggestions.get(self.selected) {
                    self.complete(spec);
                }
            }
            KeyCode::Enter => {
                let (name, args) = self.query();

                // An exact name wins; otherwise run the highlighted suggestion, which is
                // also how commands picked with ↑↓ (and nothing typed) get run.
                let spec = self
                    .specs
                    .iter()
                    .find(|spec| spec.name == name)
                    .or_else(|| suggestions.get(self.selected).copied());

                return match spec {
                    Some(spec) => CommandEvent::Submit {
                        name: spec.name,
                        args: args.to_string(),
                    },
                    None if name.is_empty() => CommandEvent::Cancel,
                    None => CommandEvent::Unknown(name.to_string()),
                };
            }
            _ => {
                if self.input.handle(key) {
                    self.selected = 0;
                }
            }
        }

        CommandEvent::None
    }

    /// Draws the prompt in `area` and the suggestion popup directly above it.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_suggestions(frame, area);

        self.input.render(
            frame,
            area,
            InputView {
                title: Line::from(Span::styled(" Command ", Style::new().fg(ACCENT).bold())),
                prompt: ":",
                placeholder: "type a command",
                enabled: true,
            },
        );
    }

    fn render_suggestions(&self, frame: &mut Frame, area: Rect) {
        let suggestions = self.suggestions();
        if suggestions.is_empty() {
            return;
        }

        let visible = suggestions.len().min(MAX_SUGGESTIONS) as u16;
        let height = (visible + 2).min(area.y);
        if height < 3 {
            return;
        }

        let popup = Rect::new(area.x, area.y - height, area.width, height);
        let name_width = suggestions
            .iter()
            .map(|spec| spec.name.len() + spec.args.len() + 1)
            .max()
            .unwrap_or(0);

        // Borders and the highlight symbol take 4 columns.
        let row_width = popup.width.saturating_sub(4) as usize;

        let items: Vec<ListItem> = suggestions
            .iter()
            .map(|spec| {
                let padding = name_width - spec.name.len() - spec.args.len();
                let mut spans = vec![
                    Span::styled(spec.name, Style::new().fg(TEXT).bold()),
                    Span::styled(format!(" {}", spec.args), Style::new().fg(MUTED)),
                    Span::raw(" ".repeat(padding + 2)),
                    Span::styled(spec.description, Style::new().fg(SUBTLE)),
                ];

                // Right-align the shortcut when it fits.
                let used: usize = spans.iter().map(Span::width).sum();
                let keys = Span::raw(spec.keys).width();
                if !spec.keys.is_empty() && used + keys + 2 <= row_width {
                    spans.push(Span::raw(" ".repeat(row_width - used - keys - 1)));
                    spans.push(Span::styled(spec.keys, Style::new().fg(MUTED)));
                }

                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(MUTED)),
            )
            .highlight_symbol(Span::styled("▌ ", Style::new().fg(ACCENT)))
            .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
            .highlight_style(Style::new().bg(SURFACE));

        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(list, popup, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    const SPECS: &[CommandSpec] = &[
        CommandSpec {
            name: "run",
            args: "",
            description: "",
            keys: "",
        },
        CommandSpec {
            name: "save",
            args: "[path]",
            description: "",
            keys: "",
        },
        CommandSpec {
            name: "back",
            args: "",
            description: "",
            keys: "",
        },
    ];

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(bar: &mut CommandBar, text: &str) {
        for c in text.chars() {
            bar.handle(key(KeyCode::Char(c)));
        }
    }

    fn submit(text: &str) -> CommandEvent {
        let mut bar = CommandBar::new(SPECS);
        type_text(&mut bar, text);
        bar.handle(key(KeyCode::Enter))
    }

    fn assert_submits(event: CommandEvent, expected: &str, expected_args: &str) {
        match event {
            CommandEvent::Submit { name, args } => {
                assert_eq!((name, args.as_str()), (expected, expected_args))
            }
            _ => panic!("expected {expected} to be submitted"),
        }
    }

    #[test]
    fn runs_an_exact_name() {
        assert_submits(submit("back"), "back", "");
    }

    #[test]
    fn ignores_a_leading_colon() {
        assert_submits(submit(":back"), "back", "");
        assert_submits(submit(": save out.rs"), "save", "out.rs");
    }

    #[test]
    fn passes_arguments() {
        assert_submits(submit("save  some/file.rs  "), "save", "some/file.rs");
    }

    #[test]
    fn runs_the_best_suggestion_for_a_prefix() {
        assert_submits(submit("s"), "save", "");
        assert_submits(submit("B"), "back", "");
    }

    #[test]
    fn enter_with_nothing_typed_runs_the_highlighted_command() {
        let mut bar = CommandBar::new(SPECS);
        bar.handle(key(KeyCode::Down));
        bar.handle(key(KeyCode::Down));
        assert_submits(bar.handle(key(KeyCode::Enter)), "back", "");
    }

    #[test]
    fn selection_stays_within_the_suggestions() {
        let mut bar = CommandBar::new(SPECS);
        for _ in 0..10 {
            bar.handle(key(KeyCode::Down));
        }
        assert_submits(bar.handle(key(KeyCode::Enter)), "back", "");
        for _ in 0..10 {
            bar.handle(key(KeyCode::Up));
        }
        assert_submits(bar.handle(key(KeyCode::Enter)), "run", "");
    }

    #[test]
    fn reports_unknown_commands() {
        match submit("nope") {
            CommandEvent::Unknown(name) => assert_eq!(name, "nope"),
            _ => panic!("expected an unknown command"),
        }
    }

    #[test]
    fn arguments_only_match_the_exact_command() {
        // `sa x` isn't `save x`: once arguments start, the name must be complete.
        assert!(matches!(submit("sa x"), CommandEvent::Unknown(_)));
    }

    #[test]
    fn tab_completes_and_leaves_room_for_arguments() {
        let mut bar = CommandBar::new(SPECS);
        type_text(&mut bar, "sa");
        bar.handle(key(KeyCode::Tab));
        assert_eq!(bar.input.value(), "save ");

        let mut bar = CommandBar::new(SPECS);
        type_text(&mut bar, "r");
        bar.handle(key(KeyCode::Tab));
        assert_eq!(bar.input.value(), "run");
    }

    #[test]
    fn esc_or_backspace_on_empty_cancels() {
        let mut bar = CommandBar::new(SPECS);
        assert!(matches!(
            bar.handle(key(KeyCode::Esc)),
            CommandEvent::Cancel
        ));
        assert!(matches!(
            bar.handle(key(KeyCode::Backspace)),
            CommandEvent::Cancel
        ));

        type_text(&mut bar, "r");
        assert!(matches!(
            bar.handle(key(KeyCode::Backspace)),
            CommandEvent::None
        ));
    }

    #[test]
    fn paste_uses_the_first_line() {
        let mut bar = CommandBar::new(SPECS);
        bar.paste("save a.rs\nrm -rf /");
        assert_eq!(bar.input.value(), "save a.rs");
    }
}
