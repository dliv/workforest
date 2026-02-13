use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "git-forest", about = "Multi-repo worktree orchestrator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize git-forest configuration
    Init,
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
