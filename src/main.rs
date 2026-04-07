//! Allux CLI binary.
//!
//! This binary provides the command-line interface for the Allux tool.

mod compression;
mod config;
mod input;
mod ollama;
mod permissions;
mod repl;
mod session;
mod setup;
mod tools;
mod workspace;

use std::env;

use anyhow::Result;
use config::Config;
use repl::Repl;

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
    let mut repl = Repl::new(config, workspace_root);
    repl.run().await
}
