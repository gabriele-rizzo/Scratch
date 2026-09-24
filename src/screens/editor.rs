use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::event::{Event, KeyCode};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Padding, Paragraph},
};
use ratatui_code_editor::{editor::Editor as CodeEditor, theme::vesper};
use tuimon::{Screen, ScreenAction};

use crate::{
    runners::{Process, RunStatus, Runner, Stream},
    ui::{
        self, ACCENT, CommandBar, CommandEvent, CommandSpec, ERROR, MUTED, ON_ACCENT, SUBTLE,
        SUCCESS, TEXT, Toast,
    },
    utils,
};

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "run",
        args: "",
        description: "Run the file",
    },
    CommandSpec {
        name: "save",
        args: "[path]",
        description: "Save the file",
    },
    CommandSpec {
        name: "clear",
        args: "",
        description: "Close the output panel",
    },
    CommandSpec {
        name: "back",
        args: "",
        description: "Pick another language",
    },
    CommandSpec {
        name: "exit",
        args: "",
        description: "Quit Scratch",
    },
];

pub struct Editor {
    editor: CodeEditor,
    editor_area: Rect,
    runner: &'static Runner,
    binary: PathBuf,
    /// Temporary directory the file is written to before running.
    workdir: PathBuf,
    /// Where the file was last saved.
    path: Option<PathBuf>,
    saved: String,
    process: Option<Process>,
    command_bar: Option<CommandBar>,
    toast: Option<Toast>,
}

impl Editor {
    pub fn new(runner: &'static Runner, binary: PathBuf) -> Result<Self> {
        let editor = CodeEditor::new(runner.syntax, "", vesper())?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.subsec_nanos());
        let workdir = env::temp_dir().join(format!("scratch-{}-{nanos}", process::id()));

        Ok(Self {
            editor,
            editor_area: Rect::default(),
            runner,
            binary,
            workdir,
            path: None,
            saved: String::new(),
            process: None,
            command_bar: None,
            toast: None,
        })
    }

    fn execute(&mut self, name: &str, args: &str) -> ScreenAction {
        match name {
            "run" => self.run(),
            "save" => self.save(args),
            "clear" => self.process = None,
            "back" => return ScreenAction::Pop,
            "exit" => return ScreenAction::Quit,
            _ => {}
        }

        ScreenAction::None
    }

    fn run(&mut self) {
        // Stops the previous run, if any.
        self.process = None;

        let file = self.workdir.join(format!("main.{}", self.runner.extension));
        let result = fs::create_dir_all(&self.workdir)
            .and_then(|_| fs::write(&file, self.editor.get_content()))
            .and_then(|_| {
                let mut command = (self.runner.command)(&self.binary, &file);
                command.current_dir(&self.workdir);
                Process::spawn(command)
            });

        match result {
            Ok(process) => self.process = Some(process),
            Err(err) => self.toast = Some(Toast::error(format!("Couldn't run: {err}"))),
        }
    }

    fn save(&mut self, args: &str) {
        let path = if args.is_empty() {
            self.path
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("scratch.{}", self.runner.extension)))
        } else {
            PathBuf::from(args)
        };

        let content = self.editor.get_content();

        match fs::write(&path, &content) {
            Ok(()) => {
                self.toast = Some(Toast::success(format!("Saved {}", path.display())));
                self.path = Some(path);
                self.saved = content;
            }
            Err(err) => {
                self.toast = Some(Toast::error(format!(
                    "Couldn't save {}: {err}",
                    path.display()
                )))
            }
        }
    }

    fn draw_output(&self, frame: &mut Frame, area: Rect, process: &Process) {
        let status = match &process.status {
            RunStatus::Running => Line::from(vec![
                Span::styled(ui::spinner(process.started), Style::new().fg(ACCENT)),
                Span::styled(
                    format!(" running {:.1}s ", process.started.elapsed().as_secs_f32()),
                    Style::new().fg(SUBTLE),
                ),
            ]),
            RunStatus::Exited(status, elapsed) => {
                let (icon, color, label) = match status.code() {
                    Some(0) => ("✓", SUCCESS, "exited 0".to_string()),
                    Some(code) => ("✗", ERROR, format!("exited {code}")),
                    None => ("✗", ERROR, "killed".to_string()),
                };

                Line::from(vec![
                    Span::styled(icon, Style::new().fg(color).bold()),
                    Span::styled(format!(" {label}"), Style::new().fg(color)),
                    Span::styled(
                        format!(" · {:.2}s ", elapsed.as_secs_f32()),
                        Style::new().fg(MUTED),
                    ),
                ])
            }
        };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(MUTED))
            .padding(Padding::horizontal(1))
            .title(Span::styled(" Output ", Style::new().fg(TEXT).bold()))
            .title_top(status.right_aligned());

        let inner = block.inner(area);
        let height = inner.height as usize;
        let skip = process.lines.len().saturating_sub(height);

        let lines: Vec<Line> = if process.lines.is_empty() {
            let placeholder = match process.status {
                RunStatus::Running => "waiting for output…",
                RunStatus::Exited(..) => "no output",
            };
            vec![Line::from(Span::styled(
                placeholder,
                Style::new().fg(MUTED).italic(),
            ))]
        } else {
            process.lines[skip..]
                .iter()
                .map(|(stream, line)| {
                    let color = match stream {
                        Stream::Stdout => TEXT,
                        Stream::Stderr => ERROR,
                    };
                    Line::from(Span::styled(line.as_str(), Style::new().fg(color)))
                })
                .collect()
        };

        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let mut left = vec![
            Span::styled(
                format!(" {} ", self.runner.name),
                Style::new().fg(ON_ACCENT).bg(ACCENT).bold(),
            ),
            Span::raw(" "),
        ];

        match &self.path {
            Some(path) => left.push(Span::styled(
                path.display().to_string(),
                Style::new().fg(SUBTLE),
            )),
            None => left.push(Span::styled("untitled", Style::new().fg(MUTED).italic())),
        }

        if self.editor.get_content() != self.saved {
            left.push(Span::styled(" ●", Style::new().fg(ACCENT)));
        }

        let right = match self.toast.as_ref().filter(|toast| toast.is_visible()) {
            Some(toast) => toast.line(),
            None => Line::from(ui::hints(&[("esc", "commands"), ("ctrl+c", "quit")])),
        };

        frame.render_widget(Paragraph::new(Line::from(left)), area);
        frame.render_widget(Paragraph::new(right.right_aligned()), area);
    }
}

impl Screen for Editor {
    fn draw(&mut self, frame: &mut Frame) {
        if let Some(process) = &mut self.process {
            process.poll();
        }

        let bottom = if self.command_bar.is_some() { 3 } else { 1 };
        let [main_area, bottom_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(bottom)]).areas(frame.area());

        let (editor_area, output_area) = match self.process {
            Some(_) => {
                let [editor, output] =
                    Layout::vertical([Constraint::Percentage(60), Constraint::Min(5)])
                        .areas(main_area);
                (editor, Some(output))
            }
            None => (main_area, None),
        };

        self.editor_area = editor_area;
        frame.render_widget(&self.editor, self.editor_area);

        if let (Some(area), Some(process)) = (output_area, &self.process) {
            self.draw_output(frame, area, process);
        }

        match &self.command_bar {
            Some(command_bar) => command_bar.render(frame, bottom_area),
            None => {
                self.draw_status(frame, bottom_area);

                if let Some((x, y)) = self.editor.get_visible_cursor(&self.editor_area) {
                    frame.set_cursor_position(Position::new(x, y));
                }
            }
        }
    }

    fn handle(&mut self, event: Event) -> Result<ScreenAction> {
        // ctrl+c copies when there's a selection, otherwise quits.
        let has_selection = self
            .editor
            .get_selection()
            .is_some_and(|selection| selection.is_active());

        if (self.command_bar.is_some() || !has_selection)
            && let Some(action) = utils::handle_exit_input(&event)
        {
            return Ok(action);
        }

        if let Some(command_bar) = &mut self.command_bar {
            let Event::Key(key) = event else {
                return Ok(ScreenAction::None);
            };

            return Ok(match command_bar.handle(key) {
                CommandEvent::None => ScreenAction::None,
                CommandEvent::Cancel => {
                    self.command_bar = None;
                    ScreenAction::None
                }
                CommandEvent::Submit { name, args } => {
                    self.command_bar = None;
                    self.execute(name, &args)
                }
                CommandEvent::Unknown(name) => {
                    self.command_bar = None;
                    self.toast = Some(Toast::error(format!("Unknown command: {name}")));
                    ScreenAction::None
                }
            });
        }

        match event {
            Event::Key(key) if key.code == KeyCode::Esc => {
                self.command_bar = Some(CommandBar::new(COMMANDS))
            }
            Event::Key(key) => self.editor.input(key, &self.editor_area)?,
            Event::Mouse(mouse) => self.editor.mouse(mouse, &self.editor_area)?,
            // Event::Paste(value)
            _ => {}
        };

        Ok(ScreenAction::None)
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.process = None;
        let _ = fs::remove_dir_all(&self.workdir);
    }
}
