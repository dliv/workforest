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
        Command::Init {
            worktree_base,
            base_branch,
            branch_template,
            username,
            repos,
            force,
            show_path,
        } => {
            let config_path = config::default_config_path()?;
            if show_path {
                println!("{}", config_path.display());
                return Ok(());
            }

            let username = username.unwrap_or_else(|| {
                eprintln!("error: --username is required\nHint: git forest init --username <your-name> --repo <path>");
                std::process::exit(1);
            });

            let repo_inputs = repos
                .into_iter()
                .map(|r| commands::RepoInput {
                    path: r,
                    name: None,
                    base_branch: None,
                })
                .collect();

            let inputs = commands::InitInputs {
                worktree_base,
                base_branch,
                branch_template,
                username,
                repos: repo_inputs,
            };

            let result = commands::cmd_init(inputs, &config_path, force)?;
            output(&result, cli.json, commands::format_init_human)?;
        }
        Command::New { name, .. } => {
            eprintln!("new {}: not yet implemented", name);
        }
        Command::Rm { name } => {
            let label = name.as_deref().unwrap_or("(auto-detect)");
            eprintln!("rm {}: not yet implemented", label);
        }
        Command::Ls => {
            let config = config::load_default_config()?;
            let result = commands::cmd_ls(&config.general.worktree_base)?;
            output(&result, cli.json, commands::format_ls_human)?;
        }
        Command::Status { name } => {
            let config = config::load_default_config()?;
            let (dir, meta) =
                forest::resolve_forest(&config.general.worktree_base, name.as_deref())?;
            let result = commands::cmd_status(&dir, &meta)?;
            output(&result, cli.json, commands::format_status_human)?;
        }
        Command::Exec { name, cmd } => {
            let config = config::load_default_config()?;
            let (dir, meta) =
                forest::resolve_forest(&config.general.worktree_base, Some(name.as_str()))?;
            let result = commands::cmd_exec(&dir, &meta, &cmd)?;
            let has_failures = !result.failures.is_empty();
            output(&result, cli.json, commands::format_exec_human)?;
            if has_failures {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn output<T: serde::Serialize>(result: &T, json: bool, human_fn: fn(&T) -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        let text = human_fn(result);
        if !text.is_empty() {
            println!("{}", text);
        }
    }
    Ok(())
}
