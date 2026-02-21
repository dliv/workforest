#![warn(clippy::all)]

mod channel;
mod cli;
mod commands;
mod config;
mod forest;
mod git;
mod meta;
mod paths;
mod testutil;
mod version_check;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{Cli, Command};

fn main() {
    // Internal subprocess entry point for non-blocking version check.
    // Intercepted before CLI parsing — not a real subcommand.
    if std::env::args().any(|a| a == channel::INTERNAL_VERSION_CHECK_ARG) {
        version_check::run_background_version_check();
        return;
    }

    let cli = Cli::parse();
    let debug = cli.debug;

    let should_version_check = matches!(
        cli.command,
        Command::Init { .. }
            | Command::New { .. }
            | Command::Rm { .. }
            | Command::Ls
            | Command::Status { .. }
            | Command::Exec { .. }
    );

    if let Err(e) = run(cli) {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }

    if should_version_check {
        version_check::check_cache_and_notify(debug);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init {
            template,
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

            let feature_branch_template = match feature_branch_template {
                Some(t) => t,
                None => {
                    bail!("--feature-branch-template is required\n  hint: git forest init --feature-branch-template \"yourname/{{name}}\" --repo <path>");
                }
            };

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
                template_name: template,
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
            template,
            branch,
            repo_branches,
            no_fetch,
            dry_run,
        } => {
            let config = config::load_default_config()?;
            let tmpl = config.resolve_template(template.as_deref())?;

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

            let result = commands::cmd_new(inputs, tmpl)?;
            output(&result, cli.json, commands::format_new_human)?;
        }
        Command::Rm {
            name,
            force,
            dry_run,
        } => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let (dir, meta) = forest::resolve_forest_multi(&bases, name.as_deref())?;
            let result = if cli.json || dry_run {
                let r = commands::cmd_rm(&dir, &meta, force, dry_run, None)?;
                output(&r, cli.json, commands::format_rm_human)?;
                r
            } else {
                use std::io::Write;
                println!("Removing forest {:?}", meta.name.as_str());
                let r = commands::cmd_rm(
                    &dir,
                    &meta,
                    force,
                    dry_run,
                    Some(&|progress| match progress {
                        commands::RmProgress::RepoStarting { name } => {
                            print!("  {}: removing...", name);
                            std::io::stdout().flush().ok();
                        }
                        commands::RmProgress::RepoDone(repo) => {
                            println!(" {}", commands::format_repo_done(repo));
                        }
                    }),
                )?;
                println!("{}", commands::format_rm_summary(&r));
                r
            };
            if !result.errors.is_empty() {
                std::process::exit(1);
            }
        }
        Command::Reset {
            confirm,
            config_only,
            dry_run,
        } => {
            let result = if cli.json {
                let r = commands::cmd_reset(confirm, config_only, dry_run, None)?;
                output(&r, true, commands::format_reset_human)?;
                r
            } else if dry_run || !confirm {
                let r = commands::cmd_reset(confirm, config_only, dry_run, None)?;
                let text = commands::format_reset_human(&r);
                if !text.is_empty() {
                    println!("{}", text);
                }
                r
            } else {
                use std::io::Write;
                if !config_only {
                    println!("Forests:");
                }
                let r = commands::cmd_reset(
                    confirm,
                    config_only,
                    dry_run,
                    Some(&|progress| match progress {
                        commands::ResetProgress::ForestStarting { name, path } => {
                            print!("  Removing {} ({})...", name, path.display());
                            std::io::stdout().flush().ok();
                        }
                        commands::ResetProgress::ForestDone(entry) => {
                            if entry.removed {
                                println!(" done");
                            } else {
                                println!(" FAILED");
                            }
                        }
                    }),
                )?;
                println!("{}", commands::format_reset_summary(&r));
                r
            };
            if !result.errors.is_empty() || result.confirm_required {
                std::process::exit(1);
            }
        }
        Command::Ls => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let result = commands::cmd_ls(&bases)?;
            output(&result, cli.json, commands::format_ls_human)?;
        }
        Command::Status { name } => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let (dir, meta) = forest::resolve_forest_multi(&bases, name.as_deref())?;
            let result = commands::cmd_status(&dir, &meta)?;
            output(&result, cli.json, commands::format_status_human)?;
        }
        Command::Exec { name, cmd } => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let (dir, meta) = forest::resolve_forest_multi(&bases, Some(name.as_str()))?;
            let result = commands::cmd_exec(&dir, &meta, &cmd)?;
            let has_failures = !result.failures.is_empty();
            output(&result, cli.json, commands::format_exec_human)?;
            if has_failures {
                std::process::exit(1);
            }
        }
        Command::AgentInstructions => {
            print!("{}", include_str!("../docs/agent-instructions.md"));
        }
        Command::Version { check } => {
            println!("{} {}", channel::APP_NAME, env!("CARGO_PKG_VERSION"));
            if check {
                if !version_check::is_enabled() {
                    eprintln!("Version check is disabled in config.");
                } else {
                    match version_check::force_check(cli.debug) {
                        version_check::ForceCheckResult::UpdateAvailable(notice) => {
                            eprintln!(
                                "Update available: {} v{} (current: v{}).",
                                channel::APP_NAME,
                                notice.latest,
                                notice.current
                            );
                        }
                        version_check::ForceCheckResult::UpToDate => {
                            eprintln!("You are up to date.");
                        }
                        version_check::ForceCheckResult::FetchFailed => {
                            eprintln!("Could not reach the update server.");
                        }
                    }
                }
            }
        }
        Command::Update => {
            let brew_check = std::process::Command::new("brew")
                .args(["--prefix", channel::APP_NAME])
                .output();

            if brew_check.map(|o| o.status.success()).unwrap_or(false) {
                println!("Updating via Homebrew...");
                // Fetch latest formula before upgrading
                let _ = std::process::Command::new("brew")
                    .args(["update", "--quiet"])
                    .status();
                let status = std::process::Command::new("brew")
                    .args(["upgrade", channel::APP_NAME])
                    .status()?;
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
            } else {
                println!("Download the latest release:");
                println!("  https://github.com/dliv/workforest/releases/latest");
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
