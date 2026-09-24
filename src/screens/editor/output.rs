use std::{borrow::Cow, path::Path};

use ansi_to_tui::IntoText;
use ratatui::{
    Frame,
    layout::{Margin, Rect},
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
    ui::{self, ACCENT, ERROR, MUTED, SUBTLE, SUCCESS, Scroll, TEXT},
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
            area: Rect::default(),
        }
    }

    pub fn scroll(&mut self) -> &mut Scroll {
        &mut self.scroll
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
                let (icon, color, label) = match status.code() {
                    Some(0) => ("✓", SUCCESS, "exited 0".to_string()),
                    Some(code) => ("✗", ERROR, format!("exited {code}")),
                    None => ("✗", ERROR, "killed".to_string()),
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
    fn trailer(&self) -> Vec<Line<'static>> {
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
        let has_output = !process.lines.is_empty();

        match &process.status {
            RunStatus::Running if has_output => vec![],
            RunStatus::Running => vec![Line::styled("waiting for output…", italic.fg(MUTED))],
            RunStatus::Exited(status, _) => {
                let summary = match status.code() {
                    Some(0) if has_output => Line::styled("Finished", italic.fg(MUTED)),
                    Some(0) => Line::styled("Finished with no output", italic.fg(MUTED)),
                    Some(code) => {
                        Line::styled(format!("Process exited with code {code}"), italic.fg(ERROR))
                    }
                    None => Line::styled("Process was killed", italic.fg(ERROR)),
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

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(MUTED))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " Output ",
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ))
            .title_top(self.status().right_aligned());

        let inner = block.inner(area);
        let trailer = self.trailer();
        let total = self.rendered().len() + trailer.len();
        let viewport = inner.height as usize;
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

        // Only the visible lines are cloned, however long the output gets.
        let lines: Vec<Line> = self
            .rendered()
            .iter()
            .cloned()
            .chain(trailer)
            .skip(offset)
            .take(viewport)
            .collect();
        let mut paragraph = Paragraph::new(lines).block(block);

        // Errors are only a few lines, so wrapping them can't throw off the scroll math.
        if let Run::Failed { .. } = self.run {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        frame.render_widget(paragraph, area);

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
