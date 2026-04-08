//! Allux CLI binary.
//!
//! This binary provides the command-line interface for the Allux tool.

mod compression;
mod config;
#[allow(dead_code)]
mod input;
#[allow(dead_code)]
mod doctor;
mod monitor;
mod ollama;
mod permissions;
#[allow(dead_code)]
mod repl;
mod session;
mod setup;
mod tools;
mod tui;
mod workspace;

use std::env;

use anyhow::Result;
use config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    // Ignore Ctrl+C globally so it never force-quits the CLI.
    // (In raw mode, it's captured as a key event and clears the line).
    tokio::spawn(async {
        loop {
            let _ = tokio::signal::ctrl_c().await;
        }
    });

    let config = match Config::load()? {
        Some(cfg) => cfg,
        None => setup::run_wizard().await?,
    };

    let workspace_root = env::current_dir()?;
    let metrics = monitor::new_shared();
    monitor::spawn_collector(metrics.clone());

    // Launch the TUI
    tui::run(config, workspace_root, metrics).await
}
