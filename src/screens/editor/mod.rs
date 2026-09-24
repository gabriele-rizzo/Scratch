use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use ratatui_code_editor::{editor::Editor as CodeEditor, theme::vesper};
use tuimon::{Screen, ScreenAction};

use crate::{
    runners::{Process, Runner},
    ui::{self, ACCENT, CommandBar, CommandEvent, CommandSpec, MUTED, ON_ACCENT, SUBTLE, Toast},
    utils,
};

mod output;
use output::Output;

const MOUSE_SCROLL_LINES: usize = 3;

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
    output: Option<Output>,
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
            output: None,
            command_bar: None,
            toast: None,
        })
    }

    fn execute(&mut self, name: &str, args: &str) -> ScreenAction {
        match name {
            "run" => self.run(),
            "save" => self.save(args),
            "clear" => self.output = None,
            "back" => return ScreenAction::Pop,
            "exit" => return ScreenAction::Quit,
            _ => {}
        }

        ScreenAction::None
    }

    fn run(&mut self) {
        // Stops the previous run, if any.
        self.output = None;

        let file = self.workdir.join(format!("main.{}", self.runner.extension));
        let written = fs::create_dir_all(&self.workdir)
            .and_then(|_| fs::write(&file, self.editor.get_content()));

        if let Err(err) = written {
            self.output = Some(Output::failed(
                format!("Couldn't write {}", file.display()),
                err,
            ));
            return;
        }

        let mut command = (self.runner.command)(&self.binary, &file);
        command.current_dir(&self.workdir);

        self.output = Some(match Process::spawn(command) {
            Ok(process) => Output::started(process, &self.workdir),
            Err(err) => {
                let name = self
                    .binary
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                Output::failed(
                    format!("Couldn't start {name}"),
                    format!("{}: {err}", self.binary.display()),
                )
            }
        });
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
            None if self.output.is_some() => Line::from(ui::hints(&[
                ("pgup/pgdn", "scroll output"),
                ("esc", "commands"),
                ("ctrl+c", "quit"),
            ])),
            None => Line::from(ui::hints(&[("esc", "commands"), ("ctrl+c", "quit")])),
        };

        frame.render_widget(Paragraph::new(Line::from(left)), area);
        frame.render_widget(Paragraph::new(right.right_aligned()), area);
    }
}

impl Screen for Editor {
    fn draw(&mut self, frame: &mut Frame) {
        if let Some(output) = &mut self.output {
            output.poll();
        }

        let bottom = if self.command_bar.is_some() { 3 } else { 1 };
        let [main_area, bottom_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(bottom)]).areas(frame.area());

        let (editor_area, output_area) = match self.output {
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

        if let (Some(area), Some(output)) = (output_area, &mut self.output) {
            output.draw(frame, area);
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

        if let Some(output) = &mut self.output {
            match &event {
                Event::Key(key) if key.code == KeyCode::PageUp => {
                    output.scroll().page_up();
                    return Ok(ScreenAction::None);
                }
                Event::Key(key) if key.code == KeyCode::PageDown => {
                    output.scroll().page_down();
                    return Ok(ScreenAction::None);
                }
                Event::Mouse(mouse)
                    if output.area.contains(Position::new(mouse.column, mouse.row)) =>
                {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => output.scroll().up(MOUSE_SCROLL_LINES),
                        MouseEventKind::ScrollDown => output.scroll().down(MOUSE_SCROLL_LINES),
                        _ => {}
                    }
                    return Ok(ScreenAction::None);
                }
                _ => {}
            }
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
        self.output = None;
        let _ = fs::remove_dir_all(&self.workdir);
    }
}
