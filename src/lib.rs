#![warn(clippy::all)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        reason = "unit tests build real git repositories and read the environment; the non-test build still lints production code"
    )
)]

pub mod channel;
mod cli;
mod commands;
pub(crate) mod config;
mod forest;
mod git;
mod testutil;
pub(crate) mod version_check;

use std::ffi::OsString;
use std::process::ExitCode;

use anyhow::{anyhow, Result};
use clap::Parser;
use cli::{Cli, Command};

/// A failure that ends a command: what `main_entry` renders and maps to an exit code.
enum Failure {
    /// A flag-format error that `run` raises itself, before any command runs.
    Usage {
        message: String,
        hint: Option<String>,
    },
    /// clap's own error for the command line.
    Clap(clap::Error),
    /// Any other error, carried with anyhow's message chain.
    Command(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Failure::Command(error)
    }
}

/// The closed set of failure codes in the JSON failure document.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum FailureCode {
    /// The command line was malformed: clap's errors and `run`'s own flag-format errors.
    Usage,
    /// Any other failure.
    CommandFailed,
}

impl Failure {
    fn code(&self) -> FailureCode {
        match self {
            Failure::Usage { .. } | Failure::Clap(_) => FailureCode::Usage,
            Failure::Command(_) => FailureCode::CommandFailed,
        }
    }

    /// The message and hint of the JSON failure document. A command error's text is split at
    /// its first `\n  hint: `; a clap error gives its first paragraph and its `Usage: ` line.
    fn message_and_hint(&self) -> (String, Option<String>) {
        match self {
            Failure::Usage { message, hint } => (message.clone(), hint.clone()),
            Failure::Command(error) => {
                let text = format!("{error:#}");
                match text.split_once("\n  hint: ") {
                    Some((message, hint)) => (message.to_owned(), Some(hint.to_owned())),
                    None => (text, None),
                }
            }
            Failure::Clap(error) => {
                let rendered = error.render().to_string();
                let text = rendered.strip_prefix("error: ").unwrap_or(&rendered);
                let message = text.split("\n\n").next().unwrap_or(text).trim().to_owned();
                let hint = text
                    .lines()
                    .find(|line| line.starts_with("Usage: "))
                    .map(ToOwned::to_owned);
                (message, hint)
            }
        }
    }
}

/// The JSON document a failure prints under `--json`.
#[derive(serde::Serialize)]
struct FailureDocument {
    error: FailureBody,
}

#[derive(serde::Serialize)]
struct FailureBody {
    code: FailureCode,
    message: String,
    hint: Option<String>,
}

/// How a command that ran to its end finished, which picks its exit code.
enum Completion {
    /// Everything the command set out to do was done.
    Complete,
    /// The command ran but left part of its work undone: `rm` or `reset` errors, failed
    /// forests of `rm --all`, or `exec` failures.
    Incomplete,
    /// `reset` without `--confirm`: the preview was printed and nothing was removed.
    ConfirmRequired,
}

/// The only mapping from a command's outcome to the process exit code.
fn exit_code(outcome: &Result<Completion, Failure>) -> ExitCode {
    match outcome {
        Ok(Completion::Complete) => ExitCode::SUCCESS,
        Ok(Completion::Incomplete) | Ok(Completion::ConfirmRequired) => ExitCode::from(1),
        Err(Failure::Command(_)) => ExitCode::from(1),
        Err(Failure::Usage { .. }) | Err(Failure::Clap(_)) => ExitCode::from(2),
    }
}

/// The only failure renderer. Under `--json`, one JSON document on standard output and
/// nothing on standard error; otherwise clap's errors through clap and every other failure as
/// one `error: ...` block on standard error.
#[expect(
    clippy::print_stderr,
    reason = "composition root: the single failure renderer"
)]
fn report_failure(failure: &Failure, json: bool) {
    if json {
        let (message, hint) = failure.message_and_hint();
        let document = FailureDocument {
            error: FailureBody {
                code: failure.code(),
                message,
                hint,
            },
        };
        if print_json(&document).is_ok() {
            return;
        }
    }
    let text = match failure {
        Failure::Clap(error) => {
            let _ = error.print();
            return;
        }
        Failure::Usage {
            message,
            hint: Some(hint),
        } => format!("{message}\n  hint: {hint}"),
        Failure::Usage {
            message,
            hint: None,
        } => message.clone(),
        Failure::Command(error) => format!("{error:#}"),
    };
    eprintln!("error: {text}");
}

#[expect(
    clippy::disallowed_methods,
    reason = "composition root: reads process arguments and the clock once, and owns the exit code"
)]
pub fn main_entry() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();

    // Internal subprocess entry point for non-blocking version check.
    // Intercepted before CLI parsing — not a real subcommand.
    if args
        .iter()
        .any(|arg| arg == channel::INTERNAL_VERSION_CHECK_ARG)
    {
        version_check::run_background_version_check();
        return ExitCode::SUCCESS;
    }

    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) if error.exit_code() == 0 => {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            // `--json` counts when it comes before any `--`, which starts a command's own args.
            let json = args
                .iter()
                .skip(1)
                .take_while(|arg| *arg != "--")
                .any(|arg| arg == "--json");
            let outcome = Err(Failure::Clap(error));
            if let Err(failure) = &outcome {
                report_failure(failure, json);
            }
            return exit_code(&outcome);
        }
    };
    let debug = cli.debug;
    let json = cli.json;

    // The version check writes its state file, so dry-runs skip it: a dry-run's only change
    // is `new`'s fetch.
    let should_version_check = match &cli.command {
        Command::New { dry_run, .. } | Command::Rm { dry_run, .. } => !*dry_run,
        Command::Init { .. } | Command::Ls | Command::Status { .. } | Command::Exec { .. } => true,
        Command::Reset { .. }
        | Command::AgentInstructions
        | Command::Version { .. }
        | Command::Update => false,
    };

    let now = chrono::Utc::now();
    let outcome = run(cli, now);
    match &outcome {
        Ok(Completion::Complete) => {
            if should_version_check {
                version_check::check_cache_and_notify(debug);
            }
        }
        Ok(Completion::Incomplete) | Ok(Completion::ConfirmRequired) => {}
        Err(failure) => report_failure(failure, json),
    }
    exit_code(&outcome)
}

/// `rm` of one forest is incomplete when any cleanup step reported an error.
fn rm_completion(result: &commands::RmResult) -> Completion {
    if result.errors.is_empty() {
        Completion::Complete
    } else {
        Completion::Incomplete
    }
}

/// `rm --all` is incomplete when any forest failed.
fn rm_all_completion(result: &commands::RmAllResult) -> Completion {
    if result.failed > 0 {
        Completion::Incomplete
    } else {
        Completion::Complete
    }
}

/// `reset` needs `--confirm` before it removes anything, and is incomplete when a step failed.
fn reset_completion(result: &commands::ResetResult) -> Completion {
    if result.confirm_required {
        Completion::ConfirmRequired
    } else if !result.errors.is_empty() {
        Completion::Incomplete
    } else {
        Completion::Complete
    }
}

/// `exec` is incomplete when the command failed or could not run in any repo.
fn exec_completion(result: &commands::ExecResult) -> Completion {
    if result.failures.is_empty() {
        Completion::Complete
    } else {
        Completion::Incomplete
    }
}

#[expect(
    clippy::disallowed_methods,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "composition root: dispatches commands, prints results and progress, and spawns brew for update"
)]
fn run(cli: Cli, now: chrono::DateTime<chrono::Utc>) -> Result<Completion, Failure> {
    let completion = match cli.command {
        Command::Init {
            template,
            worktree_base,
            base_branch,
            feature_branch_template,
            repos,
            repo_base_branches,
            disposable_root_entries,
            force,
            show_path,
        } => {
            let config_path = config::default_config_path()?;
            if show_path {
                println!("{}", config_path.display());
                return Ok(Completion::Complete);
            }

            let feature_branch_template = match feature_branch_template {
                Some(t) => t,
                None => {
                    return Err(Failure::Usage {
                        message: "--feature-branch-template is required".to_owned(),
                        hint: Some(
                            "git forest init --feature-branch-template \"yourname/{name}\" --repo <path>"
                                .to_owned(),
                        ),
                    });
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
                        return Err(Failure::Usage {
                            message: format!("invalid --repo-base-branch format: {:?}", rbb),
                            hint: Some("use --repo-base-branch repo-name=branch".to_owned()),
                        });
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
                return Err(anyhow!(
                    "unknown repo(s) in --repo-base-branch: {}\n  hint: repo names are derived from the last path segment of --repo",
                    unknown.join(", ")
                )
                .into());
            }

            let inputs = commands::InitInputs {
                template_name: template,
                worktree_base,
                base_branch,
                feature_branch_template,
                disposable_root_entries,
                repos: repo_inputs,
            };

            let result = commands::cmd_init(inputs, &config_path, force)?;
            output(&result, cli.json, commands::format_init_human)?;
            Completion::Complete
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
                        return Err(Failure::Usage {
                            message: format!("invalid --repo-branch format: {:?}", rb),
                            hint: Some("use --repo-branch repo-name=branch-name".to_owned()),
                        });
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

            let result = commands::cmd_new_at(inputs, tmpl, now)?;
            output(&result, cli.json, commands::format_new_human)?;
            Completion::Complete
        }
        Command::Rm {
            name,
            all,
            force,
            discard_root_entries,
            dry_run,
        } => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let rm_options = commands::RmOptions {
                require_utf8_json_paths: cli.json,
                additional_disposable_root_entries: discard_root_entries,
                ..commands::RmOptions::new(force, dry_run)
            };

            if all {
                let result = if cli.json || dry_run {
                    let r = commands::cmd_rm_all_with_options(&bases, rm_options.clone(), None)?;
                    output(&r, cli.json, commands::format_rm_all_human)?;
                    r
                } else {
                    let r = commands::cmd_rm_all_with_options(
                        &bases,
                        rm_options,
                        Some(&|progress| match progress {
                            commands::RmAllProgress::ForestStarting { name } => {
                                println!("Removing forest {:?}", name.as_str());
                            }
                            commands::RmAllProgress::ForestDone(_name, result) => {
                                for repo in &result.repos {
                                    println!(
                                        "  {}: {}",
                                        repo.name,
                                        commands::format_repo_done(repo)
                                    );
                                }
                                println!("{}", commands::format_rm_summary(result));
                            }
                        }),
                    )?;
                    println!("{}", commands::format_rm_all_summary(&r));
                    r
                };
                rm_all_completion(&result)
            } else {
                let (dir, meta) = forest::resolve_forest_multi(&bases, name.as_deref())?;
                let result = if cli.json || dry_run {
                    let r = commands::cmd_rm_with_options(&dir, &meta, rm_options.clone(), None)?;
                    output(&r, cli.json, commands::format_rm_human)?;
                    r
                } else {
                    use std::io::Write;
                    println!("Removing forest {:?}", meta.name.as_str());
                    let r = commands::cmd_rm_with_options(
                        &dir,
                        &meta,
                        rm_options,
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
                rm_completion(&result)
            }
        }
        Command::Reset {
            confirm,
            config_only,
            dry_run,
        } => {
            let result = if cli.json {
                let r = commands::cmd_reset(confirm, config_only, dry_run, now, None)?;
                output(&r, true, commands::format_reset_human)?;
                r
            } else if dry_run || !confirm {
                let r = commands::cmd_reset(confirm, config_only, dry_run, now, None)?;
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
                    now,
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
            reset_completion(&result)
        }
        Command::Ls => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let result = commands::cmd_ls(&bases, now)?;
            output(&result, cli.json, commands::format_ls_human)?;
            Completion::Complete
        }
        Command::Status { name } => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let (dir, meta) = forest::resolve_forest_multi(&bases, name.as_deref())?;
            let result = commands::cmd_status(&dir, &meta)?;
            output(&result, cli.json, commands::format_status_human)?;
            Completion::Complete
        }
        Command::Exec { name, cmd } => {
            let config = config::load_default_config()?;
            let bases = config.all_worktree_bases();
            let (dir, meta) = forest::resolve_forest_multi(&bases, Some(name.as_str()))?;
            // Under --json the child's stdout goes to stderr, so stdout holds only the document.
            let child_stdout_to_stderr = || std::process::Stdio::from(std::io::stderr());
            let child_stdout: &dyn Fn() -> std::process::Stdio = if cli.json {
                &child_stdout_to_stderr
            } else {
                &std::process::Stdio::inherit
            };
            let result = commands::cmd_exec(
                &dir,
                &meta,
                &cmd,
                child_stdout,
                Some(&|progress| match progress {
                    commands::ExecProgress::RepoStarting { name } => {
                        eprintln!("=== {} ===", name);
                    }
                    commands::ExecProgress::RepoFailed(failure) => match failure.code {
                        commands::ExecFailureCode::WorktreeMissing => {
                            eprintln!("  warning: {}", failure.message);
                        }
                        commands::ExecFailureCode::SpawnFailed => {
                            eprintln!("  error: {}", failure.message);
                        }
                        commands::ExecFailureCode::CommandExited => {}
                    },
                }),
            )?;
            output(&result, cli.json, commands::format_exec_human)?;
            exec_completion(&result)
        }
        Command::AgentInstructions => {
            let instructions = include_str!("../docs/agent-instructions.md");
            #[cfg(feature = "beta")]
            let instructions = instructions
                .replace("git-forest", "git-forest-beta")
                .replace("git forest", "git forest-beta");
            print!("{}", instructions);
            Completion::Complete
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
            Completion::Complete
        }
        Command::Update => {
            // Homebrew's output is not JSON, so update refuses --json before printing anything.
            if cli.json {
                return Err(Failure::Usage {
                    message: "update does not support --json".to_owned(),
                    hint: Some("run `git forest update` without `--json`".to_owned()),
                });
            }
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
                    .status()
                    .map_err(anyhow::Error::from)?;
                if !status.success() {
                    let code = status
                        .code()
                        .map_or("signal".to_string(), |code| code.to_string());
                    return Err(anyhow!(
                        "brew upgrade {} failed (exit code: {})",
                        channel::APP_NAME,
                        code
                    )
                    .into());
                }
            } else {
                println!("Download the latest release:");
                #[cfg(feature = "stable")]
                println!("  https://github.com/dliv/workforest/releases/latest");
                #[cfg(feature = "beta")]
                println!("  https://github.com/dliv/workforest/releases");
            }
            Completion::Complete
        }
    };
    Ok(completion)
}

#[expect(
    clippy::print_stdout,
    reason = "composition root: the single output dispatcher of ADR 0002"
)]
fn output<T: serde::Serialize>(result: &T, json: bool, human_fn: fn(&T) -> String) -> Result<()> {
    if json {
        print_json(result)?;
    } else {
        let text = human_fn(result);
        if !text.is_empty() {
            println!("{}", text);
        }
    }
    Ok(())
}

/// Prints one JSON document, pretty-printed, on standard output.
#[expect(
    clippy::print_stdout,
    reason = "composition root: prints every JSON document"
)]
fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
