use std::{borrow::Cow, path::Path};

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
    runners::{Process, RunStatus, Stream},
    ui::{self, ACCENT, ERROR, MUTED, SUBTLE, SUCCESS, Scroll, TEXT},
};

enum Run {
    /// `workdir` is hidden from output so paths read as `main.<ext>`.
    Started { process: Process, workdir: String },
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
        if let Run::Started { process, .. } = &mut self.run {
            process.poll();
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

    fn lines(run: &Run) -> Vec<Line<'_>> {
        let (process, workdir) = match run {
            Run::Started { process, workdir } => (process, workdir),
            Run::Failed { title, detail } => {
                return vec![
                    Line::from(vec![
                        Span::styled("▎ ", Style::new().fg(ERROR)),
                        Span::styled(
                            title.as_str(),
                            Style::new().fg(ERROR).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::default(),
                    Line::from(Span::styled(detail.as_str(), Style::new().fg(SUBTLE))),
                ];
            }
        };

        let mut lines: Vec<Line> = process
            .lines
            .iter()
            .map(|(stream, text)| line(*stream, text, workdir))
            .collect();

        match &process.status {
            RunStatus::Running if lines.is_empty() => lines.push(Line::from(Span::styled(
                "waiting for output…",
                Style::new().fg(MUTED).add_modifier(Modifier::ITALIC),
            ))),
            RunStatus::Running => {}
            RunStatus::Exited(status, _) => {
                if !lines.is_empty() {
                    lines.push(Line::default());
                }

                let summary = match status.code() {
                    Some(0) if process.lines.is_empty() => {
                        Span::styled("Finished with no output", Style::new().fg(MUTED))
                    }
                    Some(0) => Span::styled("Finished", Style::new().fg(MUTED)),
                    Some(code) => Span::styled(
                        format!("Process exited with code {code}"),
                        Style::new().fg(ERROR),
                    ),
                    None => Span::styled("Process was killed", Style::new().fg(ERROR)),
                };
                lines.push(Line::from(summary).style(Style::new().add_modifier(Modifier::ITALIC)));
            }
        }

        lines
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
        let lines = Self::lines(&self.run);
        let total = lines.len();
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

        let mut paragraph = Paragraph::new(lines)
            .block(block)
            .scroll((offset.min(u16::MAX as usize) as u16, 0));

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

/// Styles one line of program output. stderr gets a red gutter, and compiler-style
/// `error`/`warning` headings are emphasized.
fn line<'a>(stream: Stream, text: &'a str, workdir: &str) -> Line<'a> {
    let text: Cow<str> = if text.contains(workdir) {
        Cow::Owned(text.replace(workdir, ""))
    } else {
        Cow::Borrowed(text)
    };

    let (gutter, style) = match stream {
        Stream::Stdout => ("  ", Style::new().fg(TEXT)),
        Stream::Stderr => {
            let trimmed = text.trim_start();
            let style = if trimmed.starts_with("error") || trimmed.contains("Error:") {
                Style::new().fg(ERROR).add_modifier(Modifier::BOLD)
            } else if trimmed.starts_with("warning") {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(SUBTLE)
            };
            ("▎ ", style)
        }
    };

    Line::from(vec![
        Span::styled(gutter, Style::new().fg(ERROR)),
        Span::styled(text, style),
    ])
}
