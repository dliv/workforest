//! Planning a reset: the plan and result types, the pure decisions over what the forest ops
//! crate's (`git-forest-ops`) `commands::reset` observes (which forests a reset would remove,
//! through `rm`'s decisions, and what becomes of the config and state files), and the human
//! renderers.

use anyhow::{bail, Result};
use serde::Serialize;
use std::path::PathBuf;

use crate::meta::ForestMeta;
use crate::paths::RepoName;
use crate::plan::branch_state::ActualBranchState;
use crate::plan::rm::{
    decide_rm_plan, forecast_branch_delete, legacy_error_lines, worktree_metadata_refusal,
    CleanupIssue, CleanupIssueCode, RepoDryRunObservation, RepoRmPlan, RmDryRunObservation,
    RmObservation, RmOptions, RmOutcome,
};

// --- Types ---

#[derive(Debug, Serialize)]
pub struct ResetResult {
    pub dry_run: bool,
    pub confirm_required: bool,
    pub config_only: bool,
    pub config_file: FileResetEntry,
    pub state_file: FileResetEntry,
    pub forests: Vec<ForestResetEntry>,
    pub warnings: Vec<String>,
    pub errors: Vec<CleanupIssue>,
}

#[derive(Debug, Serialize)]
pub struct FileResetEntry {
    pub path: PathBuf,
    pub existed: bool,
    pub deleted: bool,
    /// If the file was backed up instead of deleted, this is the backup path.
    pub backed_up_to: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct ForestResetEntry {
    pub name: String,
    pub path: PathBuf,
    pub removed: bool,
    #[serde(default)]
    pub repos: Vec<RepoResetEntry>,
    /// Whether the forest path still exists: observed while planning for a preview, and read
    /// once the removal steps have run for a real run. The renderers use it; JSON leaves it out.
    #[serde(skip)]
    pub path_exists: bool,
}

#[derive(Debug, Serialize)]
pub struct RepoResetEntry {
    pub name: RepoName,
    pub branch: String,
    pub branch_created: bool,
    pub worktree_removed: RmOutcome,
    pub branch_deleted: RmOutcome,
}

// --- Planning over observations ---

/// What planning a reset reads. Every read error is kept for `decide_reset_plan`.
pub struct ResetObservation {
    /// The config file's path, or the error resolving it.
    pub config_path: Result<PathBuf>,
    /// The state file's path, or the error resolving it.
    pub state_path: Result<PathBuf>,
    /// What is read at those paths; `None` unless both resolved.
    pub files: Option<ResetFilesObservation>,
}

/// What a reset reads at the config and state paths.
pub struct ResetFilesObservation {
    pub config_exists: bool,
    pub state_exists: bool,
    /// The config's forests; `None` with `--config-only` or without a config file, where a reset
    /// never loads the config.
    pub config: Option<ConfigObservation>,
}

/// What a reset reads from its config file.
pub enum ConfigObservation {
    /// The config could not be loaded; the error text.
    Unparsable(String),
    /// The config's worktree bases were scanned for forests.
    Scanned(DiscoveryObservation),
}

/// The forests under the config's worktree bases.
pub struct DiscoveryObservation {
    /// Each base that could not be scanned, in config order.
    pub base_failures: Vec<BaseScanFailure>,
    /// The forests found, deduplicated, in discovery order.
    pub forests: Vec<ForestObservation>,
}

/// A worktree base that could not be scanned for forests.
pub struct BaseScanFailure {
    pub base: PathBuf,
    /// The error text.
    pub error: String,
}

/// One forest a reset would remove: its path and metadata, whether the path exists, and what
/// planning its removal read. A preview's dry-run facts are in the removal observation's
/// `dry_run`; a confirmed reset reads none.
pub struct ForestObservation {
    pub path: PathBuf,
    pub meta: ForestMeta,
    pub path_exists: bool,
    pub rm: RmObservation,
}

/// One forest of a reset plan.
pub struct ForestInfo {
    pub name: String,
    pub path: PathBuf,
    pub repos: Vec<RepoRmPlan>,
    pub root_rejection: Option<CleanupIssue>,
    /// What a dry-run of the forest's removal read while planning; present only when the run
    /// previews.
    pub dry_run: Option<RmDryRunObservation>,
    /// Whether the forest path existed while planning.
    pub path_exists: bool,
}

pub struct ResetPlan {
    pub config_path: PathBuf,
    pub config_exists: bool,
    pub state_path: PathBuf,
    pub state_exists: bool,
    pub config_only: bool,
    pub forests: Vec<ForestInfo>,
    pub warnings: Vec<String>,
    pub discovery_errors: Vec<CleanupIssue>,
}

/// Plans a reset from what the forest ops crate's (`git-forest-ops`) `commands::reset`
/// observed: the path errors first, then the config warning or the scan failures in config
/// order, then each forest's removal plan in discovery order, refusing when there is nothing to
/// reset, as planning always has.
pub fn decide_reset_plan(config_only: bool, observation: ResetObservation) -> Result<ResetPlan> {
    let config_path = observation.config_path?;
    let state_path = observation.state_path?;
    let files = observation
        .files
        .expect("a reset reads its files once both paths resolve");

    let mut forests = Vec::new();
    let mut warnings = Vec::new();
    let mut discovery_errors = Vec::new();

    match files.config {
        None => {}
        Some(ConfigObservation::Unparsable(error)) => {
            warnings.push(format!(
                "config exists but could not be parsed: {}\n  worktree directories may need manual cleanup",
                error
            ));
        }
        Some(ConfigObservation::Scanned(discovery)) => {
            for failure in discovery.base_failures {
                discovery_errors.push(CleanupIssue::new(CleanupIssueCode::WorktreeBaseScanFailed, format!(
                    "could not safely scan worktree base {}: {}\n  hint: config and state will be preserved until forest recovery is complete",
                    failure.base.display(),
                    failure.error
                )));
            }
            for forest in discovery.forests {
                let name = forest.meta.name.to_string();
                let rm_plan =
                    decide_rm_plan(&forest.path, &forest.meta, &RmOptions::default(), forest.rm)
                        .expect("validated forest metadata should produce a removal plan");
                forests.push(ForestInfo {
                    name,
                    path: forest.path,
                    root_rejection: rm_plan.root_rejection(),
                    repos: rm_plan.repo_plans,
                    dry_run: rm_plan.dry_run,
                    path_exists: forest.path_exists,
                });
            }
        }
    }

    if !files.config_exists && !files.state_exists && forests.is_empty() {
        bail!("nothing to reset\n  hint: no config, state, or forests found");
    }

    Ok(ResetPlan {
        config_path,
        config_exists: files.config_exists,
        state_path,
        state_exists: files.state_exists,
        config_only,
        forests,
        warnings,
        discovery_errors,
    })
}

/// The previewed outcome of removing one repo's worktree, from what a dry-run observed of it.
/// Refusals are appended to `errors`.
fn plan_reset_worktree_outcome(
    repo: &RepoRmPlan,
    observed: &RepoDryRunObservation,
    errors: &mut Vec<CleanupIssue>,
) -> (RmOutcome, bool) {
    if !repo.worktree_exists {
        if repo.source_exists {
            if let Some(msg) = worktree_metadata_refusal(repo, true, observed) {
                errors.push(CleanupIssue::new(
                    CleanupIssueCode::WorktreeRefused,
                    msg.clone(),
                ));
                return (RmOutcome::Failed { error: msg }, false);
            }
        }
        return (
            RmOutcome::Skipped {
                reason: "worktree already missing".to_string(),
            },
            true,
        );
    }

    if !repo.source_exists {
        let msg = format!(
            "{}: source repo missing; dry-run cannot prove worktree removal",
            repo.name
        );
        errors.push(CleanupIssue::new(
            CleanupIssueCode::WorktreeRefused,
            msg.clone(),
        ));
        return (RmOutcome::Failed { error: msg }, false);
    }

    if let Some(msg) = worktree_metadata_refusal(repo, true, observed) {
        errors.push(CleanupIssue::new(
            CleanupIssueCode::WorktreeRefused,
            msg.clone(),
        ));
        return (RmOutcome::Failed { error: msg }, false);
    }

    if matches!(repo.branch_state.actual, ActualBranchState::Unknown { .. }) {
        let msg = format!(
            "{}: branch lookup failed; dry-run cannot prove worktree removal",
            repo.name
        );
        errors.push(CleanupIssue::new(
            CleanupIssueCode::WorktreeRefused,
            msg.clone(),
        ));
        return (RmOutcome::Failed { error: msg }, false);
    }

    (RmOutcome::Success, true)
}

pub fn repo_reset_entry(
    repo: &RepoRmPlan,
    worktree_removed: RmOutcome,
    branch_deleted: RmOutcome,
) -> RepoResetEntry {
    RepoResetEntry {
        name: repo.name.clone(),
        branch: repo.branch.clone(),
        branch_created: repo.branch_created,
        worktree_removed,
        branch_deleted,
    }
}

// --- Preview ---

/// The preview of a reset: each forest's removal decided from what planning observed, through
/// `rm`'s dry-run decisions, and the config and state files kept unless every forest is clean.
/// Only previews reach it, and a preview's plan carries each forest's dry-run observation.
pub fn plan_to_dry_run(plan: &ResetPlan) -> ResetResult {
    let mut errors = plan.discovery_errors.clone();
    let forests: Vec<ForestResetEntry> = plan
        .forests
        .iter()
        .map(|f| {
            let dry_run = f
                .dry_run
                .as_ref()
                .expect("a plan made for a preview carries each forest's dry-run observation");
            let forest_error_start = errors.len();
            let repos = if let Some(error) = f
                .root_rejection
                .clone()
                .or_else(|| dry_run.current_root_refusal.clone().map(root_refused))
            {
                errors.push(error);
                vec![]
            } else {
                debug_assert_eq!(
                    f.repos.len(),
                    dry_run.repos.len(),
                    "a reset observes every repo of a forest whose root is not refused"
                );
                f.repos
                    .iter()
                    .zip(&dry_run.repos)
                    .map(|(repo, observed)| {
                        let (worktree_removed, wt_succeeded) =
                            plan_reset_worktree_outcome(repo, observed, &mut errors);
                        let branch_deleted = forecast_branch_delete(
                            repo,
                            false,
                            wt_succeeded,
                            observed,
                            &mut errors,
                        );
                        repo_reset_entry(repo, worktree_removed, branch_deleted)
                    })
                    .collect()
            };
            ForestResetEntry {
                name: f.name.clone(),
                path: f.path.clone(),
                removed: f.path_exists && errors.len() == forest_error_start,
                repos,
                path_exists: f.path_exists,
            }
        })
        .collect();
    let cleanup_succeeded = errors.is_empty();

    ResetResult {
        dry_run: true,
        confirm_required: false,
        config_only: plan.config_only,
        config_file: FileResetEntry {
            path: plan.config_path.clone(),
            existed: plan.config_exists,
            deleted: plan.config_exists && cleanup_succeeded,
            // Signal to formatter that this would be backed up, not deleted
            backed_up_to: if plan.config_exists && cleanup_succeeded {
                Some(
                    plan.config_path
                        .with_file_name("config.toml.<timestamp>.bak"),
                )
            } else {
                None
            },
        },
        state_file: FileResetEntry {
            path: plan.state_path.clone(),
            existed: plan.state_exists,
            deleted: plan.state_exists && cleanup_succeeded,
            backed_up_to: None,
        },
        forests,
        warnings: plan.warnings.clone(),
        errors,
    }
}

pub fn plan_to_confirm_required(plan: &ResetPlan) -> ResetResult {
    let mut result = plan_to_dry_run(plan);
    result.dry_run = false;
    result.confirm_required = result.errors.is_empty();
    result
}

// --- Human formatting ---

pub fn format_reset_human(result: &ResetResult) -> String {
    let mut lines = Vec::new();

    let cleanup_blocked = reset_cleanup_blocked(result);

    if result.confirm_required {
        lines.push("The following would be deleted:".to_string());
    } else if result.dry_run {
        lines.push("Dry run — no changes will be made.".to_string());
    } else if cleanup_blocked {
        lines.push("Reset blocked by errors.".to_string());
    } else if result.errors.is_empty() {
        lines.push("Reset complete.".to_string());
    } else {
        lines.push("Reset completed with errors.".to_string());
    }

    let is_preview = result.dry_run || result.confirm_required || cleanup_blocked;

    if !result.config_only {
        if result.forests.is_empty() {
            lines.push(String::new());
            lines.push("Forests: none found".to_string());
        } else {
            lines.push(String::new());
            lines.push("Forests:".to_string());
            for forest in &result.forests {
                let status = if is_preview {
                    if forest.removed {
                        "would remove"
                    } else if forest.path_exists {
                        "blocked"
                    } else {
                        "already missing"
                    }
                } else if forest.removed {
                    "removed"
                } else {
                    "failed"
                };
                lines.push(format!(
                    "  {}: {} ({})",
                    forest.name,
                    status,
                    forest.path.display()
                ));
                for repo in &forest.repos {
                    lines.push(format!(
                        "    {}",
                        format_repo_reset_status(repo, is_preview)
                    ));
                }
            }
        }
    }

    lines.push(String::new());

    let config_status = format_file_status(&result.config_file, is_preview, cleanup_blocked);
    lines.push(format!(
        "Config: {} ({})",
        config_status,
        result.config_file.path.display()
    ));

    let state_status = format_file_status(&result.state_file, is_preview, cleanup_blocked);
    lines.push(format!(
        "State:  {} ({})",
        state_status,
        result.state_file.path.display()
    ));

    if !result.warnings.is_empty() {
        lines.push(String::new());
        lines.push("Warnings:".to_string());
        for warning in &result.warnings {
            lines.push(format!("  {}", warning));
        }
    }

    if !result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in legacy_error_lines(&result.errors) {
            lines.push(format!("  {}", error));
        }
    }

    if result.confirm_required && result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Pass --confirm to proceed.".to_string());
    }

    lines.join("\n")
}

pub fn format_reset_summary(result: &ResetResult) -> String {
    let mut lines = Vec::new();

    let cleanup_blocked = reset_cleanup_blocked(result);

    if cleanup_blocked {
        lines.push("Reset blocked by errors.".to_string());
    } else if result.errors.is_empty() {
        lines.push("Reset complete.".to_string());
    } else {
        lines.push("Reset completed with errors.".to_string());
    }

    let is_preview = false;

    if !result.forests.is_empty() {
        let repo_count = result
            .forests
            .iter()
            .map(|forest| forest.repos.len())
            .sum::<usize>();
        if repo_count > 0 {
            lines.push(String::new());
            lines.push("Branch cleanup:".to_string());
            for forest in &result.forests {
                for repo in &forest.repos {
                    lines.push(format!(
                        "  {}/{}",
                        forest.name,
                        format_repo_reset_status(repo, is_preview)
                    ));
                }
            }
        }
    }

    lines.push(String::new());
    let config_status = format_file_status(&result.config_file, is_preview, cleanup_blocked);
    lines.push(format!(
        "Config: {} ({})",
        config_status,
        result.config_file.path.display()
    ));
    let state_status = format_file_status(&result.state_file, is_preview, cleanup_blocked);
    lines.push(format!(
        "State:  {} ({})",
        state_status,
        result.state_file.path.display()
    ));

    if !result.warnings.is_empty() {
        lines.push(String::new());
        lines.push("Warnings:".to_string());
        for warning in &result.warnings {
            lines.push(format!("  {}", warning));
        }
    }

    if !result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in legacy_error_lines(&result.errors) {
            lines.push(format!("  {}", error));
        }
    }

    lines.join("\n")
}

fn format_repo_reset_status(repo: &RepoResetEntry, is_preview: bool) -> String {
    let wt = match &repo.worktree_removed {
        RmOutcome::Success => {
            if is_preview {
                "would remove worktree".to_string()
            } else {
                "worktree removed".to_string()
            }
        }
        RmOutcome::Skipped { reason } => format!("worktree skipped ({})", reason),
        RmOutcome::Failed { .. } => "worktree FAILED".to_string(),
    };

    let branch = match &repo.branch_deleted {
        RmOutcome::Success => {
            if is_preview {
                format!(", would delete branch {}", repo.branch)
            } else {
                format!(", branch deleted {}", repo.branch)
            }
        }
        RmOutcome::Skipped { reason } if reason == "branch not created by forest" => {
            ", branch kept (not created by forest)".to_string()
        }
        RmOutcome::Skipped { reason } => format!(", branch skipped ({})", reason),
        RmOutcome::Failed { .. } => ", branch FAILED".to_string(),
    };

    format!("{}: {}{}", repo.name, wt, branch)
}

fn reset_cleanup_blocked(result: &ResetResult) -> bool {
    result.errors.iter().any(|issue| !file_cleanup_error(issue))
        && ((result.config_file.existed
            && !result.config_file.deleted
            && result.config_file.backed_up_to.is_none())
            || (result.state_file.existed && !result.state_file.deleted)
            || result
                .forests
                .iter()
                .any(|forest| !forest.removed && forest.path_exists))
}

/// Whether an issue comes from backing up the config or deleting the state file, the two
/// steps that run after every forest is cleaned up.
fn file_cleanup_error(issue: &CleanupIssue) -> bool {
    match issue.code {
        CleanupIssueCode::ConfigBackupFailed | CleanupIssueCode::StateDeleteFailed => true,
        CleanupIssueCode::DirtyWorktree
        | CleanupIssueCode::WorktreeRefused
        | CleanupIssueCode::WorktreeRemoveFailed
        | CleanupIssueCode::WorktreeMetadataStale
        | CleanupIssueCode::BranchDeleteFailed
        | CleanupIssueCode::ForestRootRefused
        | CleanupIssueCode::RootEntryRemoveFailed
        | CleanupIssueCode::ForestDirNotRemoved
        | CleanupIssueCode::ForestMetadataStagingFailed
        | CleanupIssueCode::ForestPlanRejected
        | CleanupIssueCode::WorktreeBaseScanFailed => false,
    }
}

/// A forest-root refusal, as an issue.
pub fn root_refused(text: String) -> CleanupIssue {
    CleanupIssue::new(CleanupIssueCode::ForestRootRefused, text)
}

fn format_file_status(entry: &FileResetEntry, is_preview: bool, cleanup_blocked: bool) -> String {
    if is_preview {
        if !entry.existed {
            "not found".to_string()
        } else if !entry.deleted {
            "blocked".to_string()
        } else if entry.backed_up_to.is_some() {
            "would back up".to_string()
        } else {
            "would delete".to_string()
        }
    } else if let Some(backup_path) = &entry.backed_up_to {
        format!("backed up to {}", backup_path.display())
    } else if entry.deleted {
        "deleted".to_string()
    } else if cleanup_blocked && entry.existed {
        "preserved".to_string()
    } else if entry.existed {
        "failed".to_string()
    } else {
        "not found".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{ForestMode, RepoMeta, META_FILENAME};
    use crate::paths::{AbsolutePath, ForestName};
    use crate::plan::branch_state::WorktreeBranchState;
    use crate::plan::rm::{
        BranchObservation, DetachedHeadSafety, ForestDirListing, ForestDirObservation,
        RepoObservation, RootListing, RootObservation, WorktreeEntry,
    };
    use std::ffi::OsString;

    const LOCKED: &str = "refusing removal because git worktree metadata is locked";

    /// A forest at `/wt/<name>` with one clean repo `<repo>`, whose branch the forest created and
    /// can delete; `locked` locks the repo's worktree metadata.
    fn forest(name: &str, repo: &str, locked: bool) -> ForestObservation {
        let path = PathBuf::from(format!("/wt/{name}"));
        let worktree_path = path.join(repo);
        let branch = format!("t/{name}");
        ForestObservation {
            meta: ForestMeta {
                name: ForestName::new(name.to_string()).unwrap(),
                created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                mode: ForestMode::Feature,
                disposable_root_entries: vec![],
                repos: vec![RepoMeta {
                    name: RepoName::new(repo.to_string()).unwrap(),
                    source: AbsolutePath::new(PathBuf::from(format!("/src/{repo}"))).unwrap(),
                    branch: branch.clone(),
                    base_branch: "main".to_string(),
                    remote: Some("origin".to_string()),
                    branch_created: true,
                }],
            },
            path_exists: true,
            rm: RmObservation {
                root: RootObservation::Directory(RootListing {
                    entries: Ok(vec![
                        Ok(OsString::from(META_FILENAME)),
                        Ok(OsString::from(repo)),
                    ]),
                    repo_entries: vec![Ok(true)],
                }),
                repos: vec![RepoObservation {
                    worktree_inside_forest: true,
                    worktree_exists: true,
                    branch_state: WorktreeBranchState::new(
                        branch.clone(),
                        ActualBranchState::Branch {
                            actual_branch: branch.clone(),
                        },
                    ),
                    source_exists: true,
                    detached_head_safety: DetachedHeadSafety::NotDetached,
                    has_dirty_files: false,
                }],
                dry_run: Some(RmDryRunObservation {
                    current_root_refusal: None,
                    repos: vec![RepoDryRunObservation {
                        worktree_is_symlink: false,
                        worktree_canonical: Some(worktree_path.clone()),
                        worktrees: Some(Ok(vec![WorktreeEntry {
                            path: worktree_path.clone(),
                            canonical: Some(worktree_path),
                            branch: Some(format!("refs/heads/{branch}")),
                            locked,
                        }])),
                        branch: Some(BranchObservation {
                            exists: Ok(true),
                            deletes_without_force: true,
                            safe_to_force_delete: false,
                        }),
                    }],
                    forest_dir: ForestDirObservation {
                        exists: true,
                        is_symlink: false,
                        listing: ForestDirListing::Entries(vec![]),
                    },
                }),
            },
            path,
        }
    }

    /// A reset plan over an existing config and state file whose worktree bases hold
    /// `forests`, after `base_failures`.
    fn plan(base_failures: Vec<BaseScanFailure>, forests: Vec<ForestObservation>) -> ResetPlan {
        decide_reset_plan(
            false,
            ResetObservation {
                config_path: Ok(PathBuf::from("/home/.config/git-forest/config.toml")),
                state_path: Ok(PathBuf::from("/home/.local/state/git-forest/state.toml")),
                files: Some(ResetFilesObservation {
                    config_exists: true,
                    state_exists: true,
                    config: Some(ConfigObservation::Scanned(DiscoveryObservation {
                        base_failures,
                        forests,
                    })),
                }),
            },
        )
        .unwrap()
    }

    #[test]
    fn decide_reset_requires_confirmation_only_without_errors() {
        let clean =
            plan_to_confirm_required(&plan(vec![], vec![forest("alpha", "foo-api", false)]));

        assert!(clean.confirm_required);
        assert!(!clean.dry_run);
        assert!(clean.errors.is_empty());
        assert!(clean.forests[0].removed);
        assert!(clean.config_file.deleted);

        let locked =
            plan_to_confirm_required(&plan(vec![], vec![forest("alpha", "foo-api", true)]));

        assert!(!locked.confirm_required);
        assert!(!locked.dry_run);
        assert!(!locked.errors.is_empty());
        assert!(!locked.config_file.deleted);
    }

    #[test]
    fn decide_reset_dry_run_uses_rm_decisions() {
        let result = plan_to_dry_run(&plan(vec![], vec![forest("alpha", "foo-api", true)]));

        assert!(result.dry_run);
        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![format!("foo-api: {LOCKED}")]
        );
        assert_eq!(result.errors[0].code, CleanupIssueCode::WorktreeRefused);
        let repo = &result.forests[0].repos[0];
        assert!(matches!(repo.worktree_removed, RmOutcome::Failed { .. }));
        assert_eq!(
            repo.branch_deleted,
            RmOutcome::Skipped {
                reason: "worktree not removed".to_string()
            }
        );
        assert!(!result.forests[0].removed);
        assert!(!result.state_file.deleted);
    }

    #[test]
    fn render_reset_reports_blocked_and_already_missing_forests_from_facts() {
        let entry = |name: &str, path_exists: bool| ForestResetEntry {
            name: name.to_string(),
            path: PathBuf::from(format!("/wt/{name}")),
            removed: false,
            repos: vec![],
            path_exists,
        };
        let mut result = ResetResult {
            dry_run: true,
            confirm_required: false,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/home/config.toml"),
                existed: true,
                deleted: false,
                backed_up_to: None,
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/home/state.toml"),
                existed: false,
                deleted: false,
                backed_up_to: None,
            },
            forests: vec![entry("alpha", true), entry("beta", false)],
            warnings: vec![],
            errors: vec![CleanupIssue::new(
                CleanupIssueCode::WorktreeRefused,
                format!("foo-api: {LOCKED}"),
            )],
        };

        let preview = format_reset_human(&result);

        assert!(
            preview.contains("  alpha: blocked (/wt/alpha)"),
            "{preview}"
        );
        assert!(
            preview.contains("  beta: already missing (/wt/beta)"),
            "{preview}"
        );

        result.dry_run = false;
        result.forests = vec![entry("beta", false)];
        result.config_file.existed = false;

        let unblocked = format_reset_human(&result);

        assert!(
            unblocked.starts_with("Reset completed with errors."),
            "{unblocked}"
        );
        assert!(
            unblocked.contains("  beta: failed (/wt/beta)"),
            "{unblocked}"
        );

        result.forests = vec![entry("alpha", true)];

        let blocked = format_reset_human(&result);

        assert!(blocked.starts_with("Reset blocked by errors."), "{blocked}");
        assert!(
            blocked.contains("  alpha: blocked (/wt/alpha)"),
            "{blocked}"
        );
    }

    #[test]
    fn decide_reset_keeps_todays_issue_order_across_forests() {
        let result = plan_to_dry_run(&plan(
            vec![BaseScanFailure {
                base: PathBuf::from("/wt2"),
                error: "permission denied".to_string(),
            }],
            vec![
                forest("alpha", "foo-api", true),
                forest("beta", "foo-web", true),
            ],
        ));

        assert_eq!(
            legacy_error_lines(&result.errors),
            vec![
                "could not safely scan worktree base /wt2: permission denied\n  hint: config and state will be preserved until forest recovery is complete".to_string(),
                format!("foo-api: {LOCKED}"),
                format!("foo-web: {LOCKED}"),
            ]
        );
        assert_eq!(result.forests.len(), 2);
        assert!(result.forests.iter().all(|forest| !forest.removed));
    }
}
