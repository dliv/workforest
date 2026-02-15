mod cli;
mod commands;
mod config;
mod forest;
mod git;
mod meta;
mod paths;
mod testutil;

use anyhow::{bail, Result};
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
            feature_branch_template,
            repos,
            repo_base_branches,
            force,
            show_path,
        } => {
            let config_path = config::default_config_path()?;
            if show_path {
                println!("{}", config_path.display());
                return Ok(());
            }

            let feature_branch_template = feature_branch_template.unwrap_or_else(|| {
                eprintln!("error: --feature-branch-template is required\n  hint: git forest init --feature-branch-template \"yourname/{{name}}\" --repo <path>");
                std::process::exit(1);
            });

            // Parse --repo-base-branch strings ("repo=branch") into a lookup
            let mut base_branch_overrides = std::collections::HashMap::new();
            for rbb in repo_base_branches {
                match rbb.split_once('=') {
                    Some((repo, branch)) => {
                        base_branch_overrides.insert(repo.to_string(), branch.to_string());
                    }
                    None => {
                        bail!(
                            "invalid --repo-base-branch format: {:?}\n  hint: use --repo-base-branch repo-name=branch",
                            rbb
                        );
                    }
                }
            }

            let repo_inputs = repos
                .into_iter()
                .map(|r| {
                    // Derive name from path for base_branch lookup
                    let name = std::path::Path::new(&r)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let repo_base = base_branch_overrides.remove(&name);
                    commands::RepoInput {
                        path: r,
                        name: None,
                        base_branch: repo_base,
                    }
                })
                .collect();

            // Warn about unmatched --repo-base-branch keys
            if !base_branch_overrides.is_empty() {
                let unknown: Vec<&str> = base_branch_overrides.keys().map(|k| k.as_str()).collect();
                bail!(
                    "unknown repo(s) in --repo-base-branch: {}\n  hint: repo names are derived from the last path segment of --repo",
                    unknown.join(", ")
                );
            }

            let inputs = commands::InitInputs {
                worktree_base,
                base_branch,
                feature_branch_template,
                repos: repo_inputs,
            };

            let result = commands::cmd_init(inputs, &config_path, force)?;
            output(&result, cli.json, commands::format_init_human)?;
        }
        Command::New {
            name,
            mode,
            branch,
            repo_branches,
            no_fetch,
            dry_run,
        } => {
            let config = config::load_default_config()?;

            // Parse --repo-branch strings ("repo=branch") into tuples
            let mut parsed_repo_branches = Vec::new();
            for rb in repo_branches {
                match rb.split_once('=') {
                    Some((repo, br)) => {
                        parsed_repo_branches.push((repo.to_string(), br.to_string()));
                    }
                    None => {
                        bail!(
                            "invalid --repo-branch format: {:?}\n  hint: use --repo-branch repo-name=branch-name",
                            rb
                        );
                    }
                }
            }

            let inputs = commands::NewInputs {
                name,
                mode,
                branch_override: branch,
                repo_branches: parsed_repo_branches,
                no_fetch,
                dry_run,
            };

            let result = commands::cmd_new(inputs, &config)?;
            output(&result, cli.json, commands::format_new_human)?;
        }
        Command::Rm {
            name,
            force,
            dry_run,
        } => {
            let config = config::load_default_config()?;
            let (dir, meta) =
                forest::resolve_forest(&config.general.worktree_base, name.as_deref())?;
            let result = commands::cmd_rm(&dir, &meta, force, dry_run)?;
            let has_errors = !result.errors.is_empty();
            output(&result, cli.json, commands::format_rm_human)?;
            if has_errors {
                std::process::exit(1);
            }
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
