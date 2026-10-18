//! Planning a forest's removal: the plan and result types, the pure decisions over what
//! `commands::rm` observes (the removal plan, the root cleanup, the safety rules and the
//! dry-run with its branch-deletion forecast), and the human renderers of `rm` and `rm --all`.

use anyhow::{bail, Result};
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::meta::{ForestMeta, META_FILENAME};
use crate::paths::{
    forest_root_entry_comparison_key, validate_disposable_root_entries, AbsolutePath,
    DisposableRootEntry, ForestName, RepoName,
};
use crate::plan::branch_state::{compact_git_error, ActualBranchState, WorktreeBranchState};

// --- Types ---

#[derive(Debug, Clone, Default)]
pub struct RmOptions {
    pub force: bool,
    pub dry_run: bool,
    pub require_utf8_json_paths: bool,
    pub additional_disposable_root_entries: Vec<DisposableRootEntry>,
}

impl RmOptions {
    pub fn new(force: bool, dry_run: bool) -> Self {
        Self {
            force,
            dry_run,
            require_utf8_json_paths: false,
            additional_disposable_root_entries: vec![],
        }
    }
}

pub struct RmPlan {
    pub forest_name: ForestName,
    pub forest_dir: PathBuf,
    pub repo_plans: Vec<RepoRmPlan>,
    pub(crate) root_plan: ForestRootPlan,
    /// What a dry-run observed while planning; `None` unless the plan was made for a dry-run.
    pub(crate) dry_run: Option<RmDryRunObservation>,
}

pub(crate) enum ForestRootPlan {
    Missing,
    Ready {
        cleanup_actions: Vec<ForestRootCleanupAction>,
    },
    Rejected {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForestRootCleanupAction {
    RemoveDisposableEntry {
        entry: DisposableRootEntry,
        path: PathBuf,
    },
    RemoveForceEntry(PathBuf),
}

pub struct RepoRmPlan {
    pub name: RepoName,
    pub worktree_path: PathBuf,
    pub source: AbsolutePath,
    pub branch: String,
    pub base_branch: String,
    pub remote: Option<String>,
    pub branch_created: bool,
    pub branch_state: WorktreeBranchState,
    pub detached_head_safety: DetachedHeadSafety,
    pub worktree_exists: bool,
    pub source_exists: bool,
    pub has_dirty_files: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachedHeadSafety {
    NotDetached,
    Preserved,
    Unpreserved { head: String },
    Unverified { head: String, error: String },
}

#[derive(Debug, Serialize)]
pub struct RmResult {
    pub forest_name: ForestName,
    pub forest_dir: PathBuf,
    pub dry_run: bool,
    pub force: bool,
    pub repos: Vec<RepoRmResult>,
    pub forest_root_cleanup: Vec<ForestRootCleanupResult>,
    pub forest_dir_removed: bool,
    pub errors: Vec<CleanupIssue>,
}

/// One error that a cleanup step of `rm`, `rm --all` or `reset` reported: the step's code,
/// the message, and the recovery hint when the message carries one (`null` otherwise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CleanupIssue {
    pub code: CleanupIssueCode,
    pub message: String,
    pub hint: Option<String>,
}

/// The cleanup step that failed, named for what an agent can act on. Serialized kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupIssueCode {
    /// Uncommitted changes block removal.
    DirtyWorktree,
    /// A safety check refused removal: an unverified or unreachable detached HEAD, a missing
    /// source repo, a failed branch lookup, a symlinked or locked worktree, or a dry-run that
    /// cannot prove removal.
    WorktreeRefused,
    /// Git or filesystem removal of a worktree, or of a corrupt worktree directory, failed.
    WorktreeRemoveFailed,
    /// Git worktree metadata is stale, could not be inspected, or was not pruned or verified.
    WorktreeMetadataStale,
    /// Branch deletion failed, or the branch is checked out elsewhere or could not be checked.
    BranchDeleteFailed,
    /// The forest root is a symlink, not a directory, uninspectable or ambiguous.
    ForestRootRefused,
    /// A disposable or forced forest-root entry could not be inspected or removed.
    RootEntryRemoveFailed,
    /// The forest directory stayed.
    ForestDirNotRemoved,
    /// Staging or restoring the forest metadata around removal failed.
    ForestMetadataStagingFailed,
    /// `rm --all`: one forest's removal plan was refused.
    ForestPlanRejected,
    /// `reset`: a worktree base could not be scanned safely.
    WorktreeBaseScanFailed,
    /// `reset`: the config file could not be backed up.
    ConfigBackupFailed,
    /// `reset`: the state file could not be deleted.
    StateDeleteFailed,
}

/// Where a cleanup message carries its recovery hint.
const HINT_SEPARATOR: &str = "\n  hint: ";

/// The recovery step for uncommitted changes, shared by every dirty worktree of one result.
const DIRTY_WORKTREE_HINT: &str =
    "commit or stash changes, then retry — or use `git forest rm --force`";

impl CleanupIssue {
    /// An issue from a cleanup message, split at its first `\n  hint: ` into message and hint.
    pub(crate) fn new(code: CleanupIssueCode, text: String) -> Self {
        match text.split_once(HINT_SEPARATOR) {
            Some((message, hint)) => Self {
                code,
                message: message.to_owned(),
                hint: Some(hint.to_owned()),
            },
            None => Self {
                code,
                message: text,
                hint: None,
            },
        }
    }

    /// A dirty worktree, whose hint is the shared recovery step.
    pub(crate) fn dirty_worktree(message: String) -> Self {
        Self {
            code: CleanupIssueCode::DirtyWorktree,
            message,
            hint: Some(DIRTY_WORKTREE_HINT.to_owned()),
        }
    }
}

/// The error lines human output prints for a result's issues: each message, with
/// `\n  hint: ` and its hint when it has one, except that dirty worktrees share one trailing
/// `hint: ` entry after all the issues.
pub fn legacy_error_lines(issues: &[CleanupIssue]) -> Vec<String> {
    let mut lines: Vec<String> = issues
        .iter()
        .map(|issue| match (&issue.code, &issue.hint) {
            (CleanupIssueCode::DirtyWorktree, _) | (_, None) => issue.message.clone(),
            (_, Some(hint)) => format!("{}{}{}", issue.message, HINT_SEPARATOR, hint),
        })
        .collect();
    if issues
        .iter()
        .any(|issue| issue.code == CleanupIssueCode::DirtyWorktree)
    {
        lines.push(format!("hint: {}", DIRTY_WORKTREE_HINT));
    }
    lines
}

#[derive(Debug, Serialize)]
pub struct RepoRmResult {
    pub name: RepoName,
    pub branch_state: WorktreeBranchState,
    pub worktree_removed: RmOutcome,
    pub branch_deleted: RmOutcome,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForestRootCleanupKind {
    DisposableEntry,
    Force,
}

#[derive(Debug, Serialize)]
pub struct ForestRootCleanupResult {
    pub path: PathBuf,
    pub kind: ForestRootCleanupKind,
    pub removal: RmOutcome,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum RmOutcome {
    Success,
    Skipped { reason: String },
    Failed { error: String },
}

pub enum RmProgress<'a> {
    RepoStarting { name: &'a RepoName },
    RepoDone(&'a RepoRmResult),
}

#[derive(Debug, Serialize)]
pub struct RmAllResult {
    pub dry_run: bool,
    pub force: bool,
    pub results: Vec<RmResult>,
    pub total_forests: usize,
    pub succeeded: usize,
    pub failed: usize,
}

pub enum RmAllProgress<'a> {
    ForestStarting { name: &'a ForestName },
    ForestDone(&'a ForestName, &'a RmResult),
}

impl RmPlan {
    pub(crate) fn root_rejection(&self) -> Option<CleanupIssue> {
        match &self.root_plan {
            ForestRootPlan::Rejected { error } => Some(CleanupIssue::new(
                CleanupIssueCode::ForestRootRefused,
                error.clone(),
            )),
            ForestRootPlan::Missing | ForestRootPlan::Ready { .. } => None,
        }
    }
}

pub(crate) fn validate_json_output_paths(
    forest_dir: &Path,
    root_plan: &ForestRootPlan,
    require_utf8_json_paths: bool,
) -> Result<()> {
    if !require_utf8_json_paths {
        return Ok(());
    }
    if forest_dir.to_str().is_none() {
        bail!(
            "--json refuses forest removal because the forest path is not valid UTF-8\n  hint: rename the forest directory or run without --json"
        );
    }
    if let ForestRootPlan::Ready { cleanup_actions } = root_plan {
        if cleanup_actions
            .iter()
            .any(|action| cleanup_action_path(action).to_str().is_none())
        {
            bail!(
                "--json refuses forest removal because a forest-root entry is not valid UTF-8\n  hint: rename that entry or run without --json"
            );
        }
    }
    Ok(())
}

pub(crate) fn cleanup_action_path(action: &ForestRootCleanupAction) -> &Path {
    match action {
        ForestRootCleanupAction::RemoveDisposableEntry { path, .. } => path,
        ForestRootCleanupAction::RemoveForceEntry(path) => path,
    }
}

pub(crate) fn root_entry_name_matches(name: &std::ffi::OsStr, expected: &str) -> bool {
    name == std::ffi::OsStr::new(expected)
        || name.to_str().is_some_and(|name| {
            forest_root_entry_comparison_key(name) == forest_root_entry_comparison_key(expected)
        })
}

pub(crate) fn rejected_rm_result(
    plan: &RmPlan,
    dry_run: bool,
    force: bool,
    error: String,
) -> RmResult {
    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run,
        force,
        repos: vec![],
        forest_root_cleanup: vec![],
        forest_dir_removed: false,
        errors: vec![CleanupIssue::new(
            CleanupIssueCode::ForestRootRefused,
            error,
        )],
    }
}

pub(crate) fn root_cleanup_results(
    plan: &RmPlan,
    mut outcome: impl FnMut(&ForestRootCleanupAction) -> RmOutcome,
) -> Vec<ForestRootCleanupResult> {
    let ForestRootPlan::Ready { cleanup_actions } = &plan.root_plan else {
        return vec![];
    };

    cleanup_actions
        .iter()
        .map(|action| ForestRootCleanupResult {
            path: cleanup_action_path(action).to_path_buf(),
            kind: match action {
                ForestRootCleanupAction::RemoveDisposableEntry { .. } => {
                    ForestRootCleanupKind::DisposableEntry
                }
                ForestRootCleanupAction::RemoveForceEntry(_) => ForestRootCleanupKind::Force,
            },
            removal: outcome(action),
        })
        .collect()
}

pub(crate) fn branch_not_fully_merged_error(
    repo_plan: &RepoRmPlan,
    detail: Option<&str>,
) -> String {
    let detail = detail
        .map(|detail| format!(" ({})", detail))
        .unwrap_or_default();
    format!(
        "{}: branch {:?} is not fully merged{}\n  \
         hint: if the branch was merged (e.g. squash-merge) and the remote \
         branch was deleted, use `git forest rm --force`",
        repo_plan.name, repo_plan.branch, detail,
    )
}

// --- Planning over observations ---

/// What planning a removal reads about the forest root: its kind and, for a directory, its
/// listing and each managed repo's entry. Every read error is kept as text.
pub enum RootObservation {
    /// The root does not exist.
    Missing,
    /// The root could not be inspected; the error text.
    Uninspectable(String),
    /// The root is a symlink.
    Symlink,
    /// The root is not a directory.
    NotADirectory,
    /// The root is a directory.
    Directory(RootListing),
}

/// A forest root directory's listing.
pub struct RootListing {
    /// The root's entries in listing order, each its name or its read error text; the outer
    /// error is the text of listing the root at all.
    pub entries: Result<Vec<Result<OsString, String>>, String>,
    /// One per repo of the forest, in metadata order: whether the repo's entry is present, or
    /// the error text inspecting it.
    pub repo_entries: Vec<Result<bool, String>>,
}

/// What planning a removal reads about one repo of the forest.
pub struct RepoObservation {
    /// Whether the worktree path resolves inside the forest directory.
    pub worktree_inside_forest: bool,
    pub worktree_exists: bool,
    pub branch_state: WorktreeBranchState,
    pub source_exists: bool,
    pub detached_head_safety: DetachedHeadSafety,
    pub has_dirty_files: bool,
}

/// What planning a removal reads: the root, then each repo in metadata order, and for a
/// dry-run what its result is decided from.
pub struct RmObservation {
    pub root: RootObservation,
    pub repos: Vec<RepoObservation>,
    pub dry_run: Option<RmDryRunObservation>,
}

/// Plans the removal of the forest at `forest_dir` from what `commands::rm` observed, raising
/// option and metadata errors where planning always has. A plan made for a dry-run keeps the
/// dry-run's observation.
pub fn decide_rm_plan(
    forest_dir: &Path,
    meta: &ForestMeta,
    options: &RmOptions,
    observation: RmObservation,
) -> Result<RmPlan> {
    if options.force && !options.additional_disposable_root_entries.is_empty() {
        bail!("--force cannot be combined with --discard-root-entry");
    }

    let reserved_entries: Vec<&str> = std::iter::once(META_FILENAME)
        .chain(meta.repos.iter().map(|repo| repo.name.as_str()))
        .collect();
    let mut disposable_root_entries: Vec<DisposableRootEntry> = meta
        .disposable_root_entries
        .iter()
        .chain(&options.additional_disposable_root_entries)
        .cloned()
        .collect();
    validate_disposable_root_entries(&disposable_root_entries, reserved_entries.iter().copied())?;
    disposable_root_entries.sort();

    let root_plan = decide_forest_root(
        forest_dir,
        &meta
            .repos
            .iter()
            .map(|repo| repo.name.as_str())
            .collect::<Vec<_>>(),
        &disposable_root_entries,
        options.force,
        options.require_utf8_json_paths,
        &observation.root,
    );
    validate_json_output_paths(forest_dir, &root_plan, options.require_utf8_json_paths)?;

    let repo_plans = if matches!(root_plan, ForestRootPlan::Rejected { .. }) {
        vec![]
    } else {
        meta.repos
            .iter()
            .zip(observation.repos)
            .map(|(repo, observed)| {
                let worktree_path = forest_dir.join(repo.name.as_str());
                assert!(
                    observed.worktree_inside_forest,
                    "worktree path {:?} is not inside forest dir {:?}",
                    worktree_path, forest_dir
                );
                RepoRmPlan {
                    name: repo.name.clone(),
                    worktree_path,
                    source: repo.source.clone(),
                    branch: repo.branch.clone(),
                    base_branch: repo.base_branch.clone(),
                    remote: repo.remote.clone(),
                    branch_created: repo.branch_created,
                    branch_state: observed.branch_state,
                    detached_head_safety: observed.detached_head_safety,
                    worktree_exists: observed.worktree_exists,
                    source_exists: observed.source_exists,
                    has_dirty_files: observed.has_dirty_files,
                }
            })
            .collect()
    };

    Ok(RmPlan {
        forest_name: meta.name.clone(),
        forest_dir: forest_dir.to_path_buf(),
        repo_plans,
        root_plan,
        dry_run: observation.dry_run,
    })
}

/// The forest root's removal plan: refused for a root that is not a plain directory, cannot be
/// listed, holds a non-UTF-8 entry under `--json` or ambiguous protected entries, or has an
/// uninspectable repo entry; otherwise the disposable (or, with `--force`, unmanaged) entries
/// to remove.
pub(crate) fn decide_forest_root(
    forest_dir: &Path,
    repo_names: &[&str],
    disposable_root_entries: &[DisposableRootEntry],
    force: bool,
    require_utf8_json_paths: bool,
    observation: &RootObservation,
) -> ForestRootPlan {
    let listing = match observation {
        RootObservation::Missing => return ForestRootPlan::Missing,
        RootObservation::Uninspectable(error) => {
            return ForestRootPlan::Rejected {
                error: format!(
                    "forest removal refused: failed to inspect forest root {}: {}",
                    forest_dir.display(),
                    error
                ),
            };
        }
        RootObservation::Symlink => {
            return ForestRootPlan::Rejected {
                error: format!(
                    "forest removal refused: {} is a symlink\n  hint: inspect and unlink the forest alias manually",
                    forest_dir.display()
                ),
            };
        }
        RootObservation::NotADirectory => {
            return ForestRootPlan::Rejected {
                error: format!(
                    "forest removal refused: {} is not a directory",
                    forest_dir.display()
                ),
            };
        }
        RootObservation::Directory(listing) => listing,
    };

    let entries = match &listing.entries {
        Ok(entries) => entries,
        Err(error) => {
            return ForestRootPlan::Rejected {
                error: format!(
                    "forest removal refused: failed to inspect forest root {}: {}",
                    forest_dir.display(),
                    error
                ),
            };
        }
    };
    let mut root_entry_names = Vec::new();
    for entry in entries {
        let name = match entry {
            Ok(name) => name,
            Err(error) => {
                return ForestRootPlan::Rejected {
                    error: format!(
                        "forest removal refused: failed to inspect an entry in {}: {}",
                        forest_dir.display(),
                        error
                    ),
                };
            }
        };
        root_entry_names.push(name.clone());
    }
    if require_utf8_json_paths && root_entry_names.iter().any(|name| name.to_str().is_none()) {
        return ForestRootPlan::Rejected {
            error: "--json refuses forest removal because a forest-root entry is not valid UTF-8\n  hint: rename that entry before retrying the destructive command".to_string(),
        };
    }

    let disposable_capacity = if force {
        0
    } else {
        disposable_root_entries.len()
    };
    let mut protected_names = Vec::with_capacity(1 + repo_names.len() + disposable_capacity);
    protected_names.push(META_FILENAME);
    protected_names.extend(repo_names.iter().copied());
    if !force {
        protected_names.extend(disposable_root_entries.iter().map(|entry| entry.as_str()));
    }
    for protected_name in protected_names {
        if root_entry_names
            .iter()
            .filter(|name| root_entry_name_matches(name, protected_name))
            .count()
            > 1
        {
            return ForestRootPlan::Rejected {
                error: format!(
                    "forest removal refused: found multiple root entries equivalent to {}\n  hint: inspect and preserve the intended entry before retrying",
                    protected_name
                ),
            };
        }
    }

    let mut present_repo_names = Vec::new();
    for (repo_name, entry) in repo_names.iter().zip(&listing.repo_entries) {
        match entry {
            Ok(true) => present_repo_names.push(*repo_name),
            Ok(false) => {}
            Err(error) => {
                return ForestRootPlan::Rejected {
                    error: format!(
                        "forest removal refused: failed to inspect managed repository entry {}: {}",
                        repo_name, error
                    ),
                };
            }
        }
    }

    let cleanup_actions = if force {
        let mut actions = Vec::new();
        for name in root_entry_names {
            if root_entry_name_matches(&name, META_FILENAME)
                || present_repo_names
                    .iter()
                    .any(|repo_name| root_entry_name_matches(&name, repo_name))
            {
                continue;
            }
            actions.push(ForestRootCleanupAction::RemoveForceEntry(PathBuf::from(
                name,
            )));
        }
        actions.sort_by(|left, right| cleanup_action_path(left).cmp(cleanup_action_path(right)));
        actions
    } else {
        let mut actions = Vec::new();
        for entry in disposable_root_entries {
            if let Some(actual_name) = root_entry_names
                .iter()
                .find(|name| root_entry_name_matches(name, entry.as_str()))
            {
                actions.push(ForestRootCleanupAction::RemoveDisposableEntry {
                    entry: entry.clone(),
                    path: PathBuf::from(actual_name),
                });
            }
        }
        actions
    };

    ForestRootPlan::Ready { cleanup_actions }
}

/// Why removing a repo's worktree is refused without `--force`, if it is: a detached HEAD whose
/// commits are not preserved or cannot be verified, a missing source repo, a failed branch
/// lookup, or a symlinked worktree path (`worktree_is_symlink`, observed by the caller).
pub(crate) fn worktree_removal_refusal(
    repo_plan: &RepoRmPlan,
    force: bool,
    worktree_is_symlink: bool,
) -> Option<String> {
    if force {
        return None;
    }

    match &repo_plan.detached_head_safety {
        DetachedHeadSafety::Unpreserved { head } => {
            return Some(format!(
                "{}: detached HEAD {} has commits not reachable from any branch, remote, or tag ref; use `git forest rm --force` to remove anyway",
                repo_plan.name, head
            ));
        }
        DetachedHeadSafety::Unverified { head, error } => {
            return Some(format!(
                "{}: detached HEAD {} reachability could not be verified: {}; use `git forest rm --force` to remove anyway",
                repo_plan.name,
                head,
                compact_git_error(error)
            ));
        }
        DetachedHeadSafety::NotDetached | DetachedHeadSafety::Preserved => {}
    }

    if !repo_plan.source_exists {
        return Some(format!(
            "{}: source repo missing; refusing to remove unverified worktree without `git forest rm --force`",
            repo_plan.name
        ));
    }

    if matches!(
        repo_plan.branch_state.actual,
        ActualBranchState::Unknown { .. }
    ) {
        return Some(format!(
            "{}: branch lookup failed; refusing to remove unverified worktree without `git forest rm --force`",
            repo_plan.name
        ));
    }

    if worktree_is_symlink {
        return Some(format!(
            "{}: worktree path is a symlink; refusing to remove unverified worktree without `git forest rm --force`",
            repo_plan.name
        ));
    }

    None
}

// --- Dry-run over observations ---

/// One entry of a source repo's `git worktree list`, with its canonical path when it has one.
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub canonical: Option<PathBuf>,
    pub branch: Option<String>,
    pub locked: bool,
}

/// What a dry-run reads about the branch a forest created, from its source repo.
pub struct BranchObservation {
    /// Whether `refs/heads/<branch>` exists, or the git error text.
    pub exists: Result<bool, String>,
    /// Whether `git branch -d` would succeed: the branch is merged into its upstream or HEAD.
    pub deletes_without_force: bool,
    /// Whether force-deleting is safe: merged into the base branch or its remote-tracking ref,
    /// or fully pushed to its upstream.
    pub safe_to_force_delete: bool,
}

/// What a dry-run reads about one repo.
pub struct RepoDryRunObservation {
    /// Whether the worktree path is a symlink.
    pub worktree_is_symlink: bool,
    /// The worktree path's canonical form, or its canonical parent joined with its name.
    pub worktree_canonical: Option<PathBuf>,
    /// The source repo's `git worktree list`, or its error text; `None` when the source repo
    /// is missing, where the dry-run never looks at it.
    pub worktrees: Option<Result<Vec<WorktreeEntry>, String>>,
    /// The branch facts deletion is forecast from; `None` unless the forest created the branch
    /// and the source repo exists, where the forecast stops before reading them.
    pub branch: Option<BranchObservation>,
}

/// A forest directory's listing for a dry-run.
pub enum ForestDirListing {
    /// The directory could not be listed; the error text.
    Unreadable(String),
    /// Its entries in listing order, each its name or its read error text.
    Entries(Vec<Result<OsString, String>>),
}

/// What a dry-run reads about the forest directory.
pub struct ForestDirObservation {
    pub exists: bool,
    pub is_symlink: bool,
    pub listing: ForestDirListing,
}

/// What a dry-run reads while planning, from which `decide_rm_dry_run` builds its result
/// without touching the filesystem or git.
pub struct RmDryRunObservation {
    /// Why the forest root is refused when it is read again, if it is.
    pub current_root_refusal: Option<String>,
    /// One per repo of the forest, in metadata order.
    pub repos: Vec<RepoDryRunObservation>,
    pub forest_dir: ForestDirObservation,
}

/// The dry-run result of a removal plan: what `execute_rm` would do, from the plan and what
/// the dry-run observed. Issues are appended repo by repo in plan order, as execution would
/// report them.
pub fn decide_rm_dry_run(
    plan: &RmPlan,
    observation: &RmDryRunObservation,
    force: bool,
) -> RmResult {
    let root_refusal = match &plan.root_plan {
        ForestRootPlan::Rejected { error } => Some(error.clone()),
        ForestRootPlan::Missing | ForestRootPlan::Ready { .. } => {
            observation.current_root_refusal.clone()
        }
    };
    if let Some(error) = root_refusal {
        return rejected_rm_result(plan, true, force, error);
    }
    debug_assert_eq!(
        observation.repos.len(),
        plan.repo_plans.len(),
        "a dry-run observes every repo of a plan whose root is not refused"
    );

    let has_dirty = !force && plan.repo_plans.iter().any(|rp| rp.has_dirty_files);

    let mut errors = Vec::new();
    let repos: Vec<RepoRmResult> = plan
        .repo_plans
        .iter()
        .zip(&observation.repos)
        .map(|(rp, observed)| {
            if has_dirty {
                // Dirty preflight would block all removal
                let (worktree_removed, branch_deleted) = if rp.has_dirty_files {
                    let msg = format!("{}: worktree has uncommitted changes", rp.name);
                    errors.push(CleanupIssue::dirty_worktree(msg.clone()));
                    (
                        RmOutcome::Failed { error: msg },
                        RmOutcome::Skipped {
                            reason: "worktree not removed".to_string(),
                        },
                    )
                } else {
                    (
                        RmOutcome::Skipped {
                            reason: "blocked by dirty repos".to_string(),
                        },
                        RmOutcome::Skipped {
                            reason: "blocked by dirty repos".to_string(),
                        },
                    )
                };
                return RepoRmResult {
                    name: rp.name.clone(),
                    branch_state: rp.branch_state.clone(),
                    worktree_removed,
                    branch_deleted,
                };
            }

            let worktree_removed = if observed.worktree_is_symlink {
                if force {
                    if let Some(msg) = symlink_worktree_refusal(rp, observed) {
                        errors.push(CleanupIssue::new(
                            CleanupIssueCode::WorktreeRefused,
                            msg.clone(),
                        ));
                        RmOutcome::Failed { error: msg }
                    } else {
                        RmOutcome::Success
                    }
                } else {
                    let msg = worktree_removal_refusal(rp, force, observed.worktree_is_symlink)
                        .expect("non-force symlink worktree should have a safety error");
                    errors.push(CleanupIssue::new(
                        CleanupIssueCode::WorktreeRefused,
                        msg.clone(),
                    ));
                    RmOutcome::Failed { error: msg }
                }
            } else if let Some(msg) = worktree_metadata_refusal(rp, force, observed) {
                errors.push(CleanupIssue::new(
                    CleanupIssueCode::WorktreeRefused,
                    msg.clone(),
                ));
                RmOutcome::Failed { error: msg }
            } else if !rp.worktree_exists {
                RmOutcome::Skipped {
                    reason: "worktree already missing".to_string(),
                }
            } else if let Some(msg) =
                worktree_removal_refusal(rp, force, observed.worktree_is_symlink)
            {
                errors.push(CleanupIssue::new(
                    CleanupIssueCode::WorktreeRefused,
                    msg.clone(),
                ));
                RmOutcome::Failed { error: msg }
            } else {
                RmOutcome::Success
            };

            let wt_succeeded = !matches!(&worktree_removed, RmOutcome::Failed { .. });
            let branch_deleted =
                forecast_branch_delete(rp, force, wt_succeeded, observed, &mut errors);

            RepoRmResult {
                name: rp.name.clone(),
                branch_state: rp.branch_state.clone(),
                worktree_removed,
                branch_deleted,
            }
        })
        .collect();

    let mut forest_root_cleanup = Vec::new();
    if errors.is_empty() {
        forest_root_cleanup = root_cleanup_results(plan, |_| RmOutcome::Success);
        if let Some(msg) = forest_dir_cleanup_refusal(plan, &repos, force, &observation.forest_dir)
        {
            errors.push(CleanupIssue::new(
                CleanupIssueCode::ForestDirNotRemoved,
                msg,
            ));
        }
    }

    let forest_dir_removed = errors.is_empty();

    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run: true,
        force,
        repos,
        forest_root_cleanup,
        forest_dir_removed,
        errors,
    }
}

/// Whether a listed worktree path names `target`: equal to it or, when the target has a
/// canonical form, to that form directly or through the listed path's own canonical form.
fn worktree_paths_match(
    entry: &WorktreeEntry,
    target: &Path,
    target_canonical: Option<&Path>,
) -> bool {
    if entry.path == target {
        return true;
    }

    let Some(target_canonical) = target_canonical else {
        return false;
    };

    entry.path == target_canonical || entry.canonical.as_deref() == Some(target_canonical)
}

/// Whether git's worktree metadata lists this repo's worktree, and whether it is locked; the
/// listing's error text when the source repo's worktrees could not be listed.
fn listed_worktree_lock(
    worktrees: &Result<Vec<WorktreeEntry>, String>,
    repo_plan: &RepoRmPlan,
    target_canonical: Option<&Path>,
) -> Result<Option<bool>, String> {
    let entries = worktrees.as_ref().map_err(Clone::clone)?;
    Ok(entries
        .iter()
        .find(|entry| worktree_paths_match(entry, &repo_plan.worktree_path, target_canonical))
        .map(|entry| entry.locked))
}

/// Why a dry-run cannot prove a worktree's removal from git's worktree metadata, if it cannot:
/// locked metadata, metadata that still lists a missing worktree, metadata that does not list
/// an existing one, or metadata that cannot be read. Nothing when the source repo is missing.
pub(crate) fn worktree_metadata_refusal(
    repo_plan: &RepoRmPlan,
    force: bool,
    observed: &RepoDryRunObservation,
) -> Option<String> {
    let worktrees = observed.worktrees.as_ref()?;

    match listed_worktree_lock(worktrees, repo_plan, observed.worktree_canonical.as_deref()) {
        Ok(Some(true)) => Some(format!(
            "{}: refusing removal because git worktree metadata is locked",
            repo_plan.name
        )),
        Ok(Some(false)) if !repo_plan.worktree_exists && !force => Some(format!(
            "{}: source repo still lists missing worktree metadata for {}; dry-run cannot prove branch deletion",
            repo_plan.name,
            repo_plan.worktree_path.display()
        )),
        Ok(None)
            if repo_plan.worktree_exists
                && !matches!(repo_plan.branch_state.actual, ActualBranchState::Unknown { .. }) =>
        {
            Some(format!(
                "{}: source repo does not list worktree metadata for {}; dry-run cannot prove removal",
                repo_plan.name,
                repo_plan.worktree_path.display()
            ))
        }
        Ok(_) => None,
        Err(e) => Some(format!(
            "{}: could not inspect git worktree metadata before removal: {}",
            repo_plan.name,
            compact_git_error(&e)
        )),
    }
}

/// Why a dry-run refuses a symlinked worktree under `--force`, if it does: locked or unreadable
/// worktree metadata. Nothing when the source repo is missing.
fn symlink_worktree_refusal(
    repo_plan: &RepoRmPlan,
    observed: &RepoDryRunObservation,
) -> Option<String> {
    let worktrees = observed.worktrees.as_ref()?;

    match listed_worktree_lock(worktrees, repo_plan, observed.worktree_canonical.as_deref()) {
        Ok(Some(true)) => Some(format!(
            "{}: refusing removal because git worktree metadata is locked",
            repo_plan.name
        )),
        Ok(_) => None,
        Err(e) => Some(format!(
            "{}: could not inspect git worktree metadata before removal: {}",
            repo_plan.name,
            compact_git_error(&e)
        )),
    }
}

/// Where the branch is checked out other than this repo's worktree, if anywhere, from the
/// source repo's worktree listing.
fn branch_checked_out_elsewhere(
    worktrees: &Result<Vec<WorktreeEntry>, String>,
    repo_plan: &RepoRmPlan,
    target_canonical: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let expected_branch_ref = format!("refs/heads/{}", repo_plan.branch);
    let entries = worktrees.as_ref().map_err(Clone::clone)?;
    Ok(entries
        .iter()
        .find(|entry| {
            entry.branch.as_deref() == Some(expected_branch_ref.as_str())
                && !worktree_paths_match(entry, &repo_plan.worktree_path, target_canonical)
        })
        .map(|entry| entry.path.clone()))
}

/// The dry-run forecast of deleting a repo's branch: skipped when the worktree stays, the
/// forest did not create the branch, the source repo is missing or the branch is already gone;
/// refused when the branch is checked out elsewhere or cannot be proven merged; otherwise a
/// success. Refusals are appended to `errors`.
pub(crate) fn forecast_branch_delete(
    repo_plan: &RepoRmPlan,
    force: bool,
    wt_succeeded: bool,
    observed: &RepoDryRunObservation,
    errors: &mut Vec<CleanupIssue>,
) -> RmOutcome {
    if !wt_succeeded {
        return RmOutcome::Skipped {
            reason: "worktree not removed".to_string(),
        };
    }

    if !repo_plan.branch_created {
        return RmOutcome::Skipped {
            reason: "branch not created by forest".to_string(),
        };
    }

    if !repo_plan.source_exists {
        return RmOutcome::Skipped {
            reason: "source repo missing".to_string(),
        };
    }

    let (Some(branch), Some(worktrees)) = (&observed.branch, &observed.worktrees) else {
        unreachable!(
            "a dry-run observes the branch and worktrees of every repo whose branch the forest created and whose source exists"
        );
    };

    if let Ok(false) = branch.exists {
        return RmOutcome::Skipped {
            reason: "branch already deleted".to_string(),
        };
    }

    let checkout_conflict = match branch_checked_out_elsewhere(
        worktrees,
        repo_plan,
        observed.worktree_canonical.as_deref(),
    ) {
        Ok(Some(checkout_path)) => Some(format!(
            "{}: branch {:?} is checked out at {}; dry-run cannot delete a checked-out branch",
            repo_plan.name,
            repo_plan.branch,
            checkout_path.display()
        )),
        Ok(None) => None,
        Err(e) => Some(format!(
            "{}: could not inspect worktrees before branch deletion: {}",
            repo_plan.name,
            compact_git_error(&e)
        )),
    };
    if let Some(msg) = checkout_conflict {
        errors.push(CleanupIssue::new(
            CleanupIssueCode::BranchDeleteFailed,
            msg.clone(),
        ));
        return RmOutcome::Failed { error: msg };
    }

    if force || branch.deletes_without_force || branch.safe_to_force_delete {
        return RmOutcome::Success;
    }

    let msg = branch_not_fully_merged_error(
        repo_plan,
        Some(
            "dry-run could not prove the branch is merged into HEAD, the base branch, \
             the base remote-tracking ref, or its upstream",
        ),
    );
    errors.push(CleanupIssue::new(
        CleanupIssueCode::BranchDeleteFailed,
        msg.clone(),
    ));
    RmOutcome::Failed { error: msg }
}

/// Why a dry-run would leave the forest directory in place, if it would: a symlink, an
/// unreadable listing, or entries other than the metadata, removable worktrees and disposable
/// entries. Nothing under `--force` or when the directory is gone.
fn forest_dir_cleanup_refusal(
    plan: &RmPlan,
    repos: &[RepoRmResult],
    force: bool,
    observed: &ForestDirObservation,
) -> Option<String> {
    if force || !observed.exists {
        return None;
    }

    if observed.is_symlink {
        return Some(format!(
            "forest directory not removed: {} is a symlink",
            plan.forest_dir.display()
        ));
    }

    let removable_repo_names: std::collections::BTreeSet<String> = repos
        .iter()
        .filter_map(|repo| match &repo.worktree_removed {
            RmOutcome::Success => Some(forest_root_entry_comparison_key(repo.name.as_str())),
            RmOutcome::Skipped { .. } | RmOutcome::Failed { .. } => None,
        })
        .collect();
    let disposable_entry_paths: std::collections::BTreeSet<PathBuf> = match &plan.root_plan {
        ForestRootPlan::Ready { cleanup_actions } => cleanup_actions
            .iter()
            .filter_map(|action| match action {
                ForestRootCleanupAction::RemoveDisposableEntry { path, .. } => Some(path.clone()),
                ForestRootCleanupAction::RemoveForceEntry(_) => None,
            })
            .collect(),
        ForestRootPlan::Missing | ForestRootPlan::Rejected { .. } => {
            std::collections::BTreeSet::new()
        }
    };

    let entries = match &observed.listing {
        ForestDirListing::Entries(entries) => entries,
        ForestDirListing::Unreadable(e) => {
            return Some(format!(
                "forest directory not removed (not empty): failed to inspect {}: {}",
                plan.forest_dir.display(),
                e
            ));
        }
    };

    let mut remaining = Vec::new();
    for entry in entries {
        let Ok(file_name) = entry else {
            return Some(format!(
                "forest directory not removed (not empty): failed to inspect an entry in {}",
                plan.forest_dir.display()
            ));
        };
        let comparison_key = file_name.to_str().map(forest_root_entry_comparison_key);
        if disposable_entry_paths.contains(&PathBuf::from(file_name)) {
            continue;
        }
        if root_entry_name_matches(file_name, META_FILENAME)
            || comparison_key
                .as_ref()
                .is_some_and(|key| removable_repo_names.contains(key))
        {
            continue;
        }
        remaining.push(file_name.to_string_lossy().into_owned());
    }

    if remaining.is_empty() {
        None
    } else {
        remaining.sort();
        Some(format!(
            "forest directory not removed (not empty): would still contain {}",
            remaining.join(", ")
        ))
    }
}

// --- Human output ---

fn format_branch_state_warning_suffix(branch_state: &WorktreeBranchState) -> String {
    branch_state
        .drift_message()
        .or_else(|| branch_state.lookup_error_message())
        .map(|message| format!(" [{}]", message))
        .unwrap_or_default()
}

pub fn format_rm_human(result: &RmResult) -> String {
    let mut lines = Vec::new();

    if result.dry_run {
        lines.push("Dry run — no changes will be made.".to_string());
        lines.push(String::new());
        if result.errors.is_empty() && result.forest_dir_removed {
            lines.push(format!(
                "Would remove forest {:?}",
                result.forest_name.as_str()
            ));
        } else {
            lines.push(format!(
                "Removal blocked for forest {:?}",
                result.forest_name.as_str()
            ));
        }
    } else if result.errors.is_empty() {
        lines.push(format!("Removed forest {:?}", result.forest_name.as_str()));
    } else {
        lines.push(format!(
            "Removed forest {:?} (with errors)",
            result.forest_name.as_str()
        ));
    }

    for repo in &result.repos {
        let wt = match &repo.worktree_removed {
            RmOutcome::Success => {
                if result.dry_run {
                    "remove worktree".to_string()
                } else {
                    "worktree removed".to_string()
                }
            }
            RmOutcome::Skipped { reason } => format!("worktree skipped ({})", reason),
            RmOutcome::Failed { .. } => "worktree FAILED".to_string(),
        };

        let br = match &repo.branch_deleted {
            RmOutcome::Success => {
                if result.dry_run {
                    ", delete branch".to_string()
                } else {
                    ", branch deleted".to_string()
                }
            }
            RmOutcome::Skipped { reason } => {
                if reason == "branch not created by forest" {
                    " (branch not ours)".to_string()
                } else {
                    format!(", branch skipped ({})", reason)
                }
            }
            RmOutcome::Failed { .. } => ", branch FAILED".to_string(),
        };

        lines.push(format!(
            "  {}: {}{}{}",
            repo.name,
            wt,
            br,
            format_branch_state_warning_suffix(&repo.branch_state)
        ));
    }

    lines.extend(format_forest_root_cleanup(result));

    if result.dry_run {
        if result.forest_dir_removed {
            lines.push("  Would remove forest directory".to_string());
        } else {
            lines.push("  Would not remove forest directory".to_string());
        }
    } else if result.forest_dir_removed {
        lines.push("Forest directory removed.".to_string());
    } else {
        lines.push("Forest directory not removed (not empty).".to_string());
    }

    if !result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in legacy_error_lines(&result.errors) {
            lines.push(format!("  {}", format_error_single_line(&error)));
        }
    }

    lines.join("\n")
}

pub fn format_repo_done(repo: &RepoRmResult) -> String {
    let wt = match &repo.worktree_removed {
        RmOutcome::Success => "worktree removed".to_string(),
        RmOutcome::Skipped { reason } => format!("worktree skipped ({})", reason),
        RmOutcome::Failed { .. } => "worktree FAILED".to_string(),
    };

    let br = match &repo.branch_deleted {
        RmOutcome::Success => ", branch deleted".to_string(),
        RmOutcome::Skipped { reason } => {
            if reason == "branch not created by forest" {
                " (branch not ours)".to_string()
            } else {
                format!(", branch skipped ({})", reason)
            }
        }
        RmOutcome::Failed { .. } => ", branch FAILED".to_string(),
    };

    format!(
        "{}{}{}",
        wt,
        br,
        format_branch_state_warning_suffix(&repo.branch_state)
    )
}

pub fn format_rm_summary(result: &RmResult) -> String {
    let mut lines = Vec::new();

    lines.extend(format_forest_root_cleanup(result));

    if result.forest_dir_removed {
        lines.push("Forest directory removed.".to_string());
    } else {
        lines.push("Forest directory not removed (not empty).".to_string());
    }

    if !result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in legacy_error_lines(&result.errors) {
            lines.push(format!("  {}", format_error_single_line(&error)));
        }
    }

    lines.join("\n")
}

fn format_forest_root_cleanup(result: &RmResult) -> Vec<String> {
    result
        .forest_root_cleanup
        .iter()
        .map(|cleanup| {
            let action = match (&cleanup.kind, &cleanup.removal, result.dry_run) {
                (ForestRootCleanupKind::DisposableEntry, RmOutcome::Success, true) => {
                    "Would discard forest-root entry"
                }
                (ForestRootCleanupKind::DisposableEntry, RmOutcome::Success, false) => {
                    "Discarded forest-root entry"
                }
                (ForestRootCleanupKind::Force, RmOutcome::Success, true) => {
                    "Would force-remove forest-root entry"
                }
                (ForestRootCleanupKind::Force, RmOutcome::Success, false) => {
                    "Force-removed forest-root entry"
                }
                (_, RmOutcome::Skipped { .. }, true) => "Would skip missing forest-root entry",
                (_, RmOutcome::Skipped { .. }, false) => "Skipped forest-root entry",
                (_, RmOutcome::Failed { .. }, _) => "Failed to remove forest-root entry",
            };
            format!("  {} {}", action, cleanup.path.display())
        })
        .collect()
}

fn format_error_single_line(error: &str) -> String {
    if error.contains("stderr:") {
        let compact = compact_git_error(error);
        return match compact_hint(error).filter(|hint| !compact.contains(hint)) {
            Some(hint) => format!("{}; {}", compact, hint),
            None => compact,
        };
    }

    error
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn compact_hint(error: &str) -> Option<String> {
    let hint_lines: Vec<&str> = error
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("hint:"))
        .filter(|line| !line.is_empty())
        .collect();

    (!hint_lines.is_empty()).then(|| hint_lines.join(" "))
}

pub fn format_rm_all_human(result: &RmAllResult) -> String {
    let mut lines = Vec::new();

    if result.dry_run {
        lines.push("Dry run — no changes will be made.".to_string());
        lines.push(String::new());
    }

    for r in &result.results {
        lines.push(format_rm_human(r));
        lines.push(String::new());
    }

    if result.dry_run {
        lines.push(format!(
            "Would remove {}/{} forest(s).",
            result.succeeded, result.total_forests
        ));
    } else {
        lines.push(format!(
            "Removed {}/{} forest(s).",
            result.succeeded, result.total_forests
        ));
    }

    lines.join("\n")
}

pub fn format_rm_all_summary(result: &RmAllResult) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Removed {}/{} forest(s).",
        result.succeeded, result.total_forests
    ));

    let all_errors: Vec<String> = result
        .results
        .iter()
        .flat_map(|r| legacy_error_lines(&r.errors))
        .collect();
    if !all_errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in &all_errors {
            lines.push(format!("  {}", format_error_single_line(error)));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean repo plan for `/forests/feat/<name>`, its branch created by the forest.
    fn repo_plan(name: &str) -> RepoRmPlan {
        RepoRmPlan {
            name: RepoName::new(name.to_string()).unwrap(),
            worktree_path: PathBuf::from(format!("/forests/feat/{name}")),
            source: AbsolutePath::new(PathBuf::from(format!("/src/{name}"))).unwrap(),
            branch: "testuser/feat".to_string(),
            base_branch: "main".to_string(),
            remote: Some("origin".to_string()),
            branch_created: true,
            branch_state: WorktreeBranchState::new(
                "testuser/feat".to_string(),
                ActualBranchState::Branch {
                    actual_branch: "testuser/feat".to_string(),
                },
            ),
            detached_head_safety: DetachedHeadSafety::NotDetached,
            worktree_exists: true,
            source_exists: true,
            has_dirty_files: false,
        }
    }

    fn plan(repo_plans: Vec<RepoRmPlan>) -> RmPlan {
        RmPlan {
            forest_name: ForestName::new("feat".to_string()).unwrap(),
            forest_dir: PathBuf::from("/forests/feat"),
            repo_plans,
            root_plan: ForestRootPlan::Ready {
                cleanup_actions: vec![],
            },
            dry_run: None,
        }
    }

    /// The source repo lists this repo's worktree, unlocked, and its branch can be deleted.
    fn observed(repo_plan: &RepoRmPlan) -> RepoDryRunObservation {
        RepoDryRunObservation {
            worktree_is_symlink: false,
            worktree_canonical: Some(repo_plan.worktree_path.clone()),
            worktrees: Some(Ok(vec![WorktreeEntry {
                path: repo_plan.worktree_path.clone(),
                canonical: Some(repo_plan.worktree_path.clone()),
                branch: Some(format!("refs/heads/{}", repo_plan.branch)),
                locked: false,
            }])),
            branch: Some(BranchObservation {
                exists: Ok(true),
                deletes_without_force: true,
                safe_to_force_delete: false,
            }),
        }
    }

    /// The forest directory holds its metadata and the repos' worktrees, plus `extra`.
    fn observation(
        plan: &RmPlan,
        repos: Vec<RepoDryRunObservation>,
        extra: &[&str],
    ) -> RmDryRunObservation {
        let entries = std::iter::once(META_FILENAME.to_string())
            .chain(plan.repo_plans.iter().map(|rp| rp.name.to_string()))
            .chain(extra.iter().map(|name| name.to_string()))
            .map(|name| Ok(OsString::from(name)))
            .collect();
        RmDryRunObservation {
            current_root_refusal: None,
            repos,
            forest_dir: ForestDirObservation {
                exists: true,
                is_symlink: false,
                listing: ForestDirListing::Entries(entries),
            },
        }
    }

    fn dry_run(plan: &RmPlan, repos: Vec<RepoDryRunObservation>) -> RmResult {
        decide_rm_dry_run(plan, &observation(plan, repos, &[]), false)
    }

    #[test]
    fn decide_rm_blocks_every_repo_when_one_is_dirty() {
        let mut dirty = repo_plan("foo-api");
        dirty.has_dirty_files = true;
        let clean = repo_plan("foo-web");
        let repos = vec![observed(&dirty), observed(&clean)];
        let plan = plan(vec![dirty, clean]);

        let result = dry_run(&plan, repos);

        assert_eq!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed {
                error: "foo-api: worktree has uncommitted changes".to_string()
            }
        );
        let blocked = RmOutcome::Skipped {
            reason: "blocked by dirty repos".to_string(),
        };
        assert_eq!(result.repos[1].worktree_removed, blocked);
        assert_eq!(result.repos[1].branch_deleted, blocked);
        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![
                "foo-api: worktree has uncommitted changes".to_string(),
                "hint: commit or stash changes, then retry — or use `git forest rm --force`"
                    .to_string(),
            ]
        );
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn decide_rm_refuses_unverified_or_symlinked_worktrees_without_force() {
        let mut detached = repo_plan("foo-api");
        detached.detached_head_safety = DetachedHeadSafety::Unpreserved {
            head: "abc123".to_string(),
        };
        let symlinked = repo_plan("foo-web");
        let mut symlinked_observed = observed(&symlinked);
        symlinked_observed.worktree_is_symlink = true;
        let repos = vec![observed(&detached), symlinked_observed];
        let plan = plan(vec![detached, symlinked]);

        let result = dry_run(&plan, repos);

        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![
                "foo-api: detached HEAD abc123 has commits not reachable from any branch, remote, or tag ref; use `git forest rm --force` to remove anyway".to_string(),
                "foo-web: worktree path is a symlink; refusing to remove unverified worktree without `git forest rm --force`".to_string(),
            ]
        );
        assert!(result
            .errors
            .iter()
            .all(|issue| issue.code == CleanupIssueCode::WorktreeRefused));
    }

    #[test]
    fn decide_rm_forecasts_branch_deletion_from_observed_facts() {
        let merged = repo_plan("foo-api");
        let unmerged = repo_plan("foo-web");
        let mut unmerged_observed = observed(&unmerged);
        unmerged_observed.branch = Some(BranchObservation {
            exists: Ok(true),
            deletes_without_force: false,
            safe_to_force_delete: false,
        });
        let repos = vec![observed(&merged), unmerged_observed];
        let plan = plan(vec![merged, unmerged]);

        let result = dry_run(&plan, repos);

        assert_eq!(result.repos[0].branch_deleted, RmOutcome::Success);
        assert!(matches!(
            &result.repos[1].branch_deleted,
            RmOutcome::Failed { error } if error.contains("dry-run could not prove the branch is merged")
        ));
        assert_eq!(
            legacy_error_lines(&result.errors),
            vec!["foo-web: branch \"testuser/feat\" is not fully merged (dry-run could not prove the branch is merged into HEAD, the base branch, the base remote-tracking ref, or its upstream)\n  hint: if the branch was merged (e.g. squash-merge) and the remote branch was deleted, use `git forest rm --force`".to_string()]
        );
    }

    #[test]
    fn decide_rm_reports_locked_or_stale_worktree_metadata() {
        let locked = repo_plan("foo-api");
        let mut locked_observed = observed(&locked);
        if let Some(Ok(entries)) = &mut locked_observed.worktrees {
            entries[0].locked = true;
        }
        let mut stale = repo_plan("foo-web");
        stale.worktree_exists = false;
        let repos = vec![locked_observed, observed(&stale)];
        let plan = plan(vec![locked, stale]);

        let result = dry_run(&plan, repos);

        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![
                "foo-api: refusing removal because git worktree metadata is locked".to_string(),
                "foo-web: source repo still lists missing worktree metadata for /forests/feat/foo-web; dry-run cannot prove branch deletion".to_string(),
            ]
        );
    }

    #[test]
    fn decide_rm_keeps_a_forest_dir_with_unexpected_entries() {
        let repo = repo_plan("foo-api");
        let repos = vec![observed(&repo)];
        let plan = plan(vec![repo]);

        let result = decide_rm_dry_run(&plan, &observation(&plan, repos, &["notes.txt"]), false);

        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![
                "forest directory not removed (not empty): would still contain notes.txt"
                    .to_string()
            ]
        );
        assert_eq!(result.repos[0].worktree_removed, RmOutcome::Success);
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn decide_rm_keeps_todays_issue_order_across_repos() {
        let locked = repo_plan("foo-api");
        let mut locked_observed = observed(&locked);
        if let Some(Ok(entries)) = &mut locked_observed.worktrees {
            entries[0].locked = true;
        }
        let symlinked = repo_plan("foo-web");
        let mut symlinked_observed = observed(&symlinked);
        symlinked_observed.worktree_is_symlink = true;
        let repos = vec![locked_observed, symlinked_observed];
        let plan = plan(vec![locked, symlinked]);

        let result = dry_run(&plan, repos);

        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![
                "foo-api: refusing removal because git worktree metadata is locked".to_string(),
                "foo-web: worktree path is a symlink; refusing to remove unverified worktree without `git forest rm --force`".to_string(),
            ]
        );
    }
}
