use clap::{Parser, Subcommand};
use git_forest_core::meta::ForestMode;
use git_forest_core::paths::DisposableRootEntry;

#[derive(Parser)]
#[cfg_attr(
    feature = "stable",
    command(
        name = "git-forest",
        version = env!("CARGO_PKG_VERSION"),
        about = "Multi-repo worktree orchestrator",
        after_help = "For AI agent usage instructions: git forest agent-instructions"
    )
)]
#[cfg_attr(
    feature = "beta",
    command(
        name = "git-forest-beta",
        version = env!("CARGO_PKG_VERSION"),
        about = "Multi-repo worktree orchestrator (beta)",
        after_help = "For AI agent usage instructions: git forest-beta agent-instructions"
    )
)]
pub struct Cli {
    /// Emit JSON for forest operations (see each command's help for exceptions)
    #[arg(long, global = true)]
    pub json: bool,

    /// Print version-check diagnostics to stderr
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize git-forest configuration
    ///
    /// Create or update a named template of source repositories and defaults. The first
    /// template becomes the default. Detect each repository's base branch before choosing
    /// --base-branch; the default value is not a remote-branch discovery mechanism.
    ///
    /// This writes configuration and has no dry-run. --force replaces an existing template.
    /// --show-path only prints the configuration path as text, even with --json; it does
    /// not check whether the file exists or its configuration is valid.
    ///
    /// Disposable root entries are exact top-level names, not paths or globs. They authorize
    /// recursive deletion during ordinary removal and are saved in each new forest.
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
        /// Forest-root entry that ordinary removal may discard (repeatable)
        #[arg(long = "disposable-root-entry")]
        disposable_root_entries: Vec<DisposableRootEntry>,
        /// Overwrite existing template by the same name
        #[arg(long)]
        force: bool,
        /// Print config path and exit
        #[arg(long)]
        show_path: bool,
    },
    /// Create a new forest
    ///
    /// Create one worktree per configured repository. Feature mode uses the configured
    /// branch-name template; review mode uses forest/<name>. --repo-branch selects an
    /// existing branch for an individual repository; other repositories use their base.
    ///
    /// Preview with --dry-run --json. Both preview and creation fetch by default, which
    /// updates local remote-tracking refs; add --no-fetch to use only existing local refs.
    /// A preview does not reserve branches or guarantee a later run will succeed.
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
        /// Show the creation plan (still fetches unless --no-fetch)
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a forest
    ///
    /// Preview with --dry-run --json and inspect operation results and errors. Preflight
    /// checks all repositories before changing that forest. Known locks, dirty content,
    /// unsafe branch deletion, unpreserved detached commits, and failed required observations
    /// block the whole forest. Dirtiness includes config-hidden untracked/submodule changes.
    /// Submodule checkouts or retained submodule metadata require --force.
    ///
    /// --force allows discarding dirty content, submodule state, unmerged branches and root
    /// residue. It never unlocks worktrees or deletes branches checked out elsewhere.
    /// Lock refusals include the reason when available; reasons are explanatory text.
    ///
    /// --all completes discovery before mutation, then handles each forest independently:
    /// a blocked forest stays intact while healthy forests can be removed. Unmanaged root
    /// files can retain the directory after successful repo cleanup. Use exact
    /// --discard-root-entry names only when their entire contents may be deleted.
    ///
    /// Exit 1 means incomplete removal or a preview predicting failure; inspect JSON errors.
    /// Later races and I/O failures can still leave partial cleanup. There is no rollback.
    Rm {
        /// Forest name (or auto-detect from cwd)
        name: Option<String>,
        /// Remove all forests (preserves config)
        #[arg(long, conflicts_with = "name")]
        all: bool,
        /// Discard dirty/submodule content, unmerged branches and root residue; locks still block
        #[arg(long)]
        force: bool,
        /// Exact forest-root entry to discard for this removal (repeatable)
        #[arg(long = "discard-root-entry", conflicts_with = "force")]
        discard_root_entries: Vec<DisposableRootEntry>,
        /// Show what would be removed without executing
        #[arg(long)]
        dry_run: bool,
    },
    /// List all forests
    ///
    /// Inventory configured worktree bases without fetching. --json returns forests and
    /// findings. Each forest includes locked_worktree_count, locked_worktrees (repo names
    /// and optional reasons), lock_unavailable_count and lock_unavailable_repos.
    /// An unavailable observation must not be interpreted as an unlocked worktree.
    ///
    /// Inspect findings even on exit 0: missing-metadata and unreadable-metadata describe
    /// directories that need attention, without granting cleanup authority. A failure to
    /// enumerate a configured worktree base fails the command with exit 1.
    Ls,
    /// Show status of repos in a forest
    ///
    /// Use --json for automation. Inspect every repos[] entry: lock_state.type is locked,
    /// unlocked or unavailable, and status.type is Ok, Missing or Error. A successful
    /// command can still contain missing repos or unavailable observations.
    ///
    /// For an Ok status, status.output retains Git's text and status.summary adds
    /// changed_path_count, head_commit, upstream, base and commits_not_on_known_remotes.
    /// Check summary.type and each comparison/count type before reading values; unavailable
    /// is not zero. Upstream may also be not_configured or detached. Lock reasons are text,
    /// not commands or cleanup permission.
    ///
    /// Changed paths count once across staged/unstaged changes and renames; untracked files
    /// count individually and a dirty submodule counts once. Ignored files do not count.
    /// Upstream is the current branch's configured target; base is the saved creation ref.
    /// The known-remotes count excludes commits reachable from any local remote-tracking ref,
    /// so it can be zero while ahead of upstream. No known remote refs means unavailable.
    ///
    /// Status does not fetch. Stale refs and shallow history affect comparisons, and the
    /// observations are not an atomic snapshot. Counts do not authorize deletion or prove
    /// fast-forward readiness. Run agent-instructions for the complete usage guide.
    Status {
        /// Forest name (or auto-detect from cwd)
        name: Option<String>,
    },
    /// Run a command in each repo of a forest
    ///
    /// A forest name is required. Put the command and its arguments after --, for example:
    /// exec my-feature -- git status --short
    ///
    /// The supplied command may mutate every repository. There is no exec dry-run or rollback.
    /// With --json, child stdout is redirected to stderr so stdout contains the result document.
    /// Inspect failures; exit 1 means at least one repo could not run or its command failed.
    Exec {
        /// Forest name
        name: String,
        /// Command and arguments to run
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Print AI agent usage instructions
    ///
    /// Print the usage guide embedded in this installed version, including JSON fields,
    /// removal safety and workflow examples. Output is Markdown text even with --json.
    /// This command requires no configured forest and does not check for updates.
    AgentInstructions,
    /// Show version information
    ///
    /// Print the installed version as text, even with --json. --check explicitly contacts
    /// the update service unless version checks are disabled in configuration; update
    /// notices and check failures are written to stderr. This does not install an update.
    Version {
        /// Check for updates (network call)
        #[arg(long)]
        check: bool,
    },
    /// Remove all git-forest configuration, state, and forest directories
    ///
    /// Preview with --dry-run --json. Actual deletion requires --confirm. --config-only
    /// keeps forest directories and worktrees while removing configuration and state.
    ///
    /// Full reset has its own cleanup policy and can partially clean a forest; it does not
    /// provide rm's whole-forest preflight guarantee. Inspect every JSON error and operation
    /// result. Incomplete cleanup preserves configuration/state and exits 1. Use rm for
    /// routine forest cleanup when retaining configuration is intended.
    Reset {
        /// Confirm deletion (required unless --dry-run)
        #[arg(long)]
        confirm: bool,
        /// Only delete config and state files, leave worktree directories
        #[arg(long)]
        config_only: bool,
        /// Show what would be deleted without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Update git-forest to the latest version
    ///
    /// Upgrade the installed channel through Homebrew when available; otherwise print
    /// installation guidance. This can use the network and modify the installed program.
    /// Homebrew output is textual: --json is rejected with exit 2 before the update runs.
    Update,
}
