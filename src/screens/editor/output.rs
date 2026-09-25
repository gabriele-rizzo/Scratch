use std::{borrow::Cow, collections::VecDeque, path::Path, process::ExitStatus};

use ansi_to_tui::IntoText;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Padding, Paragraph},
};

use super::search::{self, Search};
use crate::{
    runners::{Process, RunStatus, SCROLLBACK, TerminalSize},
    ui::{self, ACCENT, ERROR, Input, InputView, MUTED, SUBTLE, SUCCESS, SURFACE, Scroll, TEXT},
};

enum Run {
    /// `workdir` is hidden from output so paths read as `main.<ext>`.
    Started { process: Process, workdir: String },
    /// The program couldn't be started at all.
    Failed { title: String, detail: String },
}

/// How many rows each output line wraps to at `width`, kept up to date as lines
/// arrive so long output isn't rewrapped every frame.
#[derive(Default)]
struct RowCache {
    width: usize,
    counts: VecDeque<usize>,
    total: usize,
}

/// The output panel for the latest run.
pub struct Output {
    run: Run,
    scroll: Scroll,
    /// Text being typed for the program's stdin.
    input: Input,
    /// Whether keys go to the program instead of the editor.
    focused: bool,
    /// Whether the top border is being dragged to resize the panel.
    resizing: bool,
    /// The program's arguments, shown in the title.
    args: String,
    rows: RowCache,
    /// Output lines, parsed once as they arrive; the last `SCROLLBACK` of them.
    rendered: VecDeque<Line<'static>>,
    /// How many of the program's lines have been taken into `rendered`.
    seen: usize,
    /// How many lines were dropped (or skipped) to stay within `SCROLLBACK`.
    discarded: usize,
    /// Finding text in the output, while the Find bar is open.
    search: Option<Search>,
    /// Where the panel was last drawn, for mouse hit-testing.
    pub area: Rect,
}

impl Output {
    pub fn started(process: Process, workdir: &Path, args: &[String]) -> Self {
        let mut output = Self::new(Run::Started {
            process,
            workdir: format!("{}/", workdir.display()),
        });
        output.args = crate::utils::join_args(args);
        output
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
            resizing: false,
            args: String::new(),
            rows: RowCache::default(),
            rendered: VecDeque::new(),
            seen: 0,
            discarded: 0,
            search: None,
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

    pub fn set_resizing(&mut self, resizing: bool) {
        self.resizing = resizing;
    }

    /// Opens the Find bar, searching for `query`.
    pub fn start_search(&mut self, query: &str) {
        let mut search = Search::default();
        search.input.set(query);
        self.search = Some(search);
        self.research();
    }

    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// Keys for the Find bar: typing searches, ↑↓ (or Enter and Shift+Enter)
    /// move between matches, Esc closes it.
    pub fn search_key(&mut self, key: KeyEvent) {
        let Some(search) = &mut self.search else {
            return;
        };
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match key.code {
            KeyCode::Esc => self.search = None,
            KeyCode::Enter if shift => search.step(false),
            KeyCode::Enter | KeyCode::Down => search.step(true),
            KeyCode::Up => search.step(false),
            _ => {
                if search.input.handle(key) {
                    self.research();
                }
            }
        }
    }

    pub fn search_paste(&mut self, text: &str) {
        if let Some(search) = &mut self.search {
            search.input.insert(crate::utils::first_line(text));
            self.research();
        }
    }

    pub fn close_search(&mut self) {
        self.search = None;
    }

    /// Searches every kept line again, starting from the newest match, which is
    /// usually nearest what's in view.
    fn research(&mut self) {
        let Some(search) = &mut self.search else {
            return;
        };

        search.matches = self
            .rendered
            .iter()
            .enumerate()
            .flat_map(|(index, line)| search.find(self.discarded + index, &line_text(line)))
            .collect();
        search.current = search.matches.len().saturating_sub(1);
        search.jump = !search.matches.is_empty();
    }

    /// Draws the Find bar in `area` (3 rows).
    pub fn draw_search(&self, frame: &mut Frame, area: Rect) {
        let Some(search) = &self.search else { return };

        let mut title = vec![Span::styled(
            " Find ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )];
        let counter = search.counter();
        if !counter.is_empty() {
            title.push(Span::styled(
                format!("· {counter} "),
                Style::new().fg(MUTED),
            ));
        }

        search.input.render(
            frame,
            area,
            InputView {
                title: Line::from(title),
                prompt: "/",
                placeholder: "search the output",
                enabled: true,
            },
        );
    }

    /// Whether the panel changes on its own and needs regular updates.
    pub fn is_live(&self) -> bool {
        self.process().is_some_and(|process| !process.is_settled())
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

    /// Pastes into the input; every complete line is sent, the rest stays to edit.
    pub fn paste(&mut self, text: &str) {
        let text = crate::utils::normalize_newlines(text);
        let mut lines = text.split('\n').peekable();

        while let Some(line) = lines.next() {
            self.input.insert(line);

            if lines.peek().is_some() {
                self.submit();
            }
        }
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
        if let Run::Started { process, workdir } = &mut self.run {
            process.poll();

            // Render only the new lines that will still be kept.
            let total = process.dropped + process.lines.len();
            let first = self
                .seen
                .max(process.dropped)
                .max(total.saturating_sub(SCROLLBACK));
            self.discarded += first - self.seen;
            for index in first..total {
                let raw = &process.lines[index - process.dropped];
                let line = render_line(raw, workdir);

                // New lines are searched as they arrive.
                if let Some(search) = &mut self.search {
                    let number = self.discarded + self.rendered.len();
                    search
                        .matches
                        .extend(search.find(number, &line_text(&line)));
                }
                self.rendered.push_back(line);
            }
            self.seen = total;

            let mut removed_rows = 0;
            while self.rendered.len() > SCROLLBACK {
                self.rendered.pop_front();
                self.discarded += 1;

                // `counts` covers a prefix of `rendered`, so its front is this line.
                if let Some(count) = self.rows.counts.pop_front() {
                    self.rows.total -= count;
                    removed_rows += count;
                }
            }

            // Keep the same lines in view when scrolled back.
            self.scroll.shift(removed_rows);

            if let Some(search) = &mut self.search {
                search.forget_before(self.discarded);
            }
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

    fn title(&self) -> Line<'static> {
        let mut title = vec![Span::styled(
            " Output ",
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )];

        if !self.args.is_empty() {
            title.push(Span::styled("· ", Style::new().fg(MUTED)));
            title.push(Span::styled(self.args.clone(), Style::new().fg(SUBTLE)));
            title.push(Span::raw(" "));
        }

        Line::from(title)
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

    /// The output as plain text (colors removed), or just its last `lines` lines.
    /// Returns the text and how many lines it has.
    pub fn plain_text(&self, lines: Option<usize>) -> (String, usize) {
        let mut all: Vec<String> = self
            .rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        if let Some(partial) = self.partial() {
            all.push(
                partial
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect(),
            );
        }

        let skip = lines.map_or(0, |lines| all.len().saturating_sub(lines));
        let kept = &all[skip..];
        (kept.join("\n"), kept.len())
    }

    /// Notes lines dropped to stay within the scrollback.
    fn discarded_note(&self) -> Option<Line<'static>> {
        (self.discarded > 0).then(|| {
            Line::styled(
                format!("… {} earlier lines not kept", thousands(self.discarded)),
                Style::new().fg(MUTED).add_modifier(Modifier::ITALIC),
            )
        })
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

    /// Counts rows for lines that arrived since the last frame, or for all of them
    /// when the width changed.
    fn update_row_counts(&mut self, width: usize) {
        let rows = &mut self.rows;
        if rows.width != width {
            *rows = RowCache {
                width,
                ..RowCache::default()
            };
        }

        for line in self.rendered.iter().skip(rows.counts.len()) {
            let count = ui::row_count(line, width);
            rows.counts.push_back(count);
            rows.total += count;
        }
    }

    /// Scrolls so the current match sits mid-panel, once after it changes.
    fn jump_to_match(&mut self, note_count: usize, width: usize, viewport: usize) {
        let Some(search) = self.search.as_mut().filter(|search| search.jump) else {
            return;
        };
        search.jump = false;
        let Some(found) = search.current() else {
            return;
        };
        let Some(position) = found.line.checked_sub(self.discarded) else {
            return;
        };

        // Rows above the match's line, then its row within the line: wrapping the
        // text up to and including its first character tells which row that is.
        let above: usize = note_count + self.rows.counts.iter().take(position).sum::<usize>();
        let text = self
            .rendered
            .get(position)
            .map(line_text)
            .unwrap_or_default();
        let prefix: String = text.chars().take(found.start + 1).collect();
        let within = ui::row_count(&Line::raw(prefix), width).saturating_sub(1);

        self.scroll
            .jump_to((above + within).saturating_sub(viewport / 2));
    }

    /// The terminal size a running program gets in a panel drawn in `area`: inside
    /// the borders and padding, above the input row. Matches `draw`.
    pub fn terminal_size(area: Rect) -> TerminalSize {
        TerminalSize {
            columns: area.width.saturating_sub(4),
            rows: area.height.saturating_sub(3),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.area = area;

        let border = if self.focused || self.resizing {
            ACCENT
        } else {
            MUTED
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(border))
            .padding(Padding::horizontal(1))
            .title(self.title())
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

        // The program's terminal matches the space its output gets.
        if let Some(process) = self.process_mut() {
            process.resize(TerminalSize {
                columns: content.width,
                rows: content.height,
            });
        }

        let width = content.width as usize;
        self.update_row_counts(width);
        let viewport = content.height as usize;

        let note = self.discarded_note();
        let note_count = note.as_ref().map_or(0, |line| ui::row_count(line, width));

        let partial = self.partial();
        let extra: Vec<Line<'static>> = partial
            .iter()
            .cloned()
            .chain(self.trailer(partial.is_some()))
            .collect();
        let extra_counts: Vec<usize> = extra
            .iter()
            .map(|line| ui::row_count(line, width))
            .collect();

        // Everything below counts wrapped rows, not lines.
        let total = note_count + self.rows.total + extra_counts.iter().sum::<usize>();
        self.jump_to_match(note_count, width, viewport);
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

        // Only the visible lines are wrapped and cloned, however long the output gets.
        // Lines with matches are highlighted; the rest are shown as they are.
        let search = self.search.as_ref();
        let current = search.and_then(Search::current);
        let rendered = self.rendered.iter().enumerate().map(|(index, line)| {
            let matches = search.map_or(&[][..], |search| search.in_line(self.discarded + index));
            if matches.is_empty() {
                Cow::Borrowed(line)
            } else {
                Cow::Owned(search::highlight(line, matches, current))
            }
        });

        let lines = note
            .iter()
            .map(Cow::Borrowed)
            .zip([note_count])
            .chain(rendered.zip(self.rows.counts.iter().copied()))
            .chain(extra.iter().map(Cow::Borrowed).zip(extra_counts));

        let mut skip = offset;
        let mut visible: Vec<Line> = Vec::with_capacity(viewport);

        for (line, count) in lines {
            if visible.len() == viewport {
                break;
            }
            if skip >= count {
                skip -= count;
                continue;
            }

            let remaining = viewport - visible.len();
            visible.extend(
                ui::wrap(&line, width)
                    .into_iter()
                    .skip(skip)
                    .take(remaining),
            );
            skip = 0;
        }

        frame.render_widget(Paragraph::new(visible), content);

        if let Some(row) = input_row {
            self.draw_input(frame, row);
        }

        ui::scrollbar(
            frame,
            area.inner(Margin::new(0, 1)),
            total,
            viewport,
            offset,
            SUBTLE,
        );
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
    // Only detected on Unix, where programs die by signal.
    #[cfg_attr(not(unix), allow(dead_code))]
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

/// A line's text without its styling.
fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// Formats a count with thousands separators, e.g. `12,345`.
fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::new();

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }

    formatted
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

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    const WORKDIR: &str = "/tmp/scratch-1-2/";

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn formats_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn hides_the_temporary_directory() {
        let line = render_line("  File \"/tmp/scratch-1-2/main.py\", line 1", WORKDIR);
        assert_eq!(text(&line), "  File \"main.py\", line 1");
    }

    #[test]
    fn keeps_the_programs_colors() {
        let line = render_line("\x1b[31mred\x1b[0m plain", WORKDIR);
        assert_eq!(text(&line), "red plain");
        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn emphasizes_plain_errors_and_warnings() {
        let error = render_line("error[E0308]: mismatched types", WORKDIR);
        let exception = render_line("ValueError: bad value", WORKDIR);
        let warning = render_line("warning: unused variable", WORKDIR);
        let plain = render_line("hello", WORKDIR);

        assert_eq!(error.style.fg, Some(ERROR));
        assert_eq!(exception.style.fg, Some(ERROR));
        assert_eq!(warning.style.fg, Some(ACCENT));
        assert_eq!(plain.style.fg, Some(TEXT));
    }

    #[cfg(unix)]
    mod draw {
        use std::{
            process::Command,
            thread,
            time::{Duration, Instant},
        };

        use ratatui::{Terminal, backend::TestBackend};

        use super::*;

        const AREA: Rect = Rect::new(0, 0, 40, 12);

        fn run(script: &str) -> Output {
            let mut command = Command::new("sh");
            command.arg("-c").arg(script);
            let process = Process::spawn(command, Output::terminal_size(AREA)).unwrap();
            Output::started(process, Path::new("/nonexistent"), &[])
        }

        fn draw(output: &mut Output) -> Vec<String> {
            let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
            terminal.draw(|frame| output.draw(frame, AREA)).unwrap();

            let buffer = terminal.backend().buffer();
            (0..AREA.height)
                .map(|y| {
                    (0..AREA.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect()
        }

        fn wait_for(output: &mut Output, done: impl Fn(&Output) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done(output) {
                assert!(Instant::now() < deadline, "timed out");
                thread::sleep(Duration::from_millis(10));
                output.poll();
            }
        }

        #[test]
        fn wraps_long_lines_inside_the_panel() {
            let mut output = run(&format!("echo {}", "a".repeat(80)));
            wait_for(&mut output, |output| {
                !output.is_running() && !output.rendered.is_empty()
            });

            let screen = draw(&mut output);
            // 36 columns inside the borders and padding: 80 chars wrap to 3 rows.
            assert_eq!(screen[1].trim_matches(['│', ' ']), "a".repeat(36));
            assert_eq!(screen[2].trim_matches(['│', ' ']), "a".repeat(36));
            assert_eq!(screen[3].trim_matches(['│', ' ']), "a".repeat(8));
            assert_eq!(output.rows.total, 3);
        }

        #[test]
        fn notes_lines_dropped_from_scrollback() {
            let mut output = run(&format!("seq 1 {}", SCROLLBACK + 250));
            wait_for(&mut output, |output| {
                !output.is_running() && output.seen == SCROLLBACK + 250
            });

            assert_eq!(output.rendered.len(), SCROLLBACK);
            assert_eq!(output.discarded, 250);

            output.scroll.up(usize::MAX);
            let screen = draw(&mut output);
            assert!(
                screen[1].contains("… 250 earlier lines not kept"),
                "{screen:?}"
            );
            assert!(screen[2].contains("251"), "{screen:?}");
        }

        #[test]
        fn plain_text_drops_colors_and_can_keep_just_the_last_lines() {
            let mut output = run("printf '\\033[31mred\\033[0m\\nb\\nunfinished'");
            wait_for(&mut output, |output| !output.is_live());

            assert_eq!(
                output.plain_text(None),
                ("red\nb\nunfinished".to_string(), 3)
            );
            assert_eq!(output.plain_text(Some(2)), ("b\nunfinished".to_string(), 2));
            assert_eq!(output.plain_text(Some(99)).1, 3);
        }

        #[test]
        fn starting_size_matches_the_drawn_panel() {
            // If they differed, the first frame would resize and signal the program.
            let mut output =
                run("trap 'echo winch' WINCH; echo ready; while :; do sleep 0.05; done");
            wait_for(&mut output, |output| !output.rendered.is_empty());

            draw(&mut output);
            thread::sleep(Duration::from_millis(200));
            output.poll();

            let lines: Vec<String> = output.rendered.iter().map(text).collect();
            assert_eq!(lines, ["ready"]);
        }
    }
}
