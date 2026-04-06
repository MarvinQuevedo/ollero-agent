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

const SLASH_COMMANDS: &[&str] = &[
    "/help",
    "/quit",
    "/exit",
    "/q",
    "/clear",
    "/history",
    "/context",
    "/context refresh",
    "/model",
    "/model list",
];

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

        loop {
            let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }

            match (code, modifiers) {
                (KeyCode::Enter, _) => {
                    return Ok(Some(buf.iter().collect()));
                }

                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        return Ok(None);
                    }
                }

                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    buf.clear();
                    cur = 0;
                    hist_idx = None;
                    saved_buf.clear();
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                }

                (KeyCode::Backspace, _) => {
                    if cur > 0 {
                        buf.remove(cur - 1);
                        cur -= 1;
                        redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                    }
                }

                (KeyCode::Delete, _) => {
                    if cur < buf.len() {
                        buf.remove(cur);
                        redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                    }
                }

                (KeyCode::Left, _) => {
                    if cur > 0 {
                        cur -= 1;
                        redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                    }
                }
                (KeyCode::Right, _) => {
                    if cur < buf.len() {
                        cur += 1;
                        redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                    }
                }
                (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                    cur = 0;
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                }
                (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                    cur = buf.len();
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
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
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                }
                (KeyCode::Down, _) => {
                    match hist_idx {
                        None => {}
                        Some(i) if i + 1 < self.history.len() => {
                            let new_idx = i + 1;
                            hist_idx = Some(new_idx);
                            buf = self.history[new_idx].chars().collect();
                            cur = buf.len();
                            redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                        }
                        Some(_) => {
                            hist_idx = None;
                            buf = saved_buf.clone();
                            cur = buf.len();
                            redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                        }
                    }
                }

                (KeyCode::Tab, _) => {
                    let current: String = buf.iter().collect();
                    if !current.starts_with('/') {
                        continue;
                    }
                    let matches: Vec<&str> = SLASH_COMMANDS
                        .iter()
                        .copied()
                        .filter(|c| c.starts_with(current.as_str()))
                        .collect();
                    match matches.len() {
                        0 => {}
                        1 => {
                            buf = matches[0].chars().collect();
                            cur = buf.len();
                            redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                        }
                        _ => {
                            terminal::disable_raw_mode()?;
                            println!();
                            for m in &matches {
                                println!("  {m}");
                            }
                            print!("{} ", prompt);
                            stdout.flush()?;
                            terminal::enable_raw_mode()?;
                            redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                        }
                    }
                }

                (KeyCode::Char(c), _) => {
                    hist_idx = None;
                    buf.insert(cur, c);
                    cur += 1;
                    redraw(stdout, prompt, prompt_visible_len, &buf, cur)?;
                }

                _ => {}
            }
        }
    }
}

/// Redraw the **current** line only: full terminal width, cursor column from visible lengths.
fn redraw(
    stdout: &mut io::Stdout,
    prompt: &str,
    prompt_visible_len: usize,
    buf: &[char],
    cursor_pos: usize,
) -> Result<()> {
    let content: String = buf.iter().collect();
    let col = (prompt_visible_len + 1 + cursor_pos) as u16;

    execute!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::CurrentLine),
        Print(format!("{} {}", prompt, content)),
        cursor::MoveToColumn(col),
    )?;
    Ok(())
}
