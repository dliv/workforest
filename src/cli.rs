use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "git-forest", about = "Multi-repo worktree orchestrator")]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize git-forest configuration
    Init {
        /// Base directory for worktrees
        #[arg(long, default_value = "~/worktrees")]
        worktree_base: String,
        /// Default base branch for repos
        #[arg(long, default_value = "dev")]
        base_branch: String,
        /// Branch naming template (must contain {name})
        #[arg(long, default_value = "{user}/{name}")]
        branch_template: String,
        /// Your username for branch templates
        #[arg(long)]
        username: Option<String>,
        /// Git repo paths to manage (repeatable)
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Overwrite existing config file
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
    },
    /// Remove a forest
    Rm {
        /// Forest name (or auto-detect from cwd)
        name: Option<String>,
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
}
