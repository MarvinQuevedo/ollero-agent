use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    style::Print,
    terminal::{self, ClearType},
};

/// Slash command with description for the autocomplete menu.
struct SlashCmd {
    cmd: &'static str,
    desc: &'static str,
}

const SLASH_COMMANDS: &[SlashCmd] = &[
    SlashCmd { cmd: "/help",             desc: "Show available commands" },
    SlashCmd { cmd: "/quit",             desc: "Exit Allux" },
    SlashCmd { cmd: "/exit",             desc: "Exit Allux" },
    SlashCmd { cmd: "/q",                desc: "Exit Allux" },
    SlashCmd { cmd: "/clear",            desc: "Clear conversation history" },
    SlashCmd { cmd: "/history",          desc: "Show conversation history" },
    SlashCmd { cmd: "/context",          desc: "Show workspace context snapshot" },
    SlashCmd { cmd: "/context refresh",  desc: "Rescan workspace files" },
    SlashCmd { cmd: "/model",            desc: "Show current model" },
    SlashCmd { cmd: "/model list",       desc: "List available Ollama models" },
    SlashCmd { cmd: "/mode",             desc: "Show current session mode" },
    SlashCmd { cmd: "/mode chat",        desc: "Switch to chat-only mode" },
    SlashCmd { cmd: "/mode agent",       desc: "Switch to agent mode (tools)" },
    SlashCmd { cmd: "/mode plan",        desc: "Switch to plan-then-execute mode" },
    SlashCmd { cmd: "/save",             desc: "Save current session" },
    SlashCmd { cmd: "/sessions",         desc: "List saved sessions" },
    SlashCmd { cmd: "/resume",           desc: "Resume a saved session" },
    SlashCmd { cmd: "/verbose",          desc: "Toggle verbose tool output" },
    SlashCmd { cmd: "/read",             desc: "Read a file into context" },
    SlashCmd { cmd: "/glob",             desc: "Find files by pattern" },
    SlashCmd { cmd: "/tree",             desc: "Show directory tree" },
    SlashCmd { cmd: "/compress",         desc: "Show compression stats" },
    SlashCmd { cmd: "/compress now",     desc: "Compress history now" },
    SlashCmd { cmd: "/compress ai",      desc: "AI-powered history summary" },
    SlashCmd { cmd: "/compress always",  desc: "Set compression to always" },
    SlashCmd { cmd: "/compress auto",    desc: "Set compression to auto" },
    SlashCmd { cmd: "/compress manual",  desc: "Set compression to manual" },
];

/// Max menu items to show in the autocomplete dropdown.
const MAX_MENU_ITEMS: usize = 8;

pub struct InputReader {
    history: Vec<String>,
}

impl InputReader {
    pub fn new() -> Self {
        Self { history: Vec::new() }
    }

    /// Read a line in raw mode: history (↑/↓), cursor, tab completion for `/` commands,
    /// Ctrl+C clear, Ctrl+D on empty exits with `Ok(None)`.
    ///
    /// `footer_hint`: optional dimmed line printed **above** the prompt (shortcuts).
    /// It is *not* redrawn while typing — only the `prompt + input` line uses raw-mode redraw.
    /// (Putting the hint on the line below `>` breaks on Windows / VS Code when using absolute cursor moves.)
    pub fn read_line(
        &mut self,
        prompt: &str,
        prompt_visible_len: usize,
        footer_hint: Option<&str>,
    ) -> Result<Option<String>> {
        let mut stdout = io::stdout();

        if let Some(h) = footer_hint {
            println!("{}", h.dimmed());
        }
        print!("{} ", prompt);
        stdout.flush()?;

        terminal::enable_raw_mode()?;
        let result = self.inner_read(&mut stdout, prompt, prompt_visible_len);
        terminal::disable_raw_mode()?;

        println!();

        let line = result?;
        if let Some(ref s) = line {
            let s = s.trim();
            if !s.is_empty() && self.history.last().map(|l| l.as_str()) != Some(s) {
                self.history.push(s.to_string());
            }
        }
        Ok(line.map(|s| s.trim().to_string()))
    }

    fn inner_read(
        &mut self,
        stdout: &mut io::Stdout,
        prompt: &str,
        prompt_visible_len: usize,
    ) -> Result<Option<String>> {
        let mut buf: Vec<char> = Vec::new();
        let mut cur: usize = 0;
        let mut hist_idx: Option<usize> = None;
        let mut saved_buf: Vec<char> = Vec::new();
        // How many menu lines are currently printed below the input line.
        let mut menu_lines: usize = 0;

        loop {
            let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }

            match (code, modifiers) {
                (KeyCode::Enter, _) => {
                    // Clear the menu before returning.
                    clear_menu(stdout, menu_lines)?;
                    return Ok(Some(buf.iter().collect()));
                }

                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        clear_menu(stdout, menu_lines)?;
                        return Ok(None);
                    }
                }

                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    clear_menu(stdout, menu_lines)?;
                    menu_lines = 0;
                    buf.clear();
                    cur = 0;
                    hist_idx = None;
                    saved_buf.clear();
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur, None)?;
                }

                (KeyCode::Backspace, _) => {
                    if cur > 0 {
                        buf.remove(cur - 1);
                        cur -= 1;
                        menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                    }
                }

                (KeyCode::Delete, _) => {
                    if cur < buf.len() {
                        buf.remove(cur);
                        menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                    }
                }

                (KeyCode::Left, _) => {
                    if cur > 0 {
                        cur -= 1;
                        redraw(stdout, prompt, prompt_visible_len, &buf, cur, ghost_for(&buf).as_deref())?;
                    }
                }
                (KeyCode::Right, _) => {
                    // Right arrow at end of input: accept ghost completion
                    if cur == buf.len() {
                        if let Some(ghost) = ghost_for(&buf) {
                            buf = ghost.chars().collect();
                            cur = buf.len();
                            menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                            continue;
                        }
                    }
                    if cur < buf.len() {
                        cur += 1;
                        redraw(stdout, prompt, prompt_visible_len, &buf, cur, ghost_for(&buf).as_deref())?;
                    }
                }
                (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                    cur = 0;
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur, ghost_for(&buf).as_deref())?;
                }
                (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                    cur = buf.len();
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur, ghost_for(&buf).as_deref())?;
                }

                (KeyCode::Up, _) => {
                    if self.history.is_empty() {
                        continue;
                    }
                    let new_idx = match hist_idx {
                        None => {
                            saved_buf = buf.clone();
                            self.history.len() - 1
                        }
                        Some(i) if i > 0 => i - 1,
                        Some(i) => i,
                    };
                    hist_idx = Some(new_idx);
                    buf = self.history[new_idx].chars().collect();
                    cur = buf.len();
                    menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                }
                (KeyCode::Down, _) => {
                    match hist_idx {
                        None => {}
                        Some(i) if i + 1 < self.history.len() => {
                            let new_idx = i + 1;
                            hist_idx = Some(new_idx);
                            buf = self.history[new_idx].chars().collect();
                            cur = buf.len();
                            menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                        }
                        Some(_) => {
                            hist_idx = None;
                            buf = saved_buf.clone();
                            cur = buf.len();
                            menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                        }
                    }
                }

                (KeyCode::Tab, _) => {
                    let current: String = buf.iter().collect();
                    if !current.starts_with('/') {
                        continue;
                    }
                    // Accept the top match (ghost text).
                    if let Some(top) = ghost_for(&buf) {
                        buf = top.chars().collect();
                        cur = buf.len();
                        menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                    }
                }

                (KeyCode::Char(c), _) => {
                    hist_idx = None;
                    buf.insert(cur, c);
                    cur += 1;
                    menu_lines = redraw_with_menu(stdout, prompt, prompt_visible_len, &buf, cur, menu_lines)?;
                }

                _ => {}
            }
        }
    }
}

// ── Ghost text (inline suggestion) ──────────────────────────────────────────

/// Return the best-matching slash command for the current buffer, or None.
fn ghost_for(buf: &[char]) -> Option<String> {
    let current: String = buf.iter().collect();
    if !current.starts_with('/') || current.is_empty() {
        return None;
    }
    // Find the first command that starts with the current text (and is longer).
    SLASH_COMMANDS
        .iter()
        .find(|sc| sc.cmd.starts_with(&current) && sc.cmd.len() > current.len())
        .map(|sc| sc.cmd.to_string())
}

// ── Menu rendering ──────────────────────────────────────────────────────────

/// Get matching commands for the current buffer.
fn matching_commands(buf: &[char]) -> Vec<&'static SlashCmd> {
    let current: String = buf.iter().collect();
    if !current.starts_with('/') {
        return Vec::new();
    }
    SLASH_COMMANDS
        .iter()
        .filter(|sc| sc.cmd.starts_with(&current))
        .take(MAX_MENU_ITEMS)
        .collect()
}

/// Clear `n` menu lines that were printed below the input line,
/// then move the cursor back to the input line.
fn clear_menu(stdout: &mut io::Stdout, n: usize) -> Result<()> {
    if n == 0 {
        return Ok(());
    }
    // Move down to each menu line and clear it, then come back up.
    for _ in 0..n {
        execute!(stdout, cursor::MoveDown(1), terminal::Clear(ClearType::CurrentLine))?;
    }
    // Move back up to the input line.
    execute!(stdout, cursor::MoveUp(n as u16))?;
    Ok(())
}

/// Redraw input line + ghost text + menu. Returns the new menu_lines count.
fn redraw_with_menu(
    stdout: &mut io::Stdout,
    prompt: &str,
    prompt_visible_len: usize,
    buf: &[char],
    cursor_pos: usize,
    prev_menu_lines: usize,
) -> Result<usize> {
    // First clear old menu.
    clear_menu(stdout, prev_menu_lines)?;

    let ghost = ghost_for(buf);

    // Redraw the input line with ghost text.
    redraw(stdout, prompt, prompt_visible_len, buf, cursor_pos, ghost.as_deref())?;

    // Show the menu below if the user is typing a slash command.
    let matches = matching_commands(buf);
    let current: String = buf.iter().collect();

    // Only show menu when actively typing a command (at least `/` + one char or just `/`)
    if current.starts_with('/') && !matches.is_empty() {
        // Don't show menu if there's exactly one match and it equals the current text.
        if matches.len() == 1 && matches[0].cmd == current {
            return Ok(0);
        }

        let menu_count = matches.len();
        // Print menu lines below current cursor position.
        for sc in &matches {
            let cmd_display = sc.cmd.truecolor(100, 200, 255);
            let desc_display = sc.desc.truecolor(100, 100, 120);
            // Move to next line and print.
            execute!(
                stdout,
                Print("\r\n"),
                terminal::Clear(ClearType::CurrentLine),
                Print(format!("    {cmd_display}  {desc_display}")),
            )?;
        }
        // Move cursor back up to the input line, at the correct column.
        let col = (prompt_visible_len + 1 + cursor_pos) as u16;
        execute!(
            stdout,
            cursor::MoveUp(menu_count as u16),
            cursor::MoveToColumn(col),
        )?;
        stdout.flush()?;
        Ok(menu_count)
    } else {
        Ok(0)
    }
}

/// Redraw the **current** line only, with optional ghost completion text.
fn redraw(
    stdout: &mut io::Stdout,
    prompt: &str,
    prompt_visible_len: usize,
    buf: &[char],
    cursor_pos: usize,
    ghost: Option<&str>,
) -> Result<()> {
    let content: String = buf.iter().collect();
    let col = (prompt_visible_len + 1 + cursor_pos) as u16;

    // Build the ghost suffix: the part of the command after what the user typed.
    let ghost_suffix = match ghost {
        Some(g) if g.len() > content.len() => {
            let suffix = &g[content.len()..];
            format!("{}", suffix.truecolor(140, 140, 170))
        }
        _ => String::new(),
    };

    execute!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::CurrentLine),
        Print(format!("{} {}{}", prompt, content, ghost_suffix)),
        cursor::MoveToColumn(col),
    )?;
    Ok(())
}
