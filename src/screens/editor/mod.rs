use std::{
    env, fs, io,
    path::{Path, PathBuf},
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
use ratatui_code_editor::{actions::InsertText, editor::Editor as CodeEditor, theme::vesper};
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
        args: "[args]",
        description: "Run the file with arguments",
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
    /// The whole screen as last drawn, used to size the program's terminal.
    screen: Rect,
    runner: &'static Runner,
    binary: PathBuf,
    /// Temporary directory the file is written to before running.
    workdir: PathBuf,
    /// Where the file was last saved.
    path: Option<PathBuf>,
    /// Arguments for the program; ctrl+r reuses the last ones.
    args: Vec<String>,
    /// The text as last saved (or the starter code), to tell whether there are
    /// unsaved changes.
    saved: String,
    saved_chars: usize,
    /// Recomputed only when the text may have changed, not on every frame.
    dirty: bool,
    output: Option<Output>,
    command_bar: Option<CommandBar>,
    toast: Option<Toast>,
    /// Shown when leaving with unsaved changes.
    leaving: Option<(Leave, UnsavedPrompt)>,
}

impl Editor {
    pub fn new(runner: &'static Runner, binary: PathBuf) -> Result<Self> {
        let mut editor = CodeEditor::new(runner.syntax, runner.template, vesper())?;
        editor.set_cursor(starter_cursor(runner.template));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.subsec_nanos());
        let workdir = env::temp_dir().join(format!("scratch-{}-{nanos}", process::id()));

        Ok(Self {
            editor,
            editor_area: Rect::default(),
            screen: Rect::new(0, 0, 80, 24),
            runner,
            binary,
            workdir,
            path: None,
            args: Vec::new(),
            saved: runner.template.to_string(),
            saved_chars: runner.template.chars().count(),
            dirty: false,
            output: None,
            command_bar: None,
            toast: None,
            leaving: None,
        })
    }

    fn execute(&mut self, name: &str, args: &str) -> ScreenAction {
        match name {
            "run" => match utils::split_args(args) {
                Ok(args) => {
                    self.args = args;
                    self.run();
                }
                Err(err) => self.toast = Some(Toast::error(format!("Couldn't run: {err}"))),
            },
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
        self.dirty
    }

    /// Call after anything that may have edited the text.
    fn refresh_dirty(&mut self) {
        // Comparing lengths is O(1); the full comparison only runs when they match.
        self.dirty = self.editor.code_ref().len() != self.saved_chars
            || self.editor.get_content() != self.saved;
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

        let file = utils::display_home(&self.save_path());
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
        command.args(&self.args).current_dir(&self.workdir);

        // Start at the size the output panel is about to have.
        let (_, panel, _) = layout(self.screen, 1, true);
        let size = Output::terminal_size(panel.unwrap_or(self.screen));

        self.output = Some(match Process::spawn(command, size) {
            Ok(process) => Output::started(process, &self.workdir, &self.args),
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

    /// Where `save <args>` writes: the given path (`~` expanded, and a folder
    /// meaning `scratch.<ext>` inside it), or the default.
    fn target_path(&self, args: &str) -> Result<PathBuf, String> {
        let [path] = utils::split_args(args)?
            .try_into()
            .map_err(|args: Vec<String>| {
                if args.is_empty() {
                    String::new()
                } else {
                    "give one path; quote it if it has spaces".to_string()
                }
            })?;

        let name = format!("scratch.{}", self.runner.extension);
        let expanded = utils::expand_home(&path);

        if path.ends_with('/') || expanded.is_dir() {
            Ok(expanded.join(name))
        } else {
            Ok(expanded)
        }
    }

    /// Returns whether the file was saved.
    fn save(&mut self, args: &str) -> bool {
        let path = if args.trim().is_empty() {
            self.save_path()
        } else {
            match self.target_path(args) {
                Ok(path) => path,
                Err(err) => {
                    self.toast = Some(Toast::error(format!("Couldn't save: {err}")));
                    return false;
                }
            }
        };

        let content = self.editor.get_content();
        let shown = utils::display_home(&path);

        match write_creating_dirs(&path, &content) {
            Ok(created) => {
                let message = match created {
                    Some(dir) => format!("Saved {shown} (created {})", utils::display_home(&dir)),
                    None => format!("Saved {shown}"),
                };
                self.toast = Some(Toast::success(message));
                self.path = Some(path);
                self.saved_chars = content.chars().count();
                self.saved = content;
                self.dirty = false;
                true
            }
            Err(err) => {
                self.toast = Some(Toast::error(format!(
                    "Couldn't save {shown}: {}",
                    describe_io_error(&err)
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
                utils::display_home(path),
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
        self.screen = frame.area();

        let bottom = if self.command_bar.is_some() { 3 } else { 1 };
        let (editor_area, output_area, bottom_area) =
            layout(frame.area(), bottom, self.output.is_some());

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

        if let Some(output) = &mut self.output
            && output.is_focused()
            && let Event::Paste(text) = &event
        {
            output.paste(text);
            return Ok(ScreenAction::None);
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
            let key = match event {
                Event::Key(key) => key,
                Event::Paste(text) => {
                    command_bar.paste(&text);
                    return Ok(ScreenAction::None);
                }
                _ => return Ok(ScreenAction::None),
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
            Event::Key(key) => {
                self.editor.input(key, &self.editor_area)?;
                self.refresh_dirty();
            }
            Event::Mouse(mouse) => {
                if let MouseEventKind::Down(_) = mouse.kind
                    && let Some(output) = &mut self.output
                {
                    output.set_focus(false);
                }

                self.editor.mouse(mouse, &self.editor_area)?
            }
            Event::Paste(text) => {
                // Inserted as-is (no auto-indent), as one undo step.
                let text = utils::normalize_newlines(&text);
                self.editor.apply(InsertText { text });
                self.refresh_dirty();
            }
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

/// Writes `content` to `path`, creating missing parent folders. Returns the
/// topmost folder it had to create, if any.
fn write_creating_dirs(path: &Path, content: &str) -> io::Result<Option<PathBuf>> {
    let mut created = None;

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // The topmost missing ancestor is what gets created.
        created = parent
            .ancestors()
            .take_while(|dir| !dir.as_os_str().is_empty() && !dir.exists())
            .last()
            .map(Path::to_path_buf);
        fs::create_dir_all(parent)?;
    }

    fs::write(path, content)?;
    Ok(created)
}

/// Explains an I/O error in plain words, without the OS error number.
fn describe_io_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::IsADirectory => "that's a folder".to_string(),
        io::ErrorKind::NotADirectory => "part of the path is a file, not a folder".to_string(),
        io::ErrorKind::ReadOnlyFilesystem => "the disk is read-only".to_string(),
        io::ErrorKind::StorageFull => "the disk is full".to_string(),
        _ => {
            let message = err.to_string();
            match message.find(" (os error") {
                Some(end) => message[..end].to_string(),
                None => message,
            }
        }
    }
}

/// Splits the screen into the editor, the output panel (when shown) and the bottom
/// bar, `bottom` rows tall.
fn layout(area: Rect, bottom: u16, with_output: bool) -> (Rect, Option<Rect>, Rect) {
    let [main, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bottom)]).areas(area);

    if !with_output {
        return (main, None, bottom);
    }

    let [editor, output] =
        Layout::vertical([Constraint::Percentage(60), Constraint::Min(5)]).areas(main);
    (editor, Some(output), bottom)
}

/// Puts the cursor at the end of the starter code's greeting line, ready to edit.
fn starter_cursor(template: &str) -> usize {
    let Some(start) = template.find("Hello") else {
        return 0;
    };
    let end = template[start..]
        .find('\n')
        .map_or(template.len(), |end| start + end);
    template[..end].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::RUNNERS;

    #[test]
    fn starter_cursor_lands_after_the_greeting() {
        let template = "fn main() {\n    println!(\"Hello, world!\");\n}\n";
        let cursor = starter_cursor(template);
        assert_eq!(
            &template[..cursor],
            "fn main() {\n    println!(\"Hello, world!\");"
        );
    }

    #[test]
    fn starter_cursor_counts_chars_not_bytes() {
        assert_eq!(starter_cursor("é Hello\n"), 7);
    }

    #[test]
    fn every_template_places_the_cursor_on_its_greeting_line() {
        for runner in RUNNERS {
            let cursor = starter_cursor(runner.template);
            let before: String = runner.template.chars().take(cursor).collect();
            let line = before.lines().last().unwrap_or_default();
            assert!(line.contains("Hello, world!"), "{}", runner.name);
        }
    }

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("scratch-test-{name}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn python_editor() -> Editor {
        let python = RUNNERS
            .iter()
            .find(|runner| runner.name == "Python")
            .unwrap();
        Editor::new(python, PathBuf::from("/usr/bin/python3")).unwrap()
    }

    #[test]
    fn saving_creates_missing_folders_and_reports_the_topmost() {
        let dir = TempDir::new("save-dirs");
        let path = dir.0.join("a/b/c.py");

        let created = write_creating_dirs(&path, "x").unwrap();
        assert_eq!(created, Some(dir.0.join("a")));
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");

        // Nothing new to create the second time.
        assert_eq!(write_creating_dirs(&path, "y").unwrap(), None);
    }

    #[test]
    fn describes_common_save_errors_plainly() {
        let dir = TempDir::new("save-errors");
        let file = dir.0.join("file");
        fs::write(&file, "").unwrap();

        let under_file = write_creating_dirs(&file.join("x.py"), "").unwrap_err();
        assert!(!describe_io_error(&under_file).contains("os error"));

        let onto_dir = fs::write(&dir.0, "").unwrap_err();
        assert_eq!(describe_io_error(&onto_dir), "that's a folder");
    }

    #[test]
    fn save_targets_expand_home_and_folders() {
        let editor = python_editor();
        let dir = TempDir::new("save-targets");
        let home = PathBuf::from(env::var_os("HOME").unwrap());

        assert_eq!(editor.target_path("~/x.py").unwrap(), home.join("x.py"));
        assert_eq!(
            editor.target_path("new/").unwrap(),
            Path::new("new/scratch.py")
        );
        assert_eq!(
            editor.target_path(&dir.0.display().to_string()).unwrap(),
            dir.0.join("scratch.py")
        );
        assert_eq!(
            editor.target_path("'my file.py'").unwrap(),
            Path::new("my file.py")
        );
    }

    #[test]
    fn save_targets_reject_several_paths() {
        let editor = python_editor();
        assert!(editor.target_path("my file.py").is_err());
        assert!(editor.target_path("'unfinished").is_err());
    }
}
