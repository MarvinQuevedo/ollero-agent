mod config;
mod input;
mod ollama;
mod permissions;
mod repl;
mod setup;
mod tools;
mod workspace;

use std::env;

use anyhow::Result;
use config::Config;
use repl::Repl;

#[tokio::main]
async fn main() -> Result<()> {
    let config = match Config::load()? {
        Some(cfg) => cfg,
        None => setup::run_wizard().await?,
    };

    let workspace_root = env::current_dir()?;

    let mut repl = Repl::new(config, workspace_root);
    repl.run().await
}
