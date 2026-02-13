mod cli;
mod config;
mod forest;
mod git;
mod meta;
mod paths;
mod testutil;

use clap::Parser;
use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();

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
            eprintln!("ls: not yet implemented");
        }
        Command::Status { name } => {
            let label = name.as_deref().unwrap_or("(auto-detect)");
            eprintln!("status {}: not yet implemented", label);
        }
        Command::Exec { name, cmd } => {
            eprintln!("exec {} -- {:?}: not yet implemented", name, cmd);
        }
    }
}
