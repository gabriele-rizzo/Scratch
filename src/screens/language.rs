use std::{
    cell::Cell,
    path::PathBuf,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{HighlightSpacing, List, ListItem, ListState, Paragraph},
};
use tuimon::{Screen, ScreenAction};

use crate::{
    runners::{self, Detection, RUNNERS, Runner},
    screens::{Editor, draft_label},
    ui::{
        self, ACCENT, CommandBar, CommandEvent, CommandSpec, ERROR, Help, HelpSection, Input,
        InputView, MUTED, SUBTLE, SUCCESS, SURFACE, TEXT, Toast,
    },
    utils,
};

const COMMANDS: &[CommandSpec<Language>] = &[
    CommandSpec {
        name: "open",
        args: "<path>",
        description: "Open a file",
        keys: "",
        paths: true,
        run: Language::command_open,
    },
    CommandSpec {
        name: "help",
        args: "",
        description: "Show every key and command",
        keys: "?",
        paths: false,
        run: Language::command_help,
    },
    CommandSpec {
        name: "exit",
        args: "",
        description: "Quit Scratch",
        keys: "ctrl+c",
        paths: false,
        run: Language::command_exit,
    },
];

enum Status {
    Checking,
    Available(PathBuf),
    /// Why it can't be used, e.g. "not installed".
    Unavailable(&'static str),
}

pub struct Language {
    statuses: Vec<Status>,
    /// Pending detection results; dropped once every runner has been checked.
    detection: Option<Receiver<(usize, Detection)>>,
    started: Instant,
    input: Input,
    command_bar: Option<CommandBar<Language>>,
    toast: Option<Toast>,
    /// Index into `matches()`.
    selected: usize,
    /// The list's scroll position, kept between frames so it only scrolls once the
    /// highlight leaves the visible rows.
    list_offset: Cell<usize>,
    /// Each language's draft, as shown in the list.
    drafts: Vec<Option<String>>,
    /// Whether `drafts` may be out of date, after an editor was opened.
    drafts_stale: bool,
    help: Option<Help>,
}

impl Language {
    pub fn new() -> Self {
        Self {
            statuses: RUNNERS.iter().map(|_| Status::Checking).collect(),
            detection: Some(runners::detect()),
            started: Instant::now(),
            input: Input::default(),
            command_bar: None,
            toast: None,
            selected: 0,
            list_offset: Cell::new(0),
            drafts: Vec::new(),
            drafts_stale: true,
            help: None,
        }
    }

    /// Call when the filter changes: the list starts over from the top.
    fn filter_changed(&mut self) {
        self.selected = 0;
        self.list_offset.set(0);
    }

    /// Rereads drafts; editors write them when they close.
    fn refresh_drafts(&mut self) {
        if self.drafts_stale {
            self.drafts = RUNNERS.iter().map(draft_label).collect();
            self.drafts_stale = false;
        }
    }

    fn checking(&self) -> bool {
        self.detection.is_some()
    }

    fn checked(&self) -> usize {
        self.statuses
            .iter()
            .filter(|status| !matches!(status, Status::Checking))
            .count()
    }

    fn poll_detection(&mut self) {
        let Some(rx) = &self.detection else { return };

        for (index, binary) in rx.try_iter() {
            self.statuses[index] = match binary {
                Detection::Found(binary) => Status::Available(binary),
                Detection::Missing => Status::Unavailable("not installed"),
                Detection::Placeholder(hint) => Status::Unavailable(hint),
            };
        }

        if self.checked() == RUNNERS.len() {
            self.detection = None;
        }
    }

    /// How well `runner` matches the input: exact, then prefix, then substring.
    /// `None` hides it.
    fn rank(&self, runner: &Runner) -> Option<u8> {
        rank(runner.name, self.input.value())
    }

    /// Indices of runners that match the input, best matches first.
    fn visible(&self) -> Vec<usize> {
        let mut visible: Vec<(u8, usize)> = (0..RUNNERS.len())
            .filter_map(|index| Some((self.rank(&RUNNERS[index])?, index)))
            .collect();
        visible.sort();
        visible.into_iter().map(|(_, index)| index).collect()
    }

    /// The visible runners that are available, in display order.
    fn matches(&self) -> Vec<usize> {
        self.visible()
            .into_iter()
            .filter(|&index| matches!(self.statuses[index], Status::Available(_)))
            .collect()
    }

    fn select(&mut self) -> Result<ScreenAction> {
        let Some(&index) = self.matches().get(self.selected) else {
            return Ok(ScreenAction::None);
        };

        let Status::Available(binary) = &self.statuses[index] else {
            return Ok(ScreenAction::None);
        };

        let editor = Editor::new(&RUNNERS[index], binary.clone())?;

        // Start from the full list when coming back from the editor.
        self.input.clear();
        self.filter_changed();
        self.drafts_stale = true;

        Ok(ScreenAction::Push(Box::new(editor)))
    }

    fn command_open(&mut self, args: &str) -> ScreenAction {
        match Editor::open(args, None) {
            Ok(editor) => {
                self.drafts_stale = true;
                ScreenAction::Push(Box::new(editor))
            }
            Err(err) => {
                self.toast = Some(Toast::error(format!("Couldn't open {err}")));
                ScreenAction::None
            }
        }
    }

    fn command_help(&mut self, _: &str) -> ScreenAction {
        self.help = Some(Help::new(language_help()));
        ScreenAction::None
    }

    fn command_exit(&mut self, _: &str) -> ScreenAction {
        ScreenAction::Quit
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let header = Paragraph::new(vec![
            Line::from(Span::styled("Scratch", Style::new().fg(ACCENT).bold())),
            Line::from(Span::styled(
                "Pick a language to start a scratch file",
                Style::new().fg(SUBTLE),
            )),
        ]);

        frame.render_widget(header, area);
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect) {
        let visible = self.visible();

        if visible.is_empty() {
            let empty = Line::from(vec![
                Span::styled("No languages match ", Style::new().fg(MUTED)),
                Span::styled(self.input.value().trim(), Style::new().fg(SUBTLE)),
            ]);
            frame.render_widget(Paragraph::new(empty), area);
            return;
        }

        let name_width = RUNNERS
            .iter()
            .map(|runner| runner.name.len())
            .max()
            .unwrap_or(0);

        // A column for drafts, only when there are any.
        let draft_width = self
            .drafts
            .iter()
            .flatten()
            .map(|label| label.chars().count() + 2)
            .max()
            .unwrap_or(0)
            .min(28);

        // What's left for each program's path: the highlight, icon, name and draft
        // columns come first.
        let draft_columns = if draft_width > 0 { draft_width + 2 } else { 0 };
        // One more column is kept free for the scrollbar.
        let path_width =
            usize::from(area.width).saturating_sub(3 + name_width + 4 + draft_columns + 2);

        let items: Vec<ListItem> = visible
            .iter()
            .map(|&index| {
                let runner = &RUNNERS[index];
                let name = format!(" {:name_width$}   ", runner.name);
                let draft = self.draft_column(index, draft_width);

                let line = match &self.statuses[index] {
                    Status::Checking => Line::from(vec![
                        Span::styled(ui::spinner(self.started), Style::new().fg(ACCENT)),
                        Span::styled(name, Style::new().fg(SUBTLE)),
                        Span::styled("checking…", Style::new().fg(MUTED).italic()),
                    ]),
                    Status::Available(binary) => Line::from(
                        [
                            vec![
                                Span::styled("✓", Style::new().fg(SUCCESS)),
                                Span::styled(name, Style::new().fg(TEXT).bold()),
                            ],
                            draft,
                            vec![Span::styled(
                                utils::truncate_start(&utils::display_path(binary), path_width),
                                Style::new().fg(MUTED),
                            )],
                        ]
                        .concat(),
                    ),
                    Status::Unavailable(reason) => Line::from(vec![
                        Span::styled("✗", Style::new().fg(ERROR).dim()),
                        Span::styled(name, Style::new().fg(MUTED)),
                        Span::styled(*reason, Style::new().fg(MUTED).italic()),
                    ]),
                };

                ListItem::new(line)
            })
            .collect();

        let highlighted = self.matches().get(self.selected).copied();
        let position = highlighted.and_then(|index| visible.iter().position(|&i| i == index));

        let list = List::new(items)
            .highlight_symbol(Span::styled("▌ ", Style::new().fg(ACCENT)))
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_style(Style::new().bg(SURFACE));

        let mut state = ListState::default()
            .with_offset(self.list_offset.get())
            .with_selected(position);
        frame.render_stateful_widget(list, area, &mut state);
        self.list_offset.set(state.offset());

        ui::scrollbar(
            frame,
            area,
            visible.len(),
            usize::from(area.height),
            state.offset(),
            SUBTLE,
        );
    }

    /// The draft column for runner `index`, padded to `width` (empty when there's
    /// no column).
    fn draft_column(&self, index: usize, width: usize) -> Vec<Span<'static>> {
        if width == 0 {
            return Vec::new();
        }

        match self.drafts.get(index).cloned().flatten() {
            Some(label) => {
                let label = utils::truncate_start(&label, width - 2);
                let padding = width - label.chars().count();
                vec![
                    Span::styled("● ", Style::new().fg(ACCENT)),
                    Span::styled(label, Style::new().fg(SUBTLE)),
                    Span::raw(" ".repeat(padding)),
                ]
            }
            None => vec![Span::raw(" ".repeat(width + 2))],
        }
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        if let Some(command_bar) = &self.command_bar {
            command_bar.render(frame, area);
            return;
        }

        let title = if self.checking() {
            Line::from(vec![
                Span::styled(
                    format!(" {} ", ui::spinner(self.started)),
                    Style::new().fg(ACCENT),
                ),
                Span::styled(
                    format!("Checking languages {}/{} ", self.checked(), RUNNERS.len()),
                    Style::new().fg(MUTED),
                ),
            ])
        } else {
            Line::from(Span::styled(" Language ", Style::new().fg(ACCENT).bold()))
        };

        self.input.render(
            frame,
            area,
            InputView {
                title,
                prompt: "›",
                placeholder: if self.checking() {
                    "please wait…"
                } else {
                    "type to filter, : for commands"
                },
                enabled: !self.checking(),
            },
        );
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let line = match self.toast.as_ref().filter(|toast| toast.is_visible()) {
            Some(toast) => toast.line(),
            None if self.command_bar.is_some() => Line::from(ui::hints(&[
                ("tab", "complete"),
                ("enter", "run"),
                ("esc", "cancel"),
            ])),
            None => Line::from(ui::hints(&[
                ("↑↓", "navigate"),
                ("enter", "open"),
                (":", "commands"),
                ("?", "help"),
                ("ctrl+c", "quit"),
            ])),
        };

        frame.render_widget(Paragraph::new(line), area);
    }
}

/// Every key and command on the language screen, for the help overlay.
fn language_help() -> Vec<HelpSection> {
    vec![
        HelpSection::new(
            "Language screen",
            &[
                ("↑↓", "Move through the languages"),
                ("enter", "Open the highlighted language"),
                ("type", "Filter the list"),
                ("esc", "Clear the filter"),
                (":", "Commands"),
                ("? / f1", "This help"),
                ("ctrl+c", "Quit"),
            ],
        ),
        HelpSection::commands(COMMANDS),
    ]
}

/// How well `name` matches `query`, case-insensitively: 0 exact, 1 prefix,
/// 2 substring. `None` when it doesn't match.
fn rank(name: &str, query: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    let name = name.to_lowercase();

    if name == query {
        Some(0)
    } else if name.starts_with(&query) {
        Some(1)
    } else if name.contains(&query) {
        Some(2)
    } else {
        None
    }
}

impl Screen for Language {
    /// Ticks only while languages are being checked or a message is showing.
    fn tick_rate(&self) -> Option<Duration> {
        let live = self.checking() || self.toast.as_ref().is_some_and(Toast::is_visible);
        live.then_some(ui::SPINNER_INTERVAL)
    }

    fn update(&mut self) -> Result<ScreenAction> {
        self.poll_detection();
        Ok(ScreenAction::None)
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.refresh_drafts();

        let matches = self.matches().len();
        self.selected = self.selected.min(matches.saturating_sub(1));

        let area = frame.area().inner(ratatui::layout::Margin::new(2, 1));
        let [header, list, input, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(area);

        self.draw_header(frame, header);
        self.draw_list(frame, list);
        self.draw_input(frame, input);

        let footer = footer.inner(ratatui::layout::Margin::new(1, 0));
        self.draw_footer(frame, footer);

        if let Some(help) = &mut self.help {
            help.render(frame, frame.area());
        }
    }

    fn handle(&mut self, event: Event) -> Result<ScreenAction> {
        if let Some(help) = &mut self.help {
            if help.handle(&event) {
                self.help = None;
            }
            return Ok(ScreenAction::None);
        }

        if let Some(action) = utils::handle_exit_input(&event) {
            return Ok(action);
        }

        // `?` never appears in a language name, so it always means help.
        if self.command_bar.is_none()
            && let Event::Key(key) = &event
            && matches!(key.code, KeyCode::Char('?') | KeyCode::F(1))
        {
            self.help = Some(Help::new(language_help()));
            return Ok(ScreenAction::None);
        }

        // Results may have arrived since the last draw.
        self.poll_detection();

        if self.checking() {
            return Ok(ScreenAction::None);
        }

        let key = match event {
            Event::Key(key) => key,
            Event::Paste(text) => {
                match &mut self.command_bar {
                    Some(command_bar) => command_bar.paste(&text),
                    None => {
                        self.input.insert(utils::first_line(&text));
                        self.filter_changed();
                    }
                }
                return Ok(ScreenAction::None);
            }
            _ => return Ok(ScreenAction::None),
        };

        if key.kind != KeyEventKind::Press {
            return Ok(ScreenAction::None);
        }

        if let Some(command_bar) = &mut self.command_bar {
            return Ok(match command_bar.handle(key) {
                CommandEvent::None => ScreenAction::None,
                CommandEvent::Cancel => {
                    self.command_bar = None;
                    ScreenAction::None
                }
                CommandEvent::Submit { command, args } => {
                    self.command_bar = None;
                    (command.run)(self, &args)
                }
                CommandEvent::Unknown(name) => {
                    self.command_bar = None;
                    self.toast = Some(Toast::error(format!("Unknown command: {name}")));
                    ScreenAction::None
                }
            });
        }

        match key.code {
            KeyCode::Char(':') if self.input.is_empty() => {
                self.command_bar = Some(CommandBar::new(COMMANDS))
            }
            KeyCode::Enter => return self.select(),
            KeyCode::Esc => {
                self.input.clear();
                self.filter_changed();
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected += 1,
            _ => {
                if self.input.handle(key) {
                    self.filter_changed();
                }
            }
        }

        Ok(ScreenAction::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_exact_then_prefix_then_substring() {
        assert_eq!(rank("C", "c"), Some(0));
        assert_eq!(rank("C++", "c"), Some(1));
        assert_eq!(rank("JavaScript", "c"), Some(2));
        assert_eq!(rank("Rust", "c"), None);
    }

    #[test]
    fn ranking_ignores_case_and_surrounding_space() {
        assert_eq!(rank("Python", "  PYTH "), Some(1));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(RUNNERS.iter().all(|runner| rank(runner.name, "").is_some()));
    }
}
