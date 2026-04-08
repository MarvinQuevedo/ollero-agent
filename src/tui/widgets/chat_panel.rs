//! Scrollable chat panel displaying the conversation history.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use super::markdown;

// ── Chat message types ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ChatMessage {
    User(String),
    Assistant(String),
    System(String),
    Error(String),
    ToolHeader(String),
    ToolResult(String, String),
}

// ── Constants ───────────────────────────────────────────────────────────────

const SPINNER_FRAMES: &[&str] = &[
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}",
    "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280F}",
];

// ── Chat panel widget ───────────────────────────────────────────────────────

pub struct ChatPanel<'a> {
    pub messages: &'a [ChatMessage],
    pub streaming_text: &'a str,
    pub is_streaming: bool,
    pub spinner_frame: usize,
    pub scroll_offset: usize,
    /// Selection range: (start_line, start_col, end_line, end_col) in visible coordinates.
    pub selection: Option<(usize, u16, usize, u16)>,
}

impl<'a> ChatPanel<'a> {
    /// Convert all messages + streaming text into ratatui Lines.
    fn build_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let _inner_width = width.saturating_sub(2) as usize; // account for borders

        for msg in self.messages {
            match msg {
                ChatMessage::User(text) => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled(
                            " \u{276F} ",
                            Style::default()
                                .fg(Color::Rgb(100, 149, 237))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            text.clone(),
                            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
                ChatMessage::Assistant(text) => {
                    lines.push(Line::from(""));
                    let md_lines = markdown::to_ratatui_lines(text);
                    lines.extend(md_lines);
                }
                ChatMessage::System(text) => {
                    lines.push(Line::from(""));
                    for l in text.lines() {
                        lines.push(Line::from(Span::styled(
                            format!("  {l}"),
                            Style::default().fg(Color::Rgb(140, 140, 160)),
                        )));
                    }
                }
                ChatMessage::Error(text) => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  {text}"),
                        Style::default().fg(Color::Rgb(220, 70, 70)),
                    )));
                }
                ChatMessage::ToolHeader(text) => {
                    for l in text.lines() {
                        lines.push(Line::from(Span::styled(
                            l.to_string(),
                            Style::default().fg(Color::Rgb(100, 149, 237)),
                        )));
                    }
                }
                ChatMessage::ToolResult(name, preview) => {
                    let short = if preview.len() > 120 {
                        format!("{}...", &preview[..120])
                    } else {
                        preview.clone()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            "    \u{2713} ",
                            Style::default().fg(Color::Rgb(100, 200, 100)),
                        ),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(Color::Rgb(140, 140, 160)),
                        ),
                        Span::styled(
                            format!(" {short}"),
                            Style::default().fg(Color::Rgb(80, 80, 100)),
                        ),
                    ]));
                }
            }
        }

        // Streaming text (in-progress response)
        if self.is_streaming && !self.streaming_text.is_empty() {
            lines.push(Line::from(""));
            let md_lines = markdown::to_ratatui_lines(self.streaming_text);
            lines.extend(md_lines);
        }

        // Spinner when waiting
        if self.is_streaming && self.streaming_text.is_empty() {
            lines.push(Line::from(""));
            let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {frame} "),
                    Style::default().fg(Color::Rgb(100, 149, 237)),
                ),
                Span::styled(
                    "Thinking\u{2026}",
                    Style::default().fg(Color::Rgb(180, 160, 100)),
                ),
            ]));
        }

        lines
    }

    /// Calculate scroll info: (view_start, total_lines) for the given area height.
    pub fn calc_view(&self, width: u16, visible_height: usize) -> (usize, usize) {
        let total = self.build_lines(width).len();
        let start = if total <= visible_height {
            0
        } else {
            let max_scroll = total - visible_height;
            let actual_offset = self.scroll_offset.min(max_scroll);
            total - visible_height - actual_offset
        };
        (start, total)
    }

    /// Build plain text lines (for clipboard copy).
    pub fn build_plain_lines(&self, width: u16) -> Vec<String> {
        self.build_lines(width)
            .iter()
            .map(|line| {
                line.spans.iter().map(|s| s.content.as_ref()).collect::<String>()
            })
            .collect()
    }
}

impl<'a> Widget for ChatPanel<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT)
            .border_style(Style::default().fg(Color::Rgb(40, 42, 54)));

        let inner = block.inner(area);
        block.render(area, buf);

        let all_lines = self.build_lines(area.width);
        let visible_height = inner.height as usize;
        let total_lines = all_lines.len();

        // Calculate scroll: offset 0 = bottom, offset N = N lines up from bottom
        let start = if total_lines <= visible_height {
            0
        } else {
            let max_scroll = total_lines - visible_height;
            let actual_offset = self.scroll_offset.min(max_scroll);
            total_lines - visible_height - actual_offset
        };

        let end = (start + visible_height).min(total_lines);
        let visible_lines: Vec<Line> = all_lines[start..end].to_vec();

        let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);

        // Highlight selected text
        if let Some((sel_start_line, sel_start_col, sel_end_line, sel_end_col)) = self.selection {
            let highlight = Style::default()
                .bg(Color::Rgb(60, 80, 120))
                .fg(Color::White);

            for vis_row in 0..visible_height {
                let abs_line = start + vis_row;
                if abs_line < sel_start_line || abs_line > sel_end_line {
                    continue;
                }

                let screen_y = inner.y + vis_row as u16;
                let line_width = inner.width;

                // Columns are already in text-relative coordinates
                let col_start = if abs_line == sel_start_line {
                    sel_start_col
                } else {
                    0
                };
                let col_end = if abs_line == sel_end_line {
                    sel_end_col
                } else {
                    line_width
                };

                for x in col_start..col_end.min(line_width) {
                    let screen_x = inner.x + x;
                    if screen_x < buf.area.width && screen_y < buf.area.height {
                        buf[(screen_x, screen_y)].set_style(highlight);
                    }
                }
            }
        }
    }
}
