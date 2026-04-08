//! Input area widget using tui-textarea, with slash-command autocomplete.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};
use tui_textarea::TextArea;

/// Slash commands with descriptions for autocomplete.
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show available commands"),
    ("/quit", "Exit Allux"),
    ("/clear", "Clear conversation history"),
    ("/history", "Show conversation history"),
    ("/context", "Show workspace context snapshot"),
    ("/context refresh", "Rescan workspace files"),
    ("/model", "Show current model"),
    ("/model list", "List available Ollama models"),
    ("/mode", "Show current session mode"),
    ("/mode chat", "Switch to chat-only mode"),
    ("/mode agent", "Switch to agent mode (tools)"),
    ("/mode plan", "Switch to plan-then-execute mode"),
    ("/save", "Save current session"),
    ("/sessions", "List saved sessions"),
    ("/resume", "Resume a saved session"),
    ("/verbose", "Toggle verbose tool output"),
    ("/read", "Read a file into context"),
    ("/glob", "Find files by pattern"),
    ("/tree", "Show directory tree"),
    ("/compress", "Show compression stats"),
    ("/compress now", "Compress history now"),
    ("/compress ai", "AI-powered history summary"),
    ("/compress always", "Set compression to always"),
    ("/compress auto", "Set compression to auto"),
    ("/compress manual", "Set compression to manual"),
    ("/unload", "Unload model from VRAM/RAM"),
];

/// Max autocomplete items to show.
const MAX_MENU: usize = 8;

/// Create a new TextArea with Allux styling.
pub fn new_textarea<'a>() -> TextArea<'a> {
    let mut ta = TextArea::default();
    ta.set_cursor_line_style(Style::default());
    ta.set_cursor_style(
        Style::default()
            .fg(Color::Rgb(100, 149, 237))
            .add_modifier(Modifier::REVERSED),
    );
    ta.set_block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(60, 70, 90)))
            .title(Span::styled(
                " \u{276F} Input (Enter to send, Ctrl+D exit) ",
                Style::default()
                    .fg(Color::Rgb(100, 149, 237))
                    .add_modifier(Modifier::BOLD),
            )),
    );
    ta.set_placeholder_text("Type a message or /help...");
    ta.set_placeholder_style(Style::default().fg(Color::Rgb(80, 80, 100)));
    ta
}

/// Get the current text from the textarea.
pub fn current_text(textarea: &TextArea) -> String {
    textarea.lines().join("\n")
}

/// Get matching slash commands for the current input.
/// Returns (visible_completions, total_matching_count).
pub fn get_completions(input: &str) -> (Vec<(&'static str, &'static str)>, usize) {
    if !input.starts_with('/') || input.is_empty() {
        return (vec![], 0);
    }
    let all: Vec<_> = SLASH_COMMANDS
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(input) && *cmd != input)
        .copied()
        .collect();
    let total = all.len();
    let visible: Vec<_> = all.into_iter().take(MAX_MENU).collect();
    (visible, total)
}

/// Get the ghost text (best matching completion) for inline display.
pub fn ghost_for(input: &str) -> Option<&'static str> {
    if !input.starts_with('/') || input.is_empty() {
        return None;
    }
    SLASH_COMMANDS
        .iter()
        .find(|(cmd, _)| cmd.starts_with(input) && cmd.len() > input.len())
        .map(|(cmd, _)| *cmd)
}

/// Render the autocomplete popup above the input area.
pub struct AutocompletePopup<'a> {
    pub completions: &'a [(&'static str, &'static str)],
    pub total_count: usize,
}

impl<'a> Widget for AutocompletePopup<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.completions.is_empty() || area.height == 0 {
            return;
        }

        let hidden = self.total_count.saturating_sub(self.completions.len());
        let has_more_line = hidden > 0;
        let rows = self.completions.len() as u16 + if has_more_line { 1 } else { 0 };
        let popup_height = rows;
        let popup_width = 55u16.min(area.width);

        // Position above the input area
        let y = area.y.saturating_sub(popup_height);
        let popup_area = Rect::new(area.x + 2, y, popup_width, popup_height);

        // Clear background
        Clear.render(popup_area, buf);

        for (i, (cmd, desc)) in self.completions.iter().enumerate() {
            let row = popup_area.y + i as u16;
            if row >= buf.area.height {
                break;
            }
            let cmd_span = Span::styled(
                format!(" {cmd:<22}"),
                Style::default().fg(Color::Rgb(100, 200, 255)),
            );
            let desc_span = Span::styled(
                *desc,
                Style::default().fg(Color::Rgb(100, 100, 120)),
            );
            let line = Line::from(vec![cmd_span, desc_span]);
            buf.set_line(popup_area.x, row, &line, popup_width);

            // Background
            for x in popup_area.x..popup_area.x + popup_width {
                if x < buf.area.width && row < buf.area.height {
                    buf[(x, row)].set_bg(Color::Rgb(30, 32, 40));
                }
            }
        }

        // "+N more" line
        if has_more_line {
            let row = popup_area.y + self.completions.len() as u16;
            if row < buf.area.height {
                let more_line = Line::from(Span::styled(
                    format!(" ... +{hidden} more"),
                    Style::default().fg(Color::Rgb(80, 80, 100)),
                ));
                buf.set_line(popup_area.x, row, &more_line, popup_width);
                for x in popup_area.x..popup_area.x + popup_width {
                    if x < buf.area.width {
                        buf[(x, row)].set_bg(Color::Rgb(30, 32, 40));
                    }
                }
            }
        }
    }
}

/// Possible input actions from key events.
pub enum InputAction {
    /// User submitted text.
    Submit(String),
    /// User wants to quit (Ctrl+D on empty).
    Quit,
    /// Key was consumed by the textarea (no action needed).
    Consumed,
}

/// Process a key event for the input textarea.
pub fn handle_key(textarea: &mut TextArea, key: KeyEvent) -> InputAction {
    match (key.code, key.modifiers) {
        // Enter: submit the text
        (KeyCode::Enter, KeyModifiers::NONE) => {
            let text: String = textarea.lines().join("\n").trim().to_string();
            textarea.select_all();
            textarea.cut();
            if text.is_empty() {
                InputAction::Consumed
            } else {
                InputAction::Submit(text)
            }
        }
        // Tab: accept ghost completion
        (KeyCode::Tab, _) => {
            let text = textarea.lines().join("");
            if let Some(completed) = ghost_for(&text) {
                textarea.select_all();
                textarea.cut();
                textarea.insert_str(completed);
            }
            InputAction::Consumed
        }
        // Right arrow at end: accept ghost
        (KeyCode::Right, KeyModifiers::NONE) => {
            let text = textarea.lines().join("");
            let (row, col) = textarea.cursor();
            let at_end = row == textarea.lines().len() - 1
                && col == textarea.lines().last().map(|l| l.len()).unwrap_or(0);
            if at_end {
                if let Some(completed) = ghost_for(&text) {
                    textarea.select_all();
                    textarea.cut();
                    textarea.insert_str(completed);
                    return InputAction::Consumed;
                }
            }
            textarea.input(key);
            InputAction::Consumed
        }
        // Ctrl+D on empty: quit
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            let text: String = textarea.lines().join("");
            if text.is_empty() {
                InputAction::Quit
            } else {
                textarea.input(key);
                InputAction::Consumed
            }
        }
        // Ctrl+C: clear input
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            textarea.select_all();
            textarea.cut();
            InputAction::Consumed
        }
        // All other keys: let textarea handle it
        _ => {
            textarea.input(key);
            InputAction::Consumed
        }
    }
}
