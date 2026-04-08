//! Top status bar: model name, context usage bar, CPU/RAM metrics.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::tui::app::App;

pub struct StatusBar<'a> {
    pub app: &'a App,
}

impl<'a> Widget for StatusBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let model = &self.app.client.model;
        let model_short = if model.len() > 18 {
            &model[..18]
        } else {
            model
        };

        // Context usage
        let pct = self.app.context_pct();
        let bar_width = 16usize;
        let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);

        let bar_color = if pct < 50.0 {
            Color::Rgb(80, 160, 200)
        } else if pct < 75.0 {
            Color::Rgb(220, 180, 60)
        } else if pct < 90.0 {
            Color::Rgb(220, 130, 50)
        } else {
            Color::Rgb(220, 70, 70)
        };

        let used_tokens = (self.app.history_char_count() + 3) / 4;
        let total_tokens = self.app.config.context_size as usize;
        let used_display = fmt_k(used_tokens);
        let total_display = fmt_k(total_tokens);

        // System metrics
        let (cpu_str, cpu_color, ram_str) = if let Ok(m) = self.app.metrics.try_read() {
            let cpu = m.cpu_usage;
            let cc = if cpu < 60.0 {
                Color::Rgb(100, 200, 100)
            } else if cpu < 85.0 {
                Color::Rgb(220, 180, 60)
            } else {
                Color::Rgb(220, 70, 70)
            };
            (
                format!("CPU:{cpu:.0}%"),
                cc,
                format!("RAM:{}", m.ram_display()),
            )
        } else {
            ("CPU:--".into(), Color::DarkGray, "RAM:--".into())
        };

        // Mode
        let mode_label = self.app.mode.label();

        // Phase indicator
        let phase_str = match &self.app.phase {
            crate::tui::app::AgentPhase::Idle => "",
            crate::tui::app::AgentPhase::WaitingForLlm => " [thinking...]",
            crate::tui::app::AgentPhase::ExecutingTools => " [tools...]",
            crate::tui::app::AgentPhase::WaitingForPermission { .. } => " [permission?]",
        };

        let dim = Style::default().fg(Color::Rgb(100, 100, 120));
        let accent = Style::default().fg(Color::Rgb(100, 149, 237));
        let sep = Style::default().fg(Color::Rgb(50, 60, 80));

        let spans = vec![
            Span::styled(" ALLUX ", Style::default().fg(Color::Rgb(80, 140, 240)).add_modifier(Modifier::BOLD)),
            Span::styled(" \u{2502} ", sep),
            Span::styled(model_short, Style::default().fg(Color::Rgb(100, 180, 255))),
            Span::styled(" ", dim),
            Span::styled("\u{2588}".repeat(filled), Style::default().fg(bar_color)),
            Span::styled("\u{2591}".repeat(empty), Style::default().fg(Color::Rgb(40, 40, 50))),
            Span::styled(format!(" {used_display}/{total_display}"), dim),
            Span::styled(format!(" {pct:.0}%"), Style::default().fg(bar_color)),
            Span::styled(" \u{2502} ", sep),
            Span::styled(&cpu_str, Style::default().fg(cpu_color)),
            Span::styled(" \u{00B7} ", sep),
            Span::styled(&ram_str, Style::default().fg(Color::Rgb(140, 180, 200))),
            Span::styled(" \u{2502} ", sep),
            Span::styled(mode_label, accent),
            Span::styled(phase_str, Style::default().fg(Color::Rgb(220, 180, 60))),
        ];

        // Append status message or scroll hints on the right
        let mut right_spans: Vec<Span> = Vec::new();
        if let Some(ref msg) = self.app.status_message {
            right_spans.push(Span::styled(" \u{2502} ", sep));
            right_spans.push(Span::styled(
                msg.clone(),
                Style::default().fg(Color::Rgb(220, 180, 60)),
            ));
        } else if !self.app.auto_scroll && self.app.scroll_offset > 0 {
            right_spans.push(Span::styled(" \u{2502} ", sep));
            right_spans.push(Span::styled(
                format!("\u{2191}{}lines", self.app.scroll_offset),
                Style::default().fg(Color::Rgb(180, 140, 60)),
            ));
        } else {
            right_spans.push(Span::styled(" \u{2502} ", sep));
            right_spans.push(Span::styled(
                "drag=copy Esc=cancel",
                Style::default().fg(Color::Rgb(70, 70, 90)),
            ));
        }

        let mut all_spans = spans;
        all_spans.extend(right_spans);

        let line = Line::from(all_spans);
        let bg_style = Style::default().bg(Color::Rgb(20, 22, 30));

        // Fill background
        for x in area.left()..area.right() {
            buf[(x, area.y)].set_style(bg_style);
        }

        buf.set_line(area.x, area.y, &line, area.width);
        // Apply background to all cells in the line
        for x in area.left()..area.right() {
            buf[(x, area.y)].set_bg(Color::Rgb(20, 22, 30));
        }
    }
}

fn fmt_k(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{n}")
    }
}
