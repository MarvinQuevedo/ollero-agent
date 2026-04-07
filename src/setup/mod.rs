use std::io::{stdout, Write};

use anyhow::Result;
use colored::Colorize;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, ClearType},
};

use crate::{
    config::{Config, CONFIG_VERSION},
    ollama::client::OllamaClient,
};

/// Run the first-time setup wizard. Returns the configured Config.
pub async fn run_wizard() -> Result<Config> {
    print_banner();

    // ── Step 1: Ollama URL ────────────────────────────────────────────────
    println!("{}", "  Step 1/2 — Ollama connection".bold().white());
    println!(
        "{}",
        "  Allux uses a local Ollama server to run AI models.".dimmed()
    );
    println!(
        "{}",
        "  If you haven't installed Ollama yet: https://ollama.com\n".dimmed()
    );

    let ollama_url = ask_ollama_url()?;

    // ── Connect & fetch models ────────────────────────────────────────────
    print!("\n  {} Connecting to Ollama at {}...", "·".cyan(), ollama_url.cyan());
    stdout().flush()?;

    let models = match OllamaClient::list_models(&ollama_url).await {
        Ok(m) => {
            println!(" {}", "connected".green().bold());
            m
        }
        Err(e) => {
            println!(" {}", "failed".red().bold());
            println!();
            println!("  {} Could not reach Ollama:", "✗".red().bold());
            println!("    {}", e.to_string().dimmed());
            println!();
            println!("  Make sure Ollama is running:");
            println!("    {}", "ollama serve".cyan());
            println!();
            return Err(e);
        }
    };

    if models.is_empty() {
        println!();
        println!("  {} No models installed.", "✗".red().bold());
        println!("  Download one first, for example:");
        println!("    {}", "ollama pull llama3.2".cyan());
        anyhow::bail!("No models available.");
    }

    println!(
        "  {} Found {} model{}.\n",
        "✓".green().bold(),
        models.len(),
        if models.len() == 1 { "" } else { "s" }
    );

    // ── Step 2: Model selection ───────────────────────────────────────────
    println!("{}", "  Step 2/2 — Choose a model".bold().white());
    println!(
        "{}",
        "  This will be your default. You can change it later with /model <name>.\n".dimmed()
    );

    let model = select_model(&models)?;

    // ── Save ──────────────────────────────────────────────────────────────
    let config = Config {
        config_version: CONFIG_VERSION.to_string(),
        ollama_url,
        model,
        context_size: 8192,
        compression_mode: "auto".to_string(),
    };
    config.save()?;

    println!();
    println!(
        "  {} Setup complete! Config saved to:",
        "✓".green().bold()
    );
    println!("    {}\n", Config::config_path().display().to_string().dimmed());
    println!("{}", "  ─────────────────────────────────────".dimmed());
    println!();

    Ok(config)
}

fn print_banner() {
    // Same pixel-art ALLUX logo as the REPL, with El Salvador flag gradient.
    const LOGO: [&str; 5] = [
        "   ██    ██       ██       ██    ██ ██    ██",
        "  ████   ██       ██       ██    ██  ██  ██ ",
        " ██  ██  ██       ██       ██    ██   ████  ",
        "████████ ██       ██       ██    ██  ██  ██ ",
        "██    ██ ████████ ████████  ██████  ██    ██",
    ];
    const COLORS: [(u8, u8, u8); 5] = [
        ( 80, 140, 240),  // bright blue (top)
        (120, 170, 240),  // light blue
        (240, 245, 255),  // white (center)
        (120, 170, 240),  // light blue
        ( 80, 140, 240),  // bright blue (bottom)
    ];

    println!();
    for (line, &(r, g, b)) in LOGO.iter().zip(COLORS.iter()) {
        println!("  {}", line.truecolor(r, g, b).bold());
    }
    println!();
    println!("  {}", "Local code agent powered by Ollama".bold());
    println!("  {}", "─────────────────────────────────────────────".truecolor(100, 149, 237));
    println!();
    println!("  {}", "Welcome! First-time setup — takes 30 seconds.".bold());
    println!();
}

fn ask_ollama_url() -> Result<String> {
    let default = "http://localhost:11434";
    loop {
        print!(
            "  {} {} {}: ",
            "Ollama URL".bold(),
            "(press Enter for default)".dimmed(),
            format!("[{default}]").dimmed()
        );
        stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_string();

        if input.is_empty() {
            println!("  {} Using default: {}", "→".dimmed(), default.cyan());
            return Ok(default.into());
        }

        if validate_url(&input) {
            return Ok(input);
        }

        println!(
            "  {} That doesn't look like a valid URL.",
            "✗".red().bold()
        );
        println!(
            "  {} Examples: {}  or  {}",
            " ".dimmed(),
            "http://localhost:11434".dimmed(),
            "http://192.168.1.5:11434".dimmed()
        );
        println!();
    }
}

pub fn validate_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Interactive arrow-key model selector. Returns the chosen model name.
fn select_model(models: &[crate::ollama::types::ModelInfo]) -> Result<String> {
    println!("  {}", "Use ↑ ↓ arrows to navigate, Enter to confirm:\n".dimmed());

    let mut selected: usize = 0;

    terminal::enable_raw_mode()?;
    let mut stdout = stdout();

    draw_model_list(&mut stdout, models, selected, false)?;

    loop {
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    terminal::disable_raw_mode()?;
                    println!();
                    anyhow::bail!("Setup cancelled.");
                }

                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if selected > 0 {
                            selected -= 1;
                            draw_model_list(&mut stdout, models, selected, true)?;
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if selected < models.len() - 1 {
                            selected += 1;
                            draw_model_list(&mut stdout, models, selected, true)?;
                        }
                    }
                    KeyCode::Enter => break,
                    _ => {}
                }
            }
        }
    }

    terminal::disable_raw_mode()?;

    // Clear the list and print the confirmed selection
    execute!(
        stdout,
        cursor::MoveUp(models.len() as u16),
        terminal::Clear(ClearType::FromCursorDown),
    )?;

    let chosen = &models[selected];
    println!(
        "  {} {}  {}",
        "✓ Model:".green().bold(),
        chosen.name.cyan().bold(),
        format!("{} {}", chosen.details.parameter_size, chosen.details.quantization_level).dimmed()
    );

    Ok(chosen.name.clone())
}

fn draw_model_list(
    stdout: &mut std::io::Stdout,
    models: &[crate::ollama::types::ModelInfo],
    selected: usize,
    is_redraw: bool,
) -> Result<()> {
    if is_redraw {
        execute!(stdout, cursor::MoveUp(models.len() as u16))?;
    }

    for (i, model) in models.iter().enumerate() {
        let size_info = format!("{} {}", model.details.parameter_size, model.details.quantization_level);
        let line = if i == selected {
            format!(
                "  {} {}  {}",
                "\u{25B6}".cyan().bold(),
                model.name.bold(),
                size_info.dimmed()
            )
        } else {
            format!(
                "    {}  {}",
                model.name.dimmed(),
                size_info.dimmed()
            )
        };
        // In raw mode \n does NOT return to column 0, so use crossterm.
        execute!(
            stdout,
            cursor::MoveToColumn(0),
            terminal::Clear(ClearType::CurrentLine),
            crossterm::style::Print(line),
            cursor::MoveToNextLine(1),
        )?;
    }
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_accepts_http() {
        assert!(validate_url("http://localhost:11434"));
        assert!(validate_url("http://192.168.1.10:11434"));
    }

    #[test]
    fn test_validate_url_accepts_https() {
        assert!(validate_url("https://my-ollama.example.com"));
    }

    #[test]
    fn test_validate_url_rejects_bare_words() {
        assert!(!validate_url("hola"));
        assert!(!validate_url("localhost:11434"));
        assert!(!validate_url(""));
        assert!(!validate_url("ftp://something"));
    }
}
