use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
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
    ui::{
        self, ACCENT, CommandBar, CommandEvent, CommandSpec, MUTED, ON_ACCENT, SUBTLE, Toast,
        UnsavedChoice, UnsavedPrompt,
    },
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
        keys: "ctrl+r",
    },
    CommandSpec {
        name: "save",
        args: "[path]",
        description: "Save the file",
        keys: "ctrl+s",
    },
    CommandSpec {
        name: "clear",
        args: "",
        description: "Close the output panel",
        keys: "",
    },
    CommandSpec {
        name: "back",
        args: "",
        description: "Pick another language",
        keys: "",
    },
    CommandSpec {
        name: "exit",
        args: "",
        description: "Quit Scratch",
        keys: "ctrl+c",
    },
];

/// Where the user is going when they leave the editor.
#[derive(Clone, Copy)]
enum Leave {
    Back,
    Quit,
}

impl Leave {
    fn action(self) -> ScreenAction {
        match self {
            Leave::Back => ScreenAction::Pop,
            Leave::Quit => ScreenAction::Quit,
        }
    }
}

pub struct Editor {
    editor: CodeEditor,
    editor_area: Rect,
    /// Terminal width, used to size the program's terminal.
    width: u16,
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
    /// Shown when leaving with unsaved changes.
    leaving: Option<(Leave, UnsavedPrompt)>,
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
            width: 80,
            runner,
            binary,
            workdir,
            path: None,
            saved: String::new(),
            output: None,
            command_bar: None,
            toast: None,
            leaving: None,
        })
    }

    fn execute(&mut self, name: &str, args: &str) -> ScreenAction {
        match name {
            "run" => self.run(),
            "save" => {
                self.save(args);
            }
            "clear" => self.output = None,
            "back" => return self.leave(Leave::Back),
            "exit" => return self.leave(Leave::Quit),
            _ => {}
        }

        ScreenAction::None
    }

    fn is_dirty(&self) -> bool {
        self.editor.get_content() != self.saved
    }

    /// Where `save` without a path writes to.
    fn save_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("scratch.{}", self.runner.extension)))
    }

    /// Leaves right away, or asks first when there are unsaved changes.
    fn leave(&mut self, leave: Leave) -> ScreenAction {
        if !self.is_dirty() {
            return leave.action();
        }

        let file = self.save_path().display().to_string();
        self.command_bar = None;
        self.leaving = Some((leave, UnsavedPrompt { file }));
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

        // Borders and padding take 4 columns.
        let columns = self.width.saturating_sub(4);

        self.output = Some(match Process::spawn(command, columns) {
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

    /// Returns whether the file was saved.
    fn save(&mut self, args: &str) -> bool {
        let path = if args.is_empty() {
            self.save_path()
        } else {
            PathBuf::from(args)
        };

        let content = self.editor.get_content();

        match fs::write(&path, &content) {
            Ok(()) => {
                self.toast = Some(Toast::success(format!("Saved {}", path.display())));
                self.path = Some(path);
                self.saved = content;
                true
            }
            Err(err) => {
                self.toast = Some(Toast::error(format!(
                    "Couldn't save {}: {err}",
                    path.display()
                )));
                false
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

        if self.is_dirty() {
            left.push(Span::styled(" ●", Style::new().fg(ACCENT)));
        }

        let right = match self.toast.as_ref().filter(|toast| toast.is_visible()) {
            Some(toast) => toast.line(),
            None if self.output.as_ref().is_some_and(Output::is_focused) => {
                Line::from(ui::hints(&[
                    ("enter", "send"),
                    ("ctrl+d", "end input"),
                    ("ctrl+c", "stop"),
                    ("esc", "editor"),
                ]))
            }
            None if self.output.as_ref().is_some_and(Output::is_running) => {
                Line::from(ui::hints(&[
                    ("ctrl+o", "input"),
                    ("pgup/pgdn", "scroll"),
                    ("esc", "commands"),
                    ("ctrl+c", "quit"),
                ]))
            }
            None if self.output.is_some() => Line::from(ui::hints(&[
                ("pgup/pgdn", "scroll"),
                ("ctrl+r", "run"),
                ("esc", "commands"),
                ("ctrl+c", "quit"),
            ])),
            None => Line::from(ui::hints(&[
                ("ctrl+r", "run"),
                ("ctrl+s", "save"),
                ("esc", "commands"),
                ("ctrl+c", "quit"),
            ])),
        };

        frame.render_widget(Paragraph::new(Line::from(left)), area);
        frame.render_widget(Paragraph::new(right.right_aligned()), area);
    }
}

impl Screen for Editor {
    fn update(&mut self) -> Result<ScreenAction> {
        if let Some(output) = &mut self.output {
            output.poll();
        }

        Ok(ScreenAction::None)
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.width = frame.area().width;

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

                let output_focused = self.output.as_ref().is_some_and(Output::is_focused);

                if self.leaving.is_none()
                    && !output_focused
                    && let Some((x, y)) = self.editor.get_visible_cursor(&self.editor_area)
                {
                    frame.set_cursor_position(Position::new(x, y));
                }
            }
        }

        if let Some((_, prompt)) = &self.leaving {
            prompt.render(frame, frame.area());
        }
    }

    fn handle(&mut self, event: Event) -> Result<ScreenAction> {
        if let Some((leave, prompt)) = &self.leaving {
            let leave = *leave;

            // ctrl+c again quits without saving.
            if utils::is_ctrl_c(&event) {
                return Ok(ScreenAction::Quit);
            }

            let Event::Key(key) = event else {
                return Ok(ScreenAction::None);
            };

            return Ok(match prompt.handle(key) {
                None => ScreenAction::None,
                Some(UnsavedChoice::Cancel) => {
                    self.leaving = None;
                    ScreenAction::None
                }
                Some(UnsavedChoice::Discard) => leave.action(),
                Some(UnsavedChoice::Save) => {
                    self.leaving = None;

                    // On failure, stay; the error is shown in the status bar.
                    if self.save("") {
                        leave.action()
                    } else {
                        ScreenAction::None
                    }
                }
            });
        }

        // While the output panel has focus, keys go to the program.
        if let Some(output) = &mut self.output
            && output.is_focused()
            && let Event::Key(key) = &event
        {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

            match key.code {
                KeyCode::Char('c') if ctrl => output.interrupt(),
                KeyCode::Char('d') if ctrl => output.end_input(),
                KeyCode::Char('o') if ctrl => output.set_focus(false),
                KeyCode::Esc => output.set_focus(false),
                KeyCode::Enter => output.submit(),
                // Handled below: scrolling, run and save.
                KeyCode::PageUp | KeyCode::PageDown => {}
                KeyCode::Char('r' | 's') if ctrl => {}
                _ => output.input(*key),
            }

            let passthrough = matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                || (ctrl && matches!(key.code, KeyCode::Char('r' | 's')));
            if !passthrough {
                return Ok(ScreenAction::None);
            }
        }

        // ctrl+c copies when there's a selection, otherwise quits.
        let has_selection = self
            .editor
            .get_selection()
            .is_some_and(|selection| selection.is_active());

        if (self.command_bar.is_some() || !has_selection) && utils::is_ctrl_c(&event) {
            return Ok(self.leave(Leave::Quit));
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
                        MouseEventKind::Down(MouseButton::Left) => output.set_focus(true),
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
            Event::Key(key)
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('o') =>
            {
                if let Some(output) = &mut self.output {
                    output.set_focus(true);
                }
            }
            Event::Key(key)
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('r') =>
            {
                self.run()
            }
            Event::Key(key)
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('s') =>
            {
                self.save("");
            }
            Event::Key(key) => self.editor.input(key, &self.editor_area)?,
            Event::Mouse(mouse) => {
                if let MouseEventKind::Down(_) = mouse.kind
                    && let Some(output) = &mut self.output
                {
                    output.set_focus(false);
                }

                self.editor.mouse(mouse, &self.editor_area)?
            }
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
