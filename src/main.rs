mod cli;
mod commands;
mod config;
mod forest;
mod git;
mod meta;
mod paths;
mod testutil;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init => {
            eprintln!("init: not yet implemented");
        }
        Command::New { name } => {
            eprintln!("new {}: not yet implemented", name);
        }
        Command::Rm { name } => {
            let label = name.as_deref().unwrap_or("(auto-detect)");
            eprintln!("rm {}: not yet implemented", label);
        }
        Command::Ls => {
            let config = config::load_default_config()?;
            commands::cmd_ls(&config.general.worktree_base)?;
        }
        Command::Status { name } => {
            let config = config::load_default_config()?;
            let (dir, meta) =
                forest::resolve_forest(&config.general.worktree_base, name.as_deref())?;
            commands::cmd_status(&dir, &meta)?;
        }
        Command::Exec { name, cmd } => {
            let config = config::load_default_config()?;
            let (dir, meta) =
                forest::resolve_forest(&config.general.worktree_base, Some(name.as_str()))?;
            commands::cmd_exec(&dir, &meta, &cmd)?;
        }
    }
    Ok(())
}
