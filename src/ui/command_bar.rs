use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState},
};

use super::{ACCENT, Input, InputView, MUTED, SUBTLE, SURFACE, TEXT};
use crate::utils::PathCompletion;
use tuimon::ScreenAction;

const MAX_SUGGESTIONS: usize = 6;

/// A command a screen of type `T` understands.
pub struct CommandSpec<T: 'static> {
    pub name: &'static str,
    /// Argument hint, e.g. `[path]`. Empty when the command takes none.
    pub args: &'static str,
    pub description: &'static str,
    /// Keyboard shortcut for the command, if any, e.g. `ctrl+r`.
    pub keys: &'static str,
    /// Whether the argument is a path, which Tab completes.
    pub paths: bool,
    /// Runs the command with its arguments (the text after the name).
    pub run: fn(&mut T, &str) -> ScreenAction,
}

pub enum CommandEvent<T: 'static> {
    None,
    Cancel,
    Submit {
        command: &'static CommandSpec<T>,
        args: String,
    },
    Unknown(String),
}

/// `:` prompt with fuzzy-prefix suggestions for a screen's commands.
pub struct CommandBar<T: 'static> {
    specs: &'static [CommandSpec<T>],
    input: Input,
    selected: usize,
    /// Matches for a path being typed, recomputed only when the text changes.
    paths: Option<PathCompletion>,
    /// Whether a path match was picked with ↑↓ or Tab (so Enter uses it).
    picked: bool,
}

impl<T> CommandBar<T> {
    pub fn new(specs: &'static [CommandSpec<T>]) -> Self {
        Self {
            specs,
            input: Input::default(),
            selected: 0,
            paths: None,
            picked: false,
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

    fn suggestions(&self) -> Vec<&'static CommandSpec<T>> {
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

    /// The path being typed, when the command takes one.
    fn path_arg(&self) -> Option<&str> {
        let (name, rest) = self.text().split_once(char::is_whitespace)?;
        let spec = self.specs.iter().find(|spec| spec.name == name)?;
        spec.paths.then(|| rest.trim_start())
    }

    /// Call after the text changes.
    fn changed(&mut self) {
        self.selected = 0;
        self.picked = false;
        self.paths = self.path_arg().map(PathCompletion::new);
    }

    /// Replaces the path being typed with `path`.
    fn set_path(&mut self, path: &str) {
        let Some((name, _)) = self.text().split_once(char::is_whitespace) else {
            return;
        };
        self.input.set(&format!("{name} {path}"));
        self.changed();
    }

    /// Tab on a path: fill in the only match, else what all matches share, else
    /// step through the matches.
    fn complete_path(&mut self) {
        let Some(paths) = &self.paths else { return };

        let completed = match paths.entries.as_slice() {
            [] => None,
            [only] => Some(paths.apply(only)),
            entries => match paths.common() {
                Some(common) => Some(common),
                None => {
                    self.selected = if self.picked {
                        (self.selected + 1) % entries.len()
                    } else {
                        0
                    };
                    self.picked = true;
                    None
                }
            },
        };

        if let Some(path) = completed {
            self.set_path(&path);
        }
    }

    fn complete(&mut self, spec: &CommandSpec<T>) {
        if spec.args.is_empty() {
            self.input.set(spec.name);
        } else {
            self.input.set(&format!("{} ", spec.name));
        }
        self.changed();
    }

    pub fn paste(&mut self, text: &str) {
        self.input.insert(crate::utils::first_line(text));
        self.changed();
    }

    /// Keys for a path being typed. Returns `None` to handle the key normally.
    fn handle_path_key(&mut self, key: KeyEvent) -> Option<CommandEvent<T>> {
        let count = self.paths.as_ref()?.entries.len();

        match key.code {
            KeyCode::Tab => self.complete_path(),
            KeyCode::Up | KeyCode::Down if count > 0 => {
                self.selected = match (key.code, self.picked) {
                    (_, false) => 0,
                    (KeyCode::Up, true) => (self.selected + count - 1) % count,
                    _ => (self.selected + 1) % count,
                };
                self.picked = true;
            }
            // Enter uses a picked match: a folder is filled in to keep going,
            // a file is filled in and run.
            KeyCode::Enter if self.picked => {
                let paths = self.paths.as_ref()?;
                let entry = paths.entries.get(self.selected)?.clone();
                let path = paths.apply(&entry);
                self.set_path(&path);
                if entry.is_dir {
                    return Some(CommandEvent::None);
                }
                return None;
            }
            _ => return None,
        }

        Some(CommandEvent::None)
    }

    pub fn handle(&mut self, key: KeyEvent) -> CommandEvent<T> {
        if let Some(event) = self.handle_path_key(key) {
            return event;
        }

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
                    Some(command) => CommandEvent::Submit {
                        command,
                        args: args.to_string(),
                    },
                    None if name.is_empty() => CommandEvent::Cancel,
                    None => CommandEvent::Unknown(name.to_string()),
                };
            }
            _ => {
                if self.input.handle(key) {
                    self.changed();
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

    /// Lists path matches above the prompt, folders first-class with a `/`.
    fn render_paths(&self, frame: &mut Frame, area: Rect, paths: &PathCompletion) {
        if paths.entries.is_empty() || paths.is_complete() {
            return;
        }

        let visible = paths.entries.len().min(MAX_SUGGESTIONS) as u16;
        let height = (visible + 2).min(area.y);
        if height < 3 {
            return;
        }
        let popup = Rect::new(area.x, area.y - height, area.width, height);

        let items: Vec<ListItem> = paths
            .entries
            .iter()
            .map(|entry| {
                ListItem::new(if entry.is_dir {
                    Line::from(vec![
                        Span::styled(entry.name.as_str(), Style::new().fg(ACCENT).bold()),
                        Span::styled("/", Style::new().fg(MUTED)),
                    ])
                } else {
                    Line::styled(entry.name.as_str(), Style::new().fg(TEXT))
                })
            })
            .collect();

        let count = match paths.entries.len() {
            1 => " 1 match ".to_string(),
            count => format!(" {count} matches "),
        };
        let list = List::new(items)
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(MUTED))
                    .title_bottom(Line::styled(count, Style::new().fg(MUTED)).right_aligned()),
            )
            .highlight_symbol(Span::styled("▌ ", Style::new().fg(ACCENT)))
            .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
            .highlight_style(Style::new().bg(SURFACE));

        let mut state = ListState::default().with_selected(self.picked.then_some(self.selected));
        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(list, popup, &mut state);
    }

    fn render_suggestions(&self, frame: &mut Frame, area: Rect) {
        if let Some(paths) = &self.paths {
            return self.render_paths(frame, area, paths);
        }

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

    fn nothing(_: &mut (), _: &str) -> ScreenAction {
        ScreenAction::None
    }

    const SPECS: &[CommandSpec<()>] = &[
        CommandSpec {
            name: "run",
            args: "",
            description: "",
            keys: "",
            paths: false,
            run: nothing,
        },
        CommandSpec {
            name: "save",
            args: "[path]",
            description: "",
            keys: "",
            paths: false,
            run: nothing,
        },
        CommandSpec {
            name: "back",
            args: "",
            description: "",
            keys: "",
            paths: false,
            run: nothing,
        },
        CommandSpec {
            name: "open",
            args: "<path>",
            description: "",
            keys: "",
            paths: true,
            run: nothing,
        },
    ];

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(bar: &mut CommandBar<()>, text: &str) {
        for c in text.chars() {
            bar.handle(key(KeyCode::Char(c)));
        }
    }

    fn submit(text: &str) -> CommandEvent<()> {
        let mut bar = CommandBar::new(SPECS);
        type_text(&mut bar, text);
        bar.handle(key(KeyCode::Enter))
    }

    fn assert_submits(event: CommandEvent<()>, expected: &str, expected_args: &str) {
        match event {
            CommandEvent::Submit { command, args } => {
                assert_eq!((command.name, args.as_str()), (expected, expected_args))
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
        assert_submits(bar.handle(key(KeyCode::Enter)), "open", "");
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

    mod paths {
        use std::{env, fs, path::PathBuf, process};

        use super::*;

        struct TempDir(PathBuf);

        impl TempDir {
            fn new(name: &str, files: &[&str]) -> Self {
                let dir =
                    env::temp_dir().join(format!("scratch-test-bar-{name}-{}", process::id()));
                let _ = fs::remove_dir_all(&dir);
                fs::create_dir_all(&dir).unwrap();
                for file in files {
                    match file.strip_suffix('/') {
                        Some(folder) => fs::create_dir_all(dir.join(folder)).unwrap(),
                        None => fs::write(dir.join(file), "").unwrap(),
                    }
                }
                Self(dir)
            }

            fn path(&self, rest: &str) -> String {
                format!("{}/{rest}", self.0.display())
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        fn bar(text: &str) -> CommandBar<()> {
            let mut bar = CommandBar::new(SPECS);
            bar.paste(text);
            bar
        }

        #[test]
        fn tab_fills_in_the_only_match() {
            let dir = TempDir::new("only", &["main.rs", "notes.txt"]);
            let mut bar = bar(&format!("open {}", dir.path("ma")));
            bar.handle(key(KeyCode::Tab));
            assert_eq!(bar.input.value(), format!("open {}", dir.path("main.rs")));
        }

        #[test]
        fn tab_extends_then_steps_through_matches() {
            let dir = TempDir::new("step", &["test_a.py", "test_b.py"]);
            let mut bar = bar(&format!("open {}", dir.path("t")));

            bar.handle(key(KeyCode::Tab));
            assert_eq!(bar.input.value(), format!("open {}", dir.path("test_")));
            assert!(!bar.picked);

            bar.handle(key(KeyCode::Tab));
            assert!(bar.picked);
            assert_eq!(bar.selected, 0);
            bar.handle(key(KeyCode::Tab));
            assert_eq!(bar.selected, 1);
            bar.handle(key(KeyCode::Tab));
            assert_eq!(bar.selected, 0);
        }

        #[test]
        fn enter_on_a_picked_folder_fills_it_in_and_waits() {
            let dir = TempDir::new("folder", &["src/", "setup.py"]);
            let mut bar = bar(&format!("open {}", dir.path("s")));
            bar.handle(key(KeyCode::Down));
            bar.handle(key(KeyCode::Down));

            assert!(matches!(
                bar.handle(key(KeyCode::Enter)),
                CommandEvent::None
            ));
            assert_eq!(bar.input.value(), format!("open {}", dir.path("src/")));
        }

        #[test]
        fn enter_on_a_picked_file_runs_it() {
            let dir = TempDir::new("file", &["src/", "setup.py"]);
            let mut bar = bar(&format!("open {}", dir.path("s")));
            bar.handle(key(KeyCode::Down));

            assert_submits(
                bar.handle(key(KeyCode::Enter)),
                "open",
                &dir.path("setup.py"),
            );
        }

        #[test]
        fn enter_without_picking_runs_what_was_typed() {
            let dir = TempDir::new("typed", &["a.py", "ab.py"]);
            let mut bar = bar(&format!("open {}", dir.path("a.py")));
            assert_submits(bar.handle(key(KeyCode::Enter)), "open", &dir.path("a.py"));
        }

        #[test]
        fn only_path_commands_complete_paths() {
            let bar = bar("run a");
            assert!(bar.paths.is_none());
            let bar = bar_for_open();
            assert!(bar.paths.is_some());
        }

        fn bar_for_open() -> CommandBar<()> {
            bar("open ")
        }
    }
}
