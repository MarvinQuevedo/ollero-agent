//! Claude Code–style welcome panel (boxed header, tips, accent color).

use std::path::Path;

use colored::Colorize;

/// Claude Code–style orange accent (RGB).
pub fn accent(s: &str) -> colored::ColoredString {
    s.truecolor(217, 119, 38)
}

pub fn accent_dim(s: &str) -> colored::ColoredString {
    s.truecolor(180, 100, 45).dimmed()
}

/// Shown on the line **above** `>` while editing (keyboard discoverability; avoids cursor bugs under the prompt).
pub const INPUT_FOOTER: &str = "Ctrl+D exit (empty line) · /help · /quit · Ctrl+C clear line";

fn user_display_name() -> String {
    if cfg!(windows) {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "there".into())
    } else {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "there".into())
    }
}

/// ASCII robot (compact, works in all fonts).
const ROBOT: &str = "  o o\n   > ^\n  / \\";

pub fn print_welcome(version: &str, model: &str, workspace: &Path) {
    let user = user_display_name();
    let cwd = workspace.display().to_string();
    let v = accent("│");
    println!();
    println!(
        "{}",
        accent(&format!(
            "╭─ Ollero v{version} ─ local agent (Ollama) ─────────────────────────╮"
        ))
    );
    println!("{}", accent("│"));
    println!("{} {}", v, format!("Welcome back, {}!", user).bold());
    println!("{}", accent("│"));
    for line in ROBOT.lines() {
        println!("{}{}", v, accent(line));
    }
    println!("{}", accent("│"));
    println!(
        "{}{}",
        v,
        format!("{model} · {cwd}").dimmed()
    );
    println!("{}", accent("│"));
    println!("{}", accent("├────────────────────────────────┬───────────────────────────────────┤"));
    println!(
        "{}{}",
        v,
        " Tips for getting started".dimmed()
    );
    println!(
        "{}{}",
        v,
        "   /help — list commands".dimmed()
    );
    println!(
        "{}{}",
        v,
        "   Ctrl+D on an empty line — exit".dimmed()
    );
    println!(
        "{}{}",
        v,
        "   /model list — pick an Ollama model".dimmed()
    );
    println!("{}", accent("│"));
    println!(
        "{}{}",
        v,
        " Recent activity".dimmed()
    );
    println!(
        "{}{}",
        v,
        "   No recent activity (new session)".dimmed()
    );
    println!("{}", accent("╰────────────────────────────────┴───────────────────────────────────╯"));
    println!(
        "{}",
        accent_dim("* Chat-only models: use ```bash blocks — Ollero can offer to run them.")
    );
    println!();
}
