use std::{borrow::Cow, path::Path, process::ExitStatus};

use ansi_to_tui::IntoText;
use crossterm::event::KeyEvent;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    symbols::scrollbar,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        Wrap,
    },
};

use crate::{
    runners::{Process, RunStatus},
    ui::{self, ACCENT, ERROR, Input, MUTED, SUBTLE, SUCCESS, SURFACE, Scroll, TEXT},
};

enum Run {
    /// `workdir` is hidden from output so paths read as `main.<ext>`.
    Started {
        process: Process,
        workdir: String,
        rendered: Vec<Line<'static>>,
    },
    /// The program couldn't be started at all.
    Failed { title: String, detail: String },
}

/// The output panel for the latest run.
pub struct Output {
    run: Run,
    scroll: Scroll,
    /// Text being typed for the program's stdin.
    input: Input,
    /// Whether keys go to the program instead of the editor.
    focused: bool,
    /// Where the panel was last drawn, for mouse hit-testing.
    pub area: Rect,
}

impl Output {
    pub fn started(process: Process, workdir: &Path) -> Self {
        Self::new(Run::Started {
            process,
            workdir: format!("{}/", workdir.display()),
            rendered: Vec::new(),
        })
    }

    pub fn failed(title: impl Into<String>, detail: impl ToString) -> Self {
        Self::new(Run::Failed {
            title: title.into(),
            detail: detail.to_string(),
        })
    }

    fn new(run: Run) -> Self {
        Self {
            run,
            scroll: Scroll::default(),
            input: Input::default(),
            focused: false,
            area: Rect::default(),
        }
    }

    pub fn scroll(&mut self) -> &mut Scroll {
        &mut self.scroll
    }

    fn process(&self) -> Option<&Process> {
        match &self.run {
            Run::Started { process, .. } => Some(process),
            Run::Failed { .. } => None,
        }
    }

    fn process_mut(&mut self) -> Option<&mut Process> {
        match &mut self.run {
            Run::Started { process, .. } => Some(process),
            Run::Failed { .. } => None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.process().is_some_and(Process::is_running)
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Sends keys to the program (only while it runs) or back to the editor.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused && self.is_running();
    }

    /// Edits the pending input line.
    pub fn input(&mut self, key: KeyEvent) {
        self.input.handle(key);
    }

    /// Sends the typed line to the program.
    pub fn submit(&mut self) {
        let text = self.input.value().to_string();
        self.input.clear();

        if let Some(process) = self.process_mut() {
            process.send_line(&text);
        }
        self.scroll.follow();
    }

    /// Ends the program's input, like ctrl+d.
    pub fn end_input(&mut self) {
        if let Some(process) = self.process_mut() {
            process.send_eof();
        }
    }

    /// Interrupts the program, like ctrl+c; a second time kills it.
    pub fn interrupt(&mut self) {
        if let Some(process) = self.process_mut() {
            process.interrupt();
        }
    }

    pub fn poll(&mut self) {
        if let Run::Started {
            process,
            workdir,
            rendered,
        } = &mut self.run
        {
            process.poll();

            let new = &process.lines[rendered.len()..];
            rendered.extend(new.iter().map(|raw| render_line(raw, workdir)));
        }

        if !self.is_running() {
            self.focused = false;
        }
    }

    /// The unfinished last line, such as a prompt waiting for input.
    fn partial(&self) -> Option<Line<'static>> {
        let Run::Started {
            process, workdir, ..
        } = &self.run
        else {
            return None;
        };

        let partial = process.partial();
        (!partial.is_empty()).then(|| render_line(&partial, workdir))
    }

    fn status(&self) -> Line<'static> {
        let process = match &self.run {
            Run::Started { process, .. } => process,
            Run::Failed { .. } => {
                return Line::from(Span::styled(
                    "✗ failed to start ",
                    Style::new().fg(ERROR).add_modifier(Modifier::BOLD),
                ));
            }
        };

        match &process.status {
            RunStatus::Running => Line::from(vec![
                Span::styled(ui::spinner(process.started), Style::new().fg(ACCENT)),
                Span::styled(
                    format!(" running {:.1}s ", process.started.elapsed().as_secs_f32()),
                    Style::new().fg(SUBTLE),
                ),
            ]),
            RunStatus::Exited(status, elapsed) => {
                let (icon, color, label) = match Exit::of(status) {
                    Exit::Code(0) => ("✓", SUCCESS, "exited 0".to_string()),
                    Exit::Code(code) => ("✗", ERROR, format!("exited {code}")),
                    Exit::Interrupted => ("✗", ERROR, "interrupted".to_string()),
                    Exit::Killed => ("✗", ERROR, "killed".to_string()),
                };

                Line::from(vec![
                    Span::styled(icon, Style::new().fg(color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {label}"), Style::new().fg(color)),
                    Span::styled(
                        format!(" · {:.2}s ", elapsed.as_secs_f32()),
                        Style::new().fg(MUTED),
                    ),
                ])
            }
        }
    }

    /// Output lines, parsed once as they arrive.
    fn rendered(&self) -> &[Line<'static>] {
        match &self.run {
            Run::Started { rendered, .. } => rendered,
            Run::Failed { .. } => &[],
        }
    }

    /// Lines shown after the program's output: a placeholder, the exit summary, or
    /// the reason the program couldn't start.
    fn trailer(&self, has_partial: bool) -> Vec<Line<'static>> {
        let process = match &self.run {
            Run::Started { process, .. } => process,
            Run::Failed { title, detail } => {
                return vec![
                    Line::from(vec![
                        Span::styled("▎ ", Style::new().fg(ERROR)),
                        Span::styled(
                            title.clone(),
                            Style::new().fg(ERROR).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::default(),
                    Line::from(Span::styled(detail.clone(), Style::new().fg(SUBTLE))),
                ];
            }
        };

        let italic = Style::new().add_modifier(Modifier::ITALIC);
        let has_output = !process.lines.is_empty() || has_partial;

        match &process.status {
            RunStatus::Running if has_output => vec![],
            RunStatus::Running => vec![Line::styled("waiting for output…", italic.fg(MUTED))],
            RunStatus::Exited(status, _) => {
                let summary = match Exit::of(status) {
                    Exit::Code(0) if has_output => Line::styled("Finished", italic.fg(MUTED)),
                    Exit::Code(0) => Line::styled("Finished with no output", italic.fg(MUTED)),
                    Exit::Code(code) => {
                        Line::styled(format!("Process exited with code {code}"), italic.fg(ERROR))
                    }
                    Exit::Interrupted => Line::styled("Process was interrupted", italic.fg(ERROR)),
                    Exit::Killed => Line::styled("Process was killed", italic.fg(ERROR)),
                };

                if has_output {
                    vec![Line::default(), summary]
                } else {
                    vec![summary]
                }
            }
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.area = area;

        let border = if self.focused { ACCENT } else { MUTED };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(border))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " Output ",
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ))
            .title_top(self.status().right_aligned());

        let inner = block.inner(area);

        // While the program runs, the bottom row is its input.
        let (content, input_row) = if self.is_running() && inner.height > 1 {
            let [content, input] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
            (content, Some(input))
        } else {
            (inner, None)
        };

        let partial = self.partial();
        let trailer = self.trailer(partial.is_some());
        let total = self.rendered().len() + usize::from(partial.is_some()) + trailer.len();
        let viewport = content.height as usize;
        let offset = self.scroll.update(total, viewport);

        let hidden_below = total.saturating_sub(offset + viewport);
        let block = if hidden_below > 0 {
            block.title_bottom(
                Line::from(Span::styled(
                    format!(" ↓ {hidden_below} more "),
                    Style::new().fg(ACCENT),
                ))
                .right_aligned(),
            )
        } else {
            block
        };
        frame.render_widget(block, area);

        // Only the visible lines are cloned, however long the output gets.
        let lines: Vec<Line> = self
            .rendered()
            .iter()
            .cloned()
            .chain(partial)
            .chain(trailer)
            .skip(offset)
            .take(viewport)
            .collect();
        let mut paragraph = Paragraph::new(lines);

        // Errors are only a few lines, so wrapping them can't throw off the scroll math.
        if let Run::Failed { .. } = self.run {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        frame.render_widget(paragraph, content);

        if let Some(row) = input_row {
            self.draw_input(frame, row);
        }

        if total > viewport {
            let mut state = ScrollbarState::new(total - viewport)
                .position(offset)
                .viewport_content_length(viewport);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .symbols(scrollbar::Set {
                    track: "│",
                    thumb: "┃",
                    begin: "",
                    end: "",
                })
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::new().fg(MUTED))
                .thumb_style(Style::new().fg(SUBTLE));

            frame.render_stateful_widget(scrollbar, area.inner(Margin::new(0, 1)), &mut state);
        }
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let (prompt_style, placeholder) = if self.focused {
            (
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                "type input, enter to send",
            )
        } else {
            (Style::new().fg(MUTED), "ctrl+o to type input")
        };

        let prompt = Span::styled("› ", prompt_style);
        let text = if self.input.is_empty() {
            Span::styled(
                placeholder,
                Style::new().fg(MUTED).add_modifier(Modifier::ITALIC),
            )
        } else {
            Span::styled(self.input.value(), Style::new().fg(TEXT))
        };

        let prompt_width = prompt.width() as u16;
        frame.render_widget(
            Paragraph::new(Line::from(vec![prompt, text])).style(Style::new().bg(SURFACE)),
            area,
        );

        if self.focused {
            let x = area.x + prompt_width + self.input.cursor_width();
            frame.set_cursor_position(Position::new(x.min(area.right()), area.y));
        }
    }
}

enum Exit {
    Code(i32),
    Interrupted,
    Killed,
}

impl Exit {
    fn of(status: &ExitStatus) -> Self {
        if let Some(code) = status.code() {
            return Exit::Code(code);
        }

        #[cfg(unix)]
        if std::os::unix::process::ExitStatusExt::signal(status) == Some(libc::SIGINT) {
            return Exit::Interrupted;
        }

        Exit::Killed
    }
}

/// Parses one line of program output, keeping the program's own ANSI colors. Plain
/// compiler-style `error`/`warning` lines are emphasized.
fn render_line(raw: &str, workdir: &str) -> Line<'static> {
    let raw: Cow<str> = if raw.contains(workdir) {
        Cow::Owned(raw.replace(workdir, ""))
    } else {
        Cow::Borrowed(raw)
    };

    if raw.contains('\x1b')
        && let Ok(text) = raw.as_bytes().into_text()
    {
        return text.lines.into_iter().next().unwrap_or_default();
    }

    let trimmed = raw.trim_start();
    let style = if trimmed.starts_with("error") || trimmed.contains("Error:") {
        Style::new().fg(ERROR).add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("warning") {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT)
    };

    Line::styled(raw.into_owned(), style)
}
