// Ollero CLI - Local code agent powered by Ollama
// This is a small safe change: added comment documentation

use colored::{Colorize};
use std::env;

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    println!("{} v{}", "Ollero CLI".bold().green(), version);
}
