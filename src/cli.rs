use crate::meta::ForestMode;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "git-forest",
    version = env!("CARGO_PKG_VERSION"),
    about = "Multi-repo worktree orchestrator",
    after_help = "For AI agent usage instructions: git forest agent-instructions"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize git-forest configuration
    Init {
        /// Template name to create or update
        #[arg(long, default_value = "default")]
        template: String,
        /// Base directory for worktrees
        #[arg(long, default_value = "~/worktrees")]
        worktree_base: String,
        /// Default base branch for repos
        #[arg(long, default_value = "dev")]
        base_branch: String,
        /// Feature branch naming template (must contain {name}, e.g. "yourname/{name}")
        #[arg(long)]
        feature_branch_template: Option<String>,
        /// Git repo paths to manage (repeatable)
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Per-repo base branch override (format: repo-name=branch, repeatable)
        #[arg(long = "repo-base-branch")]
        repo_base_branches: Vec<String>,
        /// Overwrite existing template by the same name
        #[arg(long)]
        force: bool,
        /// Print config path and exit
        #[arg(long)]
        show_path: bool,
    },
    /// Create a new forest
    New {
        /// Forest name (e.g., "java-84/refactor-auth")
        name: String,
        /// Mode: feature or review
        #[arg(long)]
        mode: ForestMode,
        /// Template to use (default: from config's default_template)
        #[arg(long)]
        template: Option<String>,
        /// Override default branch for all repos
        #[arg(long)]
        branch: Option<String>,
        /// Per-repo branch override (format: repo-name=branch, repeatable)
        #[arg(long = "repo-branch")]
        repo_branches: Vec<String>,
        /// Skip fetching remotes before creating
        #[arg(long)]
        no_fetch: bool,
        /// Show plan without executing
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a forest
    Rm {
        /// Forest name (or auto-detect from cwd)
        name: Option<String>,
        /// Force removal of dirty worktrees and unmerged branches
        #[arg(long)]
        force: bool,
        /// Show what would be removed without executing
        #[arg(long)]
        dry_run: bool,
    },
    /// List all forests
    Ls,
    /// Show status of repos in a forest
    Status {
        /// Forest name (or auto-detect from cwd)
        name: Option<String>,
    },
    /// Run a command in each repo of a forest
    Exec {
        /// Forest name
        name: String,
        /// Command and arguments to run
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Print AI agent usage instructions
    AgentInstructions,
    /// Show version information
    Version {
        /// Check for updates (network call)
        #[arg(long)]
        check: bool,
    },
    /// Update git-forest to the latest version
    Update,
}
