use std::{path::PathBuf, sync::mpsc::Receiver, time::Instant};

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
    runners::{self, RUNNERS, Runner},
    screens::Editor,
    ui::{
        self, ACCENT, CommandBar, CommandEvent, CommandSpec, ERROR, Input, InputView, MUTED,
        SUBTLE, SUCCESS, SURFACE, TEXT, Toast,
    },
    utils,
};

const COMMANDS: &[CommandSpec] = &[CommandSpec {
    name: "exit",
    args: "",
    description: "Quit Scratch",
    keys: "ctrl+c",
}];

enum Status {
    Checking,
    Available(PathBuf),
    Unavailable,
}

pub struct Language {
    statuses: Vec<Status>,
    /// Pending detection results; dropped once every runner has been checked.
    detection: Option<Receiver<(usize, Option<PathBuf>)>>,
    started: Instant,
    input: Input,
    command_bar: Option<CommandBar>,
    toast: Option<Toast>,
    /// Index into `matches()`.
    selected: usize,
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
                Some(binary) => Status::Available(binary),
                None => Status::Unavailable,
            };
        }

        if self.checked() == RUNNERS.len() {
            self.detection = None;
        }
    }

    /// How well `runner` matches the input: exact, then prefix, then substring.
    /// `None` hides it.
    fn rank(&self, runner: &Runner) -> Option<u8> {
        let query = self.input.value().trim().to_lowercase();
        let name = runner.name.to_lowercase();

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
        self.selected = 0;

        Ok(ScreenAction::Push(Box::new(editor)))
    }

    fn execute(&mut self, name: &str) -> ScreenAction {
        match name {
            "exit" => ScreenAction::Quit,
            _ => ScreenAction::None,
        }
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

        let items: Vec<ListItem> = visible
            .iter()
            .map(|&index| {
                let runner = &RUNNERS[index];
                let name = format!(" {:name_width$}   ", runner.name);

                let line = match &self.statuses[index] {
                    Status::Checking => Line::from(vec![
                        Span::styled(ui::spinner(self.started), Style::new().fg(ACCENT)),
                        Span::styled(name, Style::new().fg(SUBTLE)),
                        Span::styled("checking…", Style::new().fg(MUTED).italic()),
                    ]),
                    Status::Available(binary) => Line::from(vec![
                        Span::styled("✓", Style::new().fg(SUCCESS)),
                        Span::styled(name, Style::new().fg(TEXT).bold()),
                        Span::styled(binary.display().to_string(), Style::new().fg(MUTED)),
                    ]),
                    Status::Unavailable => Line::from(vec![
                        Span::styled("✗", Style::new().fg(ERROR).dim()),
                        Span::styled(name, Style::new().fg(MUTED)),
                        Span::styled("not installed", Style::new().fg(MUTED).italic()),
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

        let mut state = ListState::default().with_selected(position);
        frame.render_stateful_widget(list, area, &mut state);
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
                ("ctrl+c", "quit"),
            ])),
        };

        frame.render_widget(Paragraph::new(line), area);
    }
}

impl Screen for Language {
    fn update(&mut self) -> Result<ScreenAction> {
        self.poll_detection();
        Ok(ScreenAction::None)
    }

    fn draw(&mut self, frame: &mut Frame) {
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
    }

    fn handle(&mut self, event: Event) -> Result<ScreenAction> {
        if let Some(action) = utils::handle_exit_input(&event) {
            return Ok(action);
        }

        // Results may have arrived since the last draw.
        self.poll_detection();

        if self.checking() {
            return Ok(ScreenAction::None);
        }

        let Event::Key(key) = event else {
            return Ok(ScreenAction::None);
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
                CommandEvent::Submit { name, .. } => {
                    self.command_bar = None;
                    self.execute(name)
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
                self.selected = 0;
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected += 1,
            _ => {
                if self.input.handle(key) {
                    self.selected = 0;
                }
            }
        }

        Ok(ScreenAction::None)
    }
}
