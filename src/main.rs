mod app;
mod backend;
mod store;
mod ui;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "pass-tui", version, about = "TUI frontend for pass")]
struct Cli {
    /// Path to password store directory
    #[arg(long, global = true)]
    store: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut app = app::App::new_with_store(cli.store)?;
    ui::run_tui(&mut app)
}
