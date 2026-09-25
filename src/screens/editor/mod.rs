use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
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
    runners::{self, Detection, Process, Runner},
    ui::{
        self, ACCENT, CommandBar, CommandEvent, CommandSpec, MUTED, ON_ACCENT, SUBTLE, Toast,
        UnsavedChoice, UnsavedPrompt,
    },
    utils,
};

mod draft;
mod files;
mod output;

use draft::Draft;
use files::{describe_io_error, write_creating_dirs};
use output::Output;

const MOUSE_SCROLL_LINES: usize = 3;

/// Drafts are kept this long after the last edit.
const DRAFT_DELAY: Duration = Duration::from_secs(1);

/// The output panel's share of the screen, in percent.
const DEFAULT_PANEL_PERCENT: u16 = 40;
const MIN_PANEL_PERCENT: u16 = 10;
const MAX_PANEL_PERCENT: u16 = 90;
const PANEL_STEP_PERCENT: u16 = 10;

/// Rows always left for the output panel and (unless zoomed) the editor.
const MIN_OUTPUT_ROWS: u16 = 5;
const MIN_EDITOR_ROWS: u16 = 3;

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
        name: "open",
        args: "<path>",
        description: "Open a file",
        keys: "",
    },
    CommandSpec {
        name: "new",
        args: "",
        description: "Start over from the starter code",
        keys: "",
    },
    CommandSpec {
        name: "zoom",
        args: "",
        description: "Toggle a full-height output panel",
        keys: "ctrl+↑/↓",
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

/// Something that replaces the current buffer, which may need asking first.
enum Leave {
    Back,
    Quit,
    New,
    Open(Box<Editor>),
}

impl Leave {
    /// For the prompt: "Save changes to x before …?"
    fn description(&self) -> &'static str {
        match self {
            Leave::Back | Leave::Quit => "leaving",
            Leave::New => "starting over",
            Leave::Open(_) => "opening another file",
        }
    }
}

/// How the output panel is sized.
#[derive(Clone, Copy)]
struct Panel {
    percent: u16,
    zoomed: bool,
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
    /// The file this buffer belongs to, once saved or opened.
    path: Option<PathBuf>,
    /// Arguments for the program; ctrl+r reuses the last ones.
    args: Vec<String>,
    /// The file's text as last saved (or the starter code), to tell whether there
    /// are unsaved changes.
    saved: String,
    saved_chars: usize,
    /// Recomputed only when the text may have changed, not on every frame.
    dirty: bool,
    /// When the buffer was last edited, until its draft is kept.
    edited: Option<Instant>,
    output: Option<Output>,
    panel: Panel,
    /// Whether the panel's top border is being dragged.
    resizing: bool,
    command_bar: Option<CommandBar>,
    toast: Option<Toast>,
    /// Shown when unsaved changes are about to be replaced.
    leaving: Option<(Leave, UnsavedPrompt)>,
}

impl Editor {
    /// An editor for `runner`, with its draft from last time or the starter code.
    pub fn new(runner: &'static Runner, binary: PathBuf) -> Result<Self> {
        let Some(draft) = Draft::load(runner) else {
            return Self::with_text(
                runner,
                binary,
                runner.template,
                starter_cursor(runner.template),
                None,
            );
        };

        let mut editor = Self::with_text(runner, binary, &draft.content, draft.cursor, None)?;
        editor.path = draft.path;

        // Changes count against the file as it is now (or the starter code).
        editor.saved = match &editor.path {
            Some(path) => fs::read_to_string(path).unwrap_or_default(),
            None => runner.template.to_string(),
        };
        editor.saved_chars = editor.saved.chars().count();
        editor.refresh_dirty();
        Ok(editor)
    }

    /// An editor for the file at `args` (one path). Its language comes from the
    /// extension, or `fallback` when that's unknown.
    pub fn open(args: &str, fallback: Option<&'static Runner>) -> Result<Self, String> {
        let [path] = utils::split_args(args)?
            .try_into()
            .map_err(|args: Vec<String>| {
                if args.is_empty() {
                    "give a path to open".to_string()
                } else {
                    "give one path; quote it if it has spaces".to_string()
                }
            })?;

        let path = utils::expand_home(&path);
        let shown = utils::display_path(&path);
        let content = files::read_text(&path).map_err(|err| format!("{shown}: {err}"))?;

        let detected = runners::for_path(&path);
        let runner = detected
            .or(fallback)
            .ok_or_else(|| format!("{shown}: unknown file type; pick a language first"))?;

        let binary = match runners::detect_one(runner) {
            Detection::Found(binary) => binary,
            Detection::Missing => return Err(format!("{} isn't installed", runner.name)),
            Detection::Placeholder(hint) => return Err(format!("{}: {hint}", runner.name)),
        };

        let path = std::path::absolute(&path).unwrap_or(path);
        let mut editor = Self::with_text(runner, binary, &content, 0, Some(path))
            .map_err(|err| err.to_string())?;

        let message = match detected {
            Some(_) => format!("Opened {shown}"),
            None => format!("Opened {shown} as {}", runner.name),
        };
        editor.toast = Some(Toast::success(message));
        Ok(editor)
    }

    /// An editor showing `text`, which is also what's on disk at `path`.
    fn with_text(
        runner: &'static Runner,
        binary: PathBuf,
        text: &str,
        cursor: usize,
        path: Option<PathBuf>,
    ) -> Result<Self> {
        let mut editor = CodeEditor::new(runner.syntax, text, vesper())?;
        editor.set_cursor(cursor);

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
            path,
            args: Vec::new(),
            saved: text.to_string(),
            saved_chars: text.chars().count(),
            dirty: false,
            edited: None,
            output: None,
            panel: Panel {
                percent: DEFAULT_PANEL_PERCENT,
                zoomed: false,
            },
            resizing: false,
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
            "open" => match Self::open(args, Some(self.runner)) {
                Ok(editor) => return self.leave(Leave::Open(Box::new(editor))),
                Err(err) => self.toast = Some(Toast::error(format!("Couldn't open {err}"))),
            },
            "new" => return self.leave(Leave::New),
            "zoom" => match self.output {
                Some(_) => self.panel.zoomed = !self.panel.zoomed,
                None => self.toast = Some(Toast::error("Nothing to zoom; run the file first")),
            },
            "clear" => self.output = None,
            "back" => return self.leave(Leave::Back),
            "exit" => return self.leave(Leave::Quit),
            _ => {}
        }

        ScreenAction::None
    }

    /// Whether the buffer differs from its file. An untitled buffer never needs
    /// saving to be kept: its draft is.
    fn has_unsaved_file_changes(&self) -> bool {
        self.path.is_some() && self.dirty
    }

    /// Call after anything that may have edited the text.
    fn edited(&mut self) {
        // Comparing lengths is O(1); the full comparison only runs when they match.
        self.dirty = self.editor.code_ref().len() != self.saved_chars
            || self.editor.get_content() != self.saved;
        self.edited = Some(Instant::now());
    }

    fn refresh_dirty(&mut self) {
        self.edited();
        self.edited = None;
    }

    fn draft(&self) -> Draft {
        Draft {
            content: self.editor.get_content(),
            cursor: self.editor.get_cursor(),
            path: self
                .path
                .as_ref()
                .map(|path| std::path::absolute(path).unwrap_or_else(|_| path.clone())),
        }
    }

    fn keep_draft(&mut self) {
        self.edited = None;

        if let Err(err) = self.draft().store(self.runner) {
            self.toast = Some(Toast::error(format!(
                "Couldn't keep a draft: {}",
                describe_io_error(&err)
            )));
        }
    }

    /// Where `save` without a path writes to.
    fn save_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("scratch.{}", self.runner.extension)))
    }

    /// Replaces the buffer right away, or asks first when that would lose changes.
    fn leave(&mut self, leave: Leave) -> ScreenAction {
        // Leaving keeps an untitled buffer's draft; starting over or opening
        // another file of this language replaces it.
        let ask = match leave {
            Leave::Back | Leave::Quit => self.has_unsaved_file_changes(),
            Leave::New | Leave::Open(_) => self.dirty,
        };

        if !ask {
            return self.finish(leave);
        }

        let file = utils::display_path(&self.save_path());
        let action = leave.description();
        self.command_bar = None;
        self.leaving = Some((leave, UnsavedPrompt { file, action }));
        ScreenAction::None
    }

    fn finish(&mut self, leave: Leave) -> ScreenAction {
        match leave {
            Leave::Back => ScreenAction::Pop,
            Leave::Quit => ScreenAction::Quit,
            Leave::Open(editor) => ScreenAction::Replace(editor),
            Leave::New => {
                self.editor.set_content(self.runner.template);
                self.editor.set_cursor(starter_cursor(self.runner.template));
                self.path = None;
                self.saved = self.runner.template.to_string();
                self.saved_chars = self.saved.chars().count();
                self.dirty = false;
                self.keep_draft();
                self.toast = Some(Toast::success(format!("New {} file", self.runner.name)));
                ScreenAction::None
            }
        }
    }

    /// Puts the buffer back to its file, so the draft kept on leaving has no changes.
    fn discard_changes(&mut self) {
        let saved = self.saved.clone();
        self.editor.set_content(&saved);
        self.dirty = false;
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
        let (_, panel, _) = layout(self.screen, 1, Some(self.panel));
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
        let shown = utils::display_path(&path);

        match write_creating_dirs(&path, &content) {
            Ok(created) => {
                let message = match created {
                    Some(dir) => format!("Saved {shown} (created {})", utils::display_path(&dir)),
                    None => format!("Saved {shown}"),
                };
                self.toast = Some(Toast::success(message));
                self.path = Some(path);
                self.saved_chars = content.chars().count();
                self.saved = content;
                self.dirty = false;
                self.keep_draft();
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

    /// Grows (or shrinks, for negative `steps`) the output panel.
    fn resize_panel(&mut self, steps: i32) {
        let percent = i32::from(self.panel.percent) + steps * i32::from(PANEL_STEP_PERCENT);
        self.panel.percent =
            percent.clamp(i32::from(MIN_PANEL_PERCENT), i32::from(MAX_PANEL_PERCENT)) as u16;
        self.panel.zoomed = false;
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let mut left = vec![
            Span::styled(
                format!(" {} ", self.runner.name),
                Style::new().fg(ON_ACCENT).bg(ACCENT).bold(),
            ),
            Span::raw(" "),
        ];

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

        // The path gets whatever room is left, shortened from the start if needed.
        let used = Line::from(left.clone()).width() + right.width() + 4;
        let room = usize::from(area.width).saturating_sub(used);

        match &self.path {
            Some(path) => left.push(Span::styled(
                utils::truncate_start(&utils::display_path(path), room),
                Style::new().fg(SUBTLE),
            )),
            None => left.push(Span::styled("untitled", Style::new().fg(MUTED).italic())),
        }

        if self.has_unsaved_file_changes() {
            left.push(Span::styled(" ●", Style::new().fg(ACCENT)));
        }

        frame.render_widget(Paragraph::new(Line::from(left)), area);
        frame.render_widget(Paragraph::new(right.right_aligned()), area);
    }

    /// The unsaved-changes prompt takes every key while it's open.
    fn handle_prompt(&mut self, event: Event) -> ScreenAction {
        // ctrl+c again quits without saving.
        if utils::is_ctrl_c(&event) {
            self.discard_changes();
            return ScreenAction::Quit;
        }

        let Event::Key(key) = event else {
            return ScreenAction::None;
        };
        let Some((_, prompt)) = &self.leaving else {
            return ScreenAction::None;
        };

        match prompt.handle(key) {
            None => ScreenAction::None,
            Some(UnsavedChoice::Cancel) => {
                self.leaving = None;
                ScreenAction::None
            }
            Some(UnsavedChoice::Discard) => {
                let (leave, _) = self.leaving.take().expect("prompt is open");
                self.discard_changes();
                self.finish(leave)
            }
            Some(UnsavedChoice::Save) => {
                let (leave, _) = self.leaving.take().expect("prompt is open");

                // On failure, stay; the error is shown in the status bar.
                if self.save("") {
                    self.finish(leave)
                } else {
                    ScreenAction::None
                }
            }
        }
    }

    /// While the output panel has focus, keys and pastes go to the program. Returns
    /// `None` for keys handled like anywhere else (scrolling, run, save, resize).
    fn handle_focused_output(&mut self, event: &Event) -> Option<ScreenAction> {
        let output = self.output.as_mut().filter(|output| output.is_focused())?;

        let key = match event {
            Event::Paste(text) => {
                output.paste(text);
                return Some(ScreenAction::None);
            }
            Event::Key(key) => key,
            _ => return None,
        };

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => output.interrupt(),
            KeyCode::Char('d') if ctrl => output.end_input(),
            KeyCode::Char('o') if ctrl => output.set_focus(false),
            KeyCode::Esc => output.set_focus(false),
            KeyCode::Enter => output.submit(),
            KeyCode::PageUp | KeyCode::PageDown => return None,
            KeyCode::Up | KeyCode::Down if ctrl => return None,
            KeyCode::Char('r' | 's') if ctrl => return None,
            _ => output.input(*key),
        }

        Some(ScreenAction::None)
    }

    fn handle_command_bar(&mut self, event: Event) -> ScreenAction {
        let Some(command_bar) = &mut self.command_bar else {
            return ScreenAction::None;
        };

        let key = match event {
            Event::Key(key) => key,
            Event::Paste(text) => {
                command_bar.paste(&text);
                return ScreenAction::None;
            }
            _ => return ScreenAction::None,
        };

        match command_bar.handle(key) {
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
        }
    }

    /// Mouse events on the output panel: scrolling, focusing, and dragging its top
    /// border to resize it. Returns whether the event was used.
    fn handle_panel_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(output) = &mut self.output else {
            return false;
        };

        if self.resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    // The panel spans from the pointer to the bottom bar.
                    let main = layout(self.screen, 1, None).0;
                    let rows = main.bottom().saturating_sub(mouse.row);
                    let percent = u32::from(rows) * 100 / u32::from(main.height.max(1));
                    self.panel.percent =
                        (percent as u16).clamp(MIN_PANEL_PERCENT, MAX_PANEL_PERCENT);
                    self.panel.zoomed = false;
                }
                MouseEventKind::Up(_) => self.resizing = false,
                _ => {}
            }
            return true;
        }

        if !output.area.contains(Position::new(mouse.column, mouse.row)) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => output.scroll().up(MOUSE_SCROLL_LINES),
            MouseEventKind::ScrollDown => output.scroll().down(MOUSE_SCROLL_LINES),
            MouseEventKind::Down(MouseButton::Left) if mouse.row == output.area.y => {
                self.resizing = true;
            }
            MouseEventKind::Down(MouseButton::Left) => output.set_focus(true),
            _ => {}
        }
        true
    }

    fn handle_editor(&mut self, event: Event) -> Result<ScreenAction> {
        match event {
            Event::Key(key) => return self.handle_key(key),
            Event::Mouse(mouse) => {
                if self.handle_panel_mouse(mouse) {
                    return Ok(ScreenAction::None);
                }

                if let MouseEventKind::Down(_) = mouse.kind
                    && let Some(output) = &mut self.output
                {
                    output.set_focus(false);
                }
                self.editor.mouse(mouse, &self.editor_area)?;
            }
            Event::Paste(text) => {
                // Inserted as-is (no auto-indent), as one undo step.
                let text = utils::normalize_newlines(&text);
                self.editor.apply(InsertText { text });
                self.edited();
            }
            _ => {}
        }

        Ok(ScreenAction::None)
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<ScreenAction> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => self.command_bar = Some(CommandBar::new(COMMANDS)),
            KeyCode::Char('o') if ctrl => {
                if let Some(output) = &mut self.output {
                    output.set_focus(true);
                }
            }
            KeyCode::Char('r') if ctrl => self.run(),
            KeyCode::Char('s') if ctrl => {
                self.save("");
            }
            KeyCode::Up if ctrl && self.output.is_some() => self.resize_panel(1),
            KeyCode::Down if ctrl && self.output.is_some() => self.resize_panel(-1),
            KeyCode::PageUp | KeyCode::PageDown if self.output.is_some() => {
                if let Some(output) = &mut self.output {
                    match key.code {
                        KeyCode::PageUp => output.scroll().page_up(),
                        _ => output.scroll().page_down(),
                    }
                }
            }
            _ => {
                self.editor.input(key, &self.editor_area)?;
                self.edited();
            }
        }

        Ok(ScreenAction::None)
    }
}

impl Screen for Editor {
    fn update(&mut self) -> Result<ScreenAction> {
        if let Some(output) = &mut self.output {
            output.poll();
        }

        if self
            .edited
            .is_some_and(|edited| edited.elapsed() >= DRAFT_DELAY)
        {
            self.keep_draft();
        }

        Ok(ScreenAction::None)
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.screen = frame.area();

        let bottom = if self.command_bar.is_some() { 3 } else { 1 };
        let panel = self.output.as_ref().map(|_| self.panel);
        let (editor_area, output_area, bottom_area) = layout(frame.area(), bottom, panel);

        self.editor_area = editor_area;
        frame.render_widget(&self.editor, self.editor_area);

        if let (Some(area), Some(output)) = (output_area, &mut self.output) {
            output.set_resizing(self.resizing);
            output.draw(frame, area);
        }

        match &self.command_bar {
            Some(command_bar) => command_bar.render(frame, bottom_area),
            None => {
                self.draw_status(frame, bottom_area);

                let output_focused = self.output.as_ref().is_some_and(Output::is_focused);

                if self.leaving.is_none()
                    && !output_focused
                    && editor_area.height > 0
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
        if self.leaving.is_some() {
            return Ok(self.handle_prompt(event));
        }

        if let Some(action) = self.handle_focused_output(&event) {
            return Ok(action);
        }

        // ctrl+c copies when there's a selection, otherwise quits.
        let has_selection = self
            .editor
            .get_selection()
            .is_some_and(|selection| selection.is_active());

        if (self.command_bar.is_some() || !has_selection) && utils::is_ctrl_c(&event) {
            return Ok(self.leave(Leave::Quit));
        }

        if self.command_bar.is_some() {
            return Ok(self.handle_command_bar(event));
        }

        self.handle_editor(event)
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.output = None;
        let _ = fs::remove_dir_all(&self.workdir);

        // Nothing to report to anymore; a failed write is only noticed next time.
        let _ = self.draft().store(self.runner);
    }
}

/// Splits the screen into the editor, the output panel (when shown) and the bottom
/// bar, `bottom` rows tall.
fn layout(area: Rect, bottom: u16, panel: Option<Panel>) -> (Rect, Option<Rect>, Rect) {
    let [main, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bottom)]).areas(area);

    let Some(panel) = panel else {
        return (main, None, bottom);
    };

    let wanted = if panel.zoomed {
        main.height
    } else {
        (u32::from(main.height) * u32::from(panel.percent) / 100) as u16
    };
    let max = if panel.zoomed {
        main.height
    } else {
        main.height.saturating_sub(MIN_EDITOR_ROWS)
    };
    let min = MIN_OUTPUT_ROWS.min(main.height);
    let rows = wanted.min(max).max(min);

    let [editor, output] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(rows)]).areas(main);
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
    use std::path::Path;

    use super::*;
    use crate::{runners::RUNNERS, screens::editor::files::tests::TempDir};

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

    fn python_editor() -> Editor {
        let python = RUNNERS
            .iter()
            .find(|runner| runner.name == "Python")
            .unwrap();
        Editor::new(python, PathBuf::from("/usr/bin/python3")).unwrap()
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

    fn runner(name: &str) -> &'static Runner {
        RUNNERS.iter().find(|runner| runner.name == name).unwrap()
    }

    fn panel(percent: u16, zoomed: bool) -> Option<Panel> {
        Some(Panel { percent, zoomed })
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(editor: &mut Editor, text: &str) {
        for c in text.chars() {
            editor.handle_key(key(KeyCode::Char(c))).unwrap();
        }
    }

    #[test]
    fn panel_takes_its_share_of_the_screen() {
        let screen = Rect::new(0, 0, 80, 41);
        let (editor, output, bottom) = layout(screen, 1, panel(40, false));
        assert_eq!(output.unwrap().height, 16);
        assert_eq!(editor.height, 24);
        assert_eq!(bottom.height, 1);
    }

    #[test]
    fn zooming_hides_the_editor() {
        let screen = Rect::new(0, 0, 80, 41);
        let (editor, output, _) = layout(screen, 1, panel(40, true));
        assert_eq!(editor.height, 0);
        assert_eq!(output.unwrap().height, 40);
    }

    #[test]
    fn panel_keeps_minimum_sizes() {
        let screen = Rect::new(0, 0, 80, 21);
        // The editor keeps a few rows, the panel at least five.
        assert_eq!(
            layout(screen, 1, panel(90, false)).0.height,
            MIN_EDITOR_ROWS
        );
        assert_eq!(
            layout(screen, 1, panel(10, false)).1.unwrap().height,
            MIN_OUTPUT_ROWS
        );

        // Tiny screens don't panic.
        for height in 0..8 {
            layout(Rect::new(0, 0, 10, height), 3, panel(50, false));
        }
    }

    #[test]
    fn resizing_the_panel_steps_within_limits() {
        let mut editor = python_editor();
        editor.panel.zoomed = true;

        editor.resize_panel(1);
        assert_eq!(
            editor.panel.percent,
            DEFAULT_PANEL_PERCENT + PANEL_STEP_PERCENT
        );
        assert!(!editor.panel.zoomed);

        editor.resize_panel(100);
        assert_eq!(editor.panel.percent, MAX_PANEL_PERCENT);
        editor.resize_panel(-100);
        assert_eq!(editor.panel.percent, MIN_PANEL_PERCENT);
    }

    #[test]
    fn leaving_an_untitled_buffer_keeps_its_draft_without_asking() {
        let mut editor = python_editor();
        type_text(&mut editor, "x");
        assert!(editor.dirty);
        assert!(!editor.has_unsaved_file_changes());
        assert!(matches!(editor.leave(Leave::Back), ScreenAction::Pop));
    }

    #[test]
    fn starting_over_asks_before_replacing_changes() {
        let mut editor = python_editor();
        type_text(&mut editor, "x");
        assert!(matches!(editor.leave(Leave::New), ScreenAction::None));
        assert!(editor.leaving.is_some());

        // Discarding starts over from the starter code.
        editor.handle_prompt(Event::Key(key(KeyCode::Char('d'))));
        assert_eq!(editor.editor.get_content(), runner("Python").template);
        assert!(!editor.dirty);
    }

    #[test]
    fn leaving_a_changed_file_asks_first() {
        let dir = TempDir::new("leave-file");
        let file = dir.0.join("a.py");
        fs::write(&file, "print(1)\n").unwrap();

        let mut editor = Editor::with_text(
            runner("Python"),
            PathBuf::from("/usr/bin/python3"),
            "print(1)\n",
            0,
            Some(file),
        )
        .unwrap();
        assert!(matches!(editor.leave(Leave::Back), ScreenAction::Pop));

        type_text(&mut editor, "x");
        assert!(editor.has_unsaved_file_changes());
        assert!(matches!(editor.leave(Leave::Back), ScreenAction::None));
        assert_eq!(editor.leaving.as_ref().unwrap().1.action, "leaving");

        // Discarding puts the file's text back, so the kept draft has no changes.
        let action = editor.handle_prompt(Event::Key(key(KeyCode::Char('d'))));
        assert!(matches!(action, ScreenAction::Pop));
        assert_eq!(editor.editor.get_content(), "print(1)\n");
    }

    #[test]
    fn opens_files_in_the_language_of_their_extension() {
        let dir = TempDir::new("open");
        let file = dir.0.join("main.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        // Tests run with cargo, so rustc is installed.
        let editor = Editor::open(&file.display().to_string(), None).unwrap();
        assert_eq!(editor.runner.name, "Rust");
        assert_eq!(editor.editor.get_content(), "fn main() {}\n");
        assert_eq!(editor.path.as_deref(), Some(file.as_path()));
        assert!(!editor.dirty);
    }

    #[test]
    fn unknown_extensions_open_as_the_current_language() {
        let dir = TempDir::new("open-unknown");
        let file = dir.0.join("notes.txt");
        fs::write(&file, "echo hi\n").unwrap();
        let path = file.display().to_string();

        let editor = Editor::open(&path, Some(runner("Shell"))).unwrap();
        assert_eq!(editor.runner.name, "Shell");

        let err = Editor::open(&path, None).err().unwrap();
        assert!(err.contains("unknown file type"), "{err}");
    }

    #[test]
    fn open_explains_what_went_wrong() {
        let dir = TempDir::new("open-errors");
        let err = |args: &str| Editor::open(args, Some(runner("Shell"))).err().unwrap();

        assert!(err(&dir.0.join("missing.sh").display().to_string()).contains("no such file"));
        assert!(err(&dir.0.display().to_string()).contains("that's a folder"));
        assert_eq!(err(""), "give a path to open");
        assert!(err("a b").contains("give one path"));
    }
}
