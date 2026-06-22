use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

use super::branch_state::{compact_git_error, ActualBranchState, WorktreeBranchState};
use crate::forest::{dedupe_discovered_forests, discover_forests_with_dirs};
use crate::meta::{ForestMeta, META_FILENAME};
use crate::paths::{AbsolutePath, ForestName, RepoName};

// --- Types ---

pub struct RmPlan {
    pub forest_name: ForestName,
    pub forest_dir: PathBuf,
    pub repo_plans: Vec<RepoRmPlan>,
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
    pub forest_dir_removed: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RepoRmResult {
    pub name: RepoName,
    pub branch_state: WorktreeBranchState,
    pub worktree_removed: RmOutcome,
    pub branch_deleted: RmOutcome,
}

#[derive(Debug, Serialize, PartialEq)]
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

// --- Planning (read-only) ---

pub fn plan_rm(forest_dir: &std::path::Path, meta: &ForestMeta) -> RmPlan {
    let repo_plans = meta
        .repos
        .iter()
        .map(|repo| {
            let worktree_path = forest_dir.join(repo.name.as_str());
            assert!(
                worktree_path_is_inside_forest(&worktree_path, forest_dir),
                "worktree path {:?} is not inside forest dir {:?}",
                worktree_path,
                forest_dir
            );
            let worktree_exists = path_exists_or_symlink(&worktree_path);
            let worktree_is_symlink = path_is_symlink(&worktree_path);
            let branch_state = WorktreeBranchState::read(&worktree_path, &repo.branch);
            let source_exists = repo.source.is_dir();
            let detached_head_safety =
                detached_head_safety(&branch_state, &repo.source, source_exists);
            let has_dirty_files = worktree_exists
                && !worktree_is_symlink
                && !matches!(&branch_state.actual, ActualBranchState::Unknown { .. })
                && crate::git::git(&worktree_path, &["status", "--porcelain"])
                    .map(|output| !output.is_empty())
                    .unwrap_or(false);
            RepoRmPlan {
                name: repo.name.clone(),
                worktree_path: worktree_path.clone(),
                source: repo.source.clone(),
                branch: repo.branch.clone(),
                base_branch: repo.base_branch.clone(),
                remote: repo.remote.clone(),
                branch_created: repo.branch_created,
                branch_state,
                detached_head_safety,
                worktree_exists,
                source_exists,
                has_dirty_files,
            }
        })
        .collect();

    RmPlan {
        forest_name: meta.name.clone(),
        forest_dir: forest_dir.to_path_buf(),
        repo_plans,
    }
}

fn detached_head_safety(
    branch_state: &WorktreeBranchState,
    source: &AbsolutePath,
    source_exists: bool,
) -> DetachedHeadSafety {
    let ActualBranchState::Detached {
        actual_detached_head,
    } = &branch_state.actual
    else {
        return DetachedHeadSafety::NotDetached;
    };

    if !source_exists {
        return DetachedHeadSafety::Unverified {
            head: actual_detached_head.clone(),
            error: "source repo missing".to_string(),
        };
    }

    match detached_head_is_preserved(source, actual_detached_head) {
        Ok(true) => DetachedHeadSafety::Preserved,
        Ok(false) => DetachedHeadSafety::Unpreserved {
            head: actual_detached_head.clone(),
        },
        Err(e) => DetachedHeadSafety::Unverified {
            head: actual_detached_head.clone(),
            error: e.to_string(),
        },
    }
}

fn detached_head_is_preserved(source: &AbsolutePath, head: &str) -> Result<bool> {
    let commit = format!("{}^{{commit}}", head);
    let full_head = crate::git::git(source, &["rev-parse", "--verify", &commit])?;
    let containing_refs = crate::git::git(
        source,
        &[
            "for-each-ref",
            "--contains",
            &full_head,
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;

    Ok(containing_refs.lines().next().is_some())
}

fn worktree_removal_safety_error(repo_plan: &RepoRmPlan, force: bool) -> Option<String> {
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

    if path_is_symlink(&repo_plan.worktree_path) {
        return Some(format!(
            "{}: worktree path is a symlink; refusing to remove unverified worktree without `git forest rm --force`",
            repo_plan.name
        ));
    }

    None
}

// --- Execution (impure) ---

pub fn execute_rm(
    plan: &RmPlan,
    force: bool,
    on_progress: Option<&dyn Fn(RmProgress)>,
) -> RmResult {
    validate_rm_plan_paths(plan);

    // Preflight: if not forcing, reject if any repo has dirty files
    if !force {
        let dirty_repos: Vec<&RepoRmPlan> = plan
            .repo_plans
            .iter()
            .filter(|rp| rp.has_dirty_files)
            .collect();

        if !dirty_repos.is_empty() {
            let mut repos = Vec::new();
            let mut errors = Vec::new();

            for rp in &plan.repo_plans {
                if let Some(cb) = &on_progress {
                    cb(RmProgress::RepoStarting { name: &rp.name });
                }

                let (worktree_removed, branch_deleted) = if rp.has_dirty_files {
                    let msg = format!("{}: worktree has uncommitted changes", rp.name);
                    errors.push(msg.clone());
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
                let result = RepoRmResult {
                    name: rp.name.clone(),
                    branch_state: rp.branch_state.clone(),
                    worktree_removed,
                    branch_deleted,
                };

                if let Some(cb) = &on_progress {
                    cb(RmProgress::RepoDone(&result));
                }
                repos.push(result);
            }

            errors.push(
                "hint: commit or stash changes, then retry — or use `git forest rm --force`"
                    .to_string(),
            );

            return RmResult {
                forest_name: plan.forest_name.clone(),
                forest_dir: plan.forest_dir.clone(),
                dry_run: false,
                force,
                repos,
                forest_dir_removed: false,
                errors,
            };
        }
    }

    let mut repos = Vec::new();
    let mut errors = Vec::new();

    for repo_plan in &plan.repo_plans {
        if let Some(cb) = &on_progress {
            cb(RmProgress::RepoStarting {
                name: &repo_plan.name,
            });
        }

        let (worktree_removed, wt_succeeded) = remove_worktree(repo_plan, force, &mut errors);

        let branch_deleted = delete_branch(repo_plan, force, wt_succeeded, &mut errors);

        let result = RepoRmResult {
            name: repo_plan.name.clone(),
            branch_state: repo_plan.branch_state.clone(),
            worktree_removed,
            branch_deleted,
        };

        if let Some(cb) = &on_progress {
            cb(RmProgress::RepoDone(&result));
        }
        repos.push(result);
    }

    // Defense-in-depth: only remove forest dir if no errors accumulated
    let forest_dir_removed = if errors.is_empty() {
        remove_forest_dir(&plan.forest_dir, force, &mut errors)
    } else {
        // Partial failure (unexpected runtime error) — keep meta for discoverability
        false
    };

    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run: false,
        force,
        repos,
        forest_dir_removed,
        errors,
    }
}

fn validate_rm_plan_paths(plan: &RmPlan) {
    for repo_plan in &plan.repo_plans {
        assert!(
            worktree_path_is_inside_forest(&repo_plan.worktree_path, &plan.forest_dir),
            "worktree path {:?} is not inside forest dir {:?}",
            repo_plan.worktree_path,
            plan.forest_dir
        );
    }
}

fn remove_worktree(
    repo_plan: &RepoRmPlan,
    force: bool,
    errors: &mut Vec<String>,
) -> (RmOutcome, bool) {
    if !repo_plan.worktree_exists {
        if let Some(msg) = stale_missing_worktree_metadata_error(repo_plan, force) {
            errors.push(msg.clone());
            return (RmOutcome::Failed { error: msg }, false);
        }
        return (
            RmOutcome::Skipped {
                reason: "worktree already missing".to_string(),
            },
            true, // treat as success for branch deletion purposes
        );
    }

    if let Some(msg) = worktree_removal_safety_error(repo_plan, force) {
        errors.push(msg.clone());
        return (RmOutcome::Failed { error: msg }, false);
    }

    if !repo_plan.source_exists {
        // Source repo is gone — can't use git, remove directory directly
        match remove_corrupt_worktree_path(&repo_plan.worktree_path) {
            Ok(()) => {
                return (RmOutcome::Success, true);
            }
            Err(e) => {
                let msg = format!(
                    "{}: source repo missing, failed to remove worktree directory: {}",
                    repo_plan.name, e
                );
                errors.push(msg.clone());
                return (RmOutcome::Failed { error: msg }, false);
            }
        }
    }

    if path_is_symlink(&repo_plan.worktree_path) {
        return remove_corrupt_worktree_dir(
            repo_plan,
            "worktree path is a symlink".to_string(),
            errors,
        );
    }

    let wt_path_str = repo_plan.worktree_path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&wt_path_str);

    match crate::git::git(&repo_plan.source, &args) {
        Ok(_) => (RmOutcome::Success, true),
        Err(e) => {
            if force
                && repo_plan.source_exists
                && matches!(
                    repo_plan.branch_state.actual,
                    ActualBranchState::Unknown { .. }
                )
            {
                return remove_corrupt_worktree_dir(repo_plan, e.to_string(), errors);
            }

            let msg = format!("{}: git worktree remove failed: {}", repo_plan.name, e);
            errors.push(msg.clone());
            (RmOutcome::Failed { error: msg }, false)
        }
    }
}

pub(super) fn stale_missing_worktree_metadata_error(
    repo_plan: &RepoRmPlan,
    force: bool,
) -> Option<String> {
    if !repo_plan.source_exists {
        return None;
    }

    let target_canonical = canonicalize_existing_or_parent(&repo_plan.worktree_path);
    match worktree_metadata_for_path(repo_plan, target_canonical.as_deref()) {
        Ok(Some(_)) if force => prune_stale_worktree_metadata(repo_plan).err(),
        Ok(Some(_)) => Some(format!(
            "{}: source repo still lists missing worktree metadata for {}; run `git -C {} worktree prune` or retry with `git forest rm --force`",
            repo_plan.name,
            repo_plan.worktree_path.display(),
            repo_plan.source
        )),
        Ok(None) => None,
        Err(e) if force => Some(format!(
            "{}: could not inspect git worktree metadata before missing-worktree cleanup: {}",
            repo_plan.name,
            compact_git_error(&e)
        )),
        Err(_) => None,
    }
}

fn prune_stale_worktree_metadata(repo_plan: &RepoRmPlan) -> Result<(), String> {
    crate::git::git(&repo_plan.source, &["worktree", "prune", "--expire", "now"]).map_err(|e| {
        format!(
            "{}: failed to prune stale missing worktree metadata: {}",
            repo_plan.name,
            compact_git_error(&e.to_string())
        )
    })?;

    let target_canonical = canonicalize_existing_or_parent(&repo_plan.worktree_path);
    match worktree_metadata_for_path(repo_plan, target_canonical.as_deref()) {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(format!(
            "{}: git worktree metadata still lists missing worktree {} after prune",
            repo_plan.name,
            repo_plan.worktree_path.display()
        )),
        Err(e) => Err(format!(
            "{}: failed to verify stale missing worktree metadata was pruned: {}",
            repo_plan.name,
            compact_git_error(&e)
        )),
    }
}

fn remove_corrupt_worktree_dir(
    repo_plan: &RepoRmPlan,
    git_error: String,
    errors: &mut Vec<String>,
) -> (RmOutcome, bool) {
    let target_canonical = canonicalize_existing_or_parent(&repo_plan.worktree_path);
    match worktree_metadata_for_path(repo_plan, target_canonical.as_deref()) {
        Ok(Some(metadata)) if metadata.locked => {
            let msg = format!(
                "{}: git worktree remove failed: {}; refusing direct removal because git worktree metadata is locked",
                repo_plan.name,
                compact_git_error(&git_error)
            );
            errors.push(msg.clone());
            return (RmOutcome::Failed { error: msg }, false);
        }
        Ok(_) => {}
        Err(e) => {
            let msg = format!(
                "{}: git worktree remove failed: {}; could not inspect git worktree metadata before direct removal: {}",
                repo_plan.name,
                compact_git_error(&git_error),
                compact_git_error(&e)
            );
            errors.push(msg.clone());
            return (RmOutcome::Failed { error: msg }, false);
        }
    }

    if let Err(e) = remove_corrupt_worktree_path(&repo_plan.worktree_path) {
        let msg = format!(
            "{}: git worktree remove failed: {}; direct removal also failed: {}",
            repo_plan.name,
            compact_git_error(&git_error),
            e
        );
        errors.push(msg.clone());
        return (RmOutcome::Failed { error: msg }, false);
    }

    match crate::git::git(&repo_plan.source, &["worktree", "prune", "--expire", "now"]) {
        Ok(_) => {
            match worktree_metadata_for_path(repo_plan, target_canonical.as_deref()) {
                Ok(Some(_)) => {
                    let msg = format!(
                        "{}: removed corrupt worktree directory but git worktree metadata still lists it",
                        repo_plan.name
                    );
                    errors.push(msg.clone());
                    return (RmOutcome::Failed { error: msg }, false);
                }
                Ok(None) => {}
                Err(e) => {
                    let msg = format!(
                        "{}: removed corrupt worktree directory but failed to verify git worktree metadata was pruned: {}",
                        repo_plan.name,
                        compact_git_error(&e)
                    );
                    errors.push(msg.clone());
                    return (RmOutcome::Failed { error: msg }, false);
                }
            }
            (RmOutcome::Success, true)
        }
        Err(e) => {
            let msg = format!(
                "{}: removed corrupt worktree directory but failed to prune git worktree metadata: {}",
                repo_plan.name,
                compact_git_error(&e.to_string())
            );
            errors.push(msg.clone());
            (RmOutcome::Failed { error: msg }, false)
        }
    }
}

fn remove_corrupt_worktree_path(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn canonicalize_existing_or_parent(path: &Path) -> Option<PathBuf> {
    if path_is_symlink(path) {
        return canonicalize_parent_join_leaf(path);
    }

    if let Ok(path) = path.canonicalize() {
        return Some(path);
    }

    canonicalize_parent_join_leaf(path)
}

fn canonicalize_parent_join_leaf(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?.canonicalize().ok()?;
    let file_name = path.file_name()?;
    Some(parent.join(file_name))
}

fn worktree_path_is_inside_forest(worktree_path: &Path, forest_dir: &Path) -> bool {
    if let (Some(worktree), Ok(forest)) = (
        canonicalize_existing_or_parent(worktree_path),
        forest_dir.canonicalize(),
    ) {
        return worktree
            .strip_prefix(&forest)
            .map(|relative| !path_has_parent_dir(relative))
            .unwrap_or(false);
    }

    if forest_dir.canonicalize().is_ok() {
        if let Ok(relative) = worktree_path.strip_prefix(forest_dir) {
            return !path_has_parent_dir(relative);
        }
    }

    if path_has_parent_dir(worktree_path) || path_has_parent_dir(forest_dir) {
        return false;
    }

    worktree_path
        .strip_prefix(forest_dir)
        .map(|relative| !path_has_parent_dir(relative))
        .unwrap_or(false)
}

fn path_exists_or_symlink(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn path_is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

struct WorktreeMetadata {
    locked: bool,
}

struct WorktreeListEntry {
    path: PathBuf,
    branch: Option<String>,
    locked: bool,
}

fn path_has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn worktree_list_entries(source: &Path) -> Result<Vec<WorktreeListEntry>, String> {
    let output = crate::git::git(source, &["worktree", "list", "--porcelain", "-z"])
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;
    let mut current_locked = false;

    for token in output.split('\0') {
        if token.is_empty() {
            if let Some(path) = current_path.take() {
                entries.push(WorktreeListEntry {
                    path,
                    branch: current_branch.take(),
                    locked: current_locked,
                });
            }
            current_branch = None;
            current_locked = false;
            continue;
        }

        if let Some(path) = token.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if let Some(branch) = token.strip_prefix("branch ") {
            current_branch = Some(branch.to_string());
        } else if token == "locked" || token.starts_with("locked ") {
            current_locked = true;
        }
    }

    if let Some(path) = current_path {
        entries.push(WorktreeListEntry {
            path,
            branch: current_branch,
            locked: current_locked,
        });
    }

    Ok(entries)
}

fn worktree_metadata_for_path(
    repo_plan: &RepoRmPlan,
    target_canonical: Option<&Path>,
) -> Result<Option<WorktreeMetadata>, String> {
    for entry in worktree_list_entries(&repo_plan.source)? {
        if worktree_paths_match(&entry.path, &repo_plan.worktree_path, target_canonical) {
            return Ok(Some(WorktreeMetadata {
                locked: entry.locked,
            }));
        }
    }

    Ok(None)
}

fn worktree_paths_match(listed: &Path, target: &Path, target_canonical: Option<&Path>) -> bool {
    if listed == target {
        return true;
    }

    let Some(target_canonical) = target_canonical else {
        return false;
    };

    if listed == target_canonical {
        return true;
    }

    listed
        .canonicalize()
        .map(|listed| listed == target_canonical)
        .unwrap_or(false)
}

pub(super) fn delete_branch(
    repo_plan: &RepoRmPlan,
    force: bool,
    wt_succeeded: bool,
    errors: &mut Vec<String>,
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

    // Branch already gone — nothing to do (idempotent rm)
    let refname = format!("refs/heads/{}", repo_plan.branch);
    if let Ok(false) = crate::git::ref_exists(&repo_plan.source, &refname) {
        return RmOutcome::Skipped {
            reason: "branch already deleted".to_string(),
        };
    }

    if force {
        return match crate::git::git(&repo_plan.source, &["branch", "-D", &repo_plan.branch]) {
            Ok(_) => RmOutcome::Success,
            Err(e) => {
                // TOCTOU guard: branch may have been deleted between ref_exists and now
                if let Ok(false) = crate::git::ref_exists(&repo_plan.source, &refname) {
                    return RmOutcome::Skipped {
                        reason: "branch already deleted".to_string(),
                    };
                }
                let msg = format!("{}: git branch -D failed: {}", repo_plan.name, e);
                errors.push(msg.clone());
                RmOutcome::Failed { error: msg }
            }
        };
    }

    // Try safe delete first
    match crate::git::git(&repo_plan.source, &["branch", "-d", &repo_plan.branch]) {
        Ok(_) => RmOutcome::Success,
        Err(original_err) => {
            // TOCTOU guard: branch may have been deleted between ref_exists and now
            if let Ok(false) = crate::git::ref_exists(&repo_plan.source, &refname) {
                return RmOutcome::Skipped {
                    reason: "branch already deleted".to_string(),
                };
            }
            // -d failed — check if the branch was merged via a different mechanism
            // (e.g. fully merged but HEAD isn't on base, or fully pushed to upstream)
            if can_safely_force_delete(
                &repo_plan.source,
                &repo_plan.branch,
                &repo_plan.base_branch,
                repo_plan.remote.as_deref(),
            ) {
                match crate::git::git(&repo_plan.source, &["branch", "-D", &repo_plan.branch]) {
                    Ok(_) => RmOutcome::Success,
                    Err(e) => {
                        let msg = format!("{}: git branch -D failed: {}", repo_plan.name, e);
                        errors.push(msg.clone());
                        RmOutcome::Failed { error: msg }
                    }
                }
            } else {
                let msg = branch_not_fully_merged_error(repo_plan, Some(&original_err.to_string()));
                errors.push(msg.clone());
                RmOutcome::Failed { error: msg }
            }
        }
    }
}

pub(super) fn plan_branch_delete_outcome(
    repo_plan: &RepoRmPlan,
    force: bool,
    wt_succeeded: bool,
    errors: &mut Vec<String>,
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

    let refname = format!("refs/heads/{}", repo_plan.branch);
    if let Ok(false) = crate::git::ref_exists(&repo_plan.source, &refname) {
        return RmOutcome::Skipped {
            reason: "branch already deleted".to_string(),
        };
    }

    if let Some(msg) = branch_checkout_conflict_error(repo_plan) {
        errors.push(msg.clone());
        return RmOutcome::Failed { error: msg };
    }

    if force
        || branch_delete_would_succeed_without_force(&repo_plan.source, &repo_plan.branch)
        || can_safely_force_delete(
            &repo_plan.source,
            &repo_plan.branch,
            &repo_plan.base_branch,
            repo_plan.remote.as_deref(),
        )
    {
        return RmOutcome::Success;
    }

    let msg = branch_not_fully_merged_error(
        repo_plan,
        Some(
            "dry-run could not prove the branch is merged into HEAD, the base branch, \
             the base remote-tracking ref, or its upstream",
        ),
    );
    errors.push(msg.clone());
    RmOutcome::Failed { error: msg }
}

fn branch_checkout_conflict_error(repo_plan: &RepoRmPlan) -> Option<String> {
    match branch_checked_out_elsewhere(repo_plan) {
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
    }
}

fn branch_checked_out_elsewhere(repo_plan: &RepoRmPlan) -> Result<Option<PathBuf>, String> {
    let expected_branch_ref = format!("refs/heads/{}", repo_plan.branch);
    let target_canonical = canonicalize_existing_or_parent(&repo_plan.worktree_path);
    for entry in worktree_list_entries(&repo_plan.source)? {
        if entry.branch.as_deref() == Some(expected_branch_ref.as_str())
            && !worktree_paths_match(
                &entry.path,
                &repo_plan.worktree_path,
                target_canonical.as_deref(),
            )
        {
            return Ok(Some(entry.path));
        }
    }

    Ok(None)
}

fn branch_not_fully_merged_error(repo_plan: &RepoRmPlan, detail: Option<&str>) -> String {
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

fn branch_delete_would_succeed_without_force(source: &AbsolutePath, branch: &str) -> bool {
    // `git branch -d` checks the configured upstream when one exists; otherwise
    // it falls back to HEAD. Mirror that read-only so dry-run does not mutate.
    if let Some(upstream) = branch_upstream_ref(source, branch) {
        return is_ancestor_of_ref(source, branch, &upstream);
    }

    is_ancestor_of_ref(source, branch, "HEAD")
}

/// Check whether a branch can be safely force-deleted after `git branch -d` fails.
///
/// Three checks, tried in order:
/// 1. `git merge-base --is-ancestor <branch> <base_branch>` — the branch is fully
///    merged into the base branch (catches cases where `-d` failed due to HEAD position).
/// 2. `git merge-base --is-ancestor <branch> <remote>/<base_branch>` — the branch
///    is fully merged into the recorded base remote, even if local base is stale.
/// 3. The branch has an upstream and all local commits are pushed to it
///    (`git rev-list --count <upstream>..<branch>` == 0). This proves no local-only
///    commits exist, making deletion safe even when the remote branch was squash-merged
///    and then deleted.
fn can_safely_force_delete(
    source: &AbsolutePath,
    branch: &str,
    base_branch: &str,
    remote: Option<&str>,
) -> bool {
    // Fallback 1: branch is ancestor of base_branch (fully merged)
    if is_ancestor_of_ref(source, branch, base_branch) {
        return true;
    }

    // Fallback 2: branch is ancestor of the base branch's remote-tracking ref.
    if let Some(base_upstream) = base_branch_remote_ref(source, base_branch, remote) {
        if is_ancestor_of_ref(source, branch, &base_upstream) {
            return true;
        }
    }

    // Fallback 3: branch has upstream tracking and no unpushed commits
    if let Some(upstream) = branch_upstream_ref(source, branch) {
        let range = format!("{}..{}", upstream, branch);
        if let Ok(count) = crate::git::git(source, &["rev-list", "--count", &range]) {
            if count.trim() == "0" {
                return true;
            }
        }
    }

    false
}

fn branch_upstream_ref(source: &AbsolutePath, branch: &str) -> Option<String> {
    let upstream = crate::git::git(
        source,
        &[
            "for-each-ref",
            "--format=%(upstream)",
            &format!("refs/heads/{}", branch),
        ],
    )
    .ok()?;
    if upstream.is_empty() {
        None
    } else {
        Some(upstream)
    }
}

fn is_ancestor_of_ref(source: &AbsolutePath, branch: &str, refname: &str) -> bool {
    crate::git::git(source, &["merge-base", "--is-ancestor", branch, refname]).is_ok()
}

fn base_branch_remote_ref(
    source: &AbsolutePath,
    base_branch: &str,
    remote: Option<&str>,
) -> Option<String> {
    if let Some(remote) = remote {
        let remote_base_ref = format!("refs/remotes/{}/{}", remote, base_branch);
        if matches!(crate::git::ref_exists(source, &remote_base_ref), Ok(true)) {
            return Some(remote_base_ref);
        }
        return None;
    }

    let local_base_ref = format!("refs/heads/{}", base_branch);
    if let Ok(upstream) = crate::git::git(
        source,
        &["for-each-ref", "--format=%(upstream)", &local_base_ref],
    ) {
        if !upstream.is_empty() {
            return Some(upstream);
        }
    }

    if let Ok(refs) = crate::git::git(
        source,
        &["for-each-ref", "--format=%(refname)", "refs/remotes"],
    ) {
        let refs: Vec<&str> = refs
            .lines()
            .filter(|line| {
                line.strip_prefix("refs/remotes/")
                    .and_then(|remote_branch| remote_branch.split_once('/'))
                    .map(|(_remote, branch)| branch == base_branch)
                    .unwrap_or(false)
            })
            .collect();
        if refs.len() == 1 {
            return Some(refs[0].to_string());
        }
    }

    None
}

fn remove_forest_dir(forest_dir: &std::path::Path, force: bool, errors: &mut Vec<String>) -> bool {
    if !forest_dir.exists() {
        return true;
    }

    if path_is_symlink(forest_dir) {
        if force {
            return match std::fs::remove_file(forest_dir) {
                Ok(()) => true,
                Err(e) => {
                    errors.push(format!("failed to remove forest directory symlink: {}", e));
                    false
                }
            };
        }

        errors.push(format!(
            "forest directory not removed: {} is a symlink\n  hint: inspect the symlink target, then re-run with --force to unlink the forest directory symlink",
            forest_dir.display()
        ));
        return false;
    }

    if force {
        match std::fs::remove_dir_all(forest_dir) {
            Ok(()) => true,
            Err(e) => {
                errors.push(format!("failed to remove forest directory: {}", e));
                false
            }
        }
    } else {
        let meta_path = forest_dir.join(META_FILENAME);
        if let Some(error) = non_force_forest_dir_cleanup_error(forest_dir, &meta_path) {
            errors.push(error);
            return false;
        }

        if meta_path.exists() {
            if let Err(e) = std::fs::remove_file(&meta_path) {
                errors.push(format!("failed to remove meta file: {}", e));
                return false;
            }
        }

        match std::fs::remove_dir(forest_dir) {
            Ok(()) => true,
            Err(e) => {
                errors.push(format!(
                    "forest directory not removed (not empty): {}\n  hint: resolve errors above, then rm the directory manually or re-run with --force",
                    e
                ));
                false
            }
        }
    }
}

fn non_force_forest_dir_cleanup_error(forest_dir: &Path, meta_path: &Path) -> Option<String> {
    let entries = match std::fs::read_dir(forest_dir) {
        Ok(entries) => entries,
        Err(e) => {
            return Some(format!(
                "forest directory not removed (not empty): failed to inspect {}: {}\n  hint: resolve errors above, then rm the directory manually or re-run with --force",
                forest_dir.display(),
                e
            ));
        }
    };

    let mut remaining = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                return Some(format!(
                    "forest directory not removed (not empty): failed to inspect an entry in {}: {}\n  hint: resolve errors above, then rm the directory manually or re-run with --force",
                    forest_dir.display(),
                    e
                ));
            }
        };

        if entry.path() == meta_path {
            continue;
        }

        remaining.push(entry.file_name().to_string_lossy().into_owned());
    }

    if remaining.is_empty() {
        None
    } else {
        remaining.sort();
        Some(format!(
            "forest directory not removed (not empty): would still contain {}\n  hint: resolve errors above, then rm the directory manually or re-run with --force",
            remaining.join(", ")
        ))
    }
}

// --- Orchestrator ---

fn plan_to_dry_run_result(plan: &RmPlan, force: bool) -> RmResult {
    validate_rm_plan_paths(plan);

    let has_dirty = !force && plan.repo_plans.iter().any(|rp| rp.has_dirty_files);

    let mut errors = Vec::new();
    let repos: Vec<RepoRmResult> = plan
        .repo_plans
        .iter()
        .map(|rp| {
            if has_dirty {
                // Dirty preflight would block all removal
                let (worktree_removed, branch_deleted) = if rp.has_dirty_files {
                    let msg = format!("{}: worktree has uncommitted changes", rp.name);
                    errors.push(msg.clone());
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

            let worktree_removed = if path_is_symlink(&rp.worktree_path) {
                if force {
                    if let Some(msg) = symlink_worktree_dry_run_error(rp) {
                        errors.push(msg.clone());
                        RmOutcome::Failed { error: msg }
                    } else {
                        RmOutcome::Success
                    }
                } else {
                    let msg = worktree_removal_safety_error(rp, force)
                        .expect("non-force symlink worktree should have a safety error");
                    errors.push(msg.clone());
                    RmOutcome::Failed { error: msg }
                }
            } else if let Some(msg) = worktree_metadata_dry_run_error(rp, force) {
                errors.push(msg.clone());
                RmOutcome::Failed { error: msg }
            } else if !rp.worktree_exists {
                RmOutcome::Skipped {
                    reason: "worktree already missing".to_string(),
                }
            } else if let Some(msg) = worktree_removal_safety_error(rp, force) {
                errors.push(msg.clone());
                RmOutcome::Failed { error: msg }
            } else {
                RmOutcome::Success
            };

            let wt_succeeded = !matches!(&worktree_removed, RmOutcome::Failed { .. });
            let branch_deleted = plan_branch_delete_outcome(rp, force, wt_succeeded, &mut errors);

            RepoRmResult {
                name: rp.name.clone(),
                branch_state: rp.branch_state.clone(),
                worktree_removed,
                branch_deleted,
            }
        })
        .collect();

    if has_dirty {
        errors.push(
            "hint: commit or stash changes, then retry — or use `git forest rm --force`"
                .to_string(),
        );
    }

    if errors.is_empty() {
        if let Some(msg) = forest_dir_dry_run_cleanup_error(plan, &repos, force) {
            errors.push(msg);
        }
    }

    let forest_dir_removed = errors.is_empty();

    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run: true,
        force,
        repos,
        forest_dir_removed,
        errors,
    }
}

pub(super) fn worktree_metadata_dry_run_error(
    repo_plan: &RepoRmPlan,
    force: bool,
) -> Option<String> {
    if !repo_plan.source_exists {
        return None;
    }

    let target_canonical = canonicalize_existing_or_parent(&repo_plan.worktree_path);
    match worktree_metadata_for_path(repo_plan, target_canonical.as_deref()) {
        Ok(Some(metadata)) if metadata.locked => Some(format!(
            "{}: refusing removal because git worktree metadata is locked",
            repo_plan.name
        )),
        Ok(Some(_)) if !repo_plan.worktree_exists && !force => Some(format!(
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

fn symlink_worktree_dry_run_error(repo_plan: &RepoRmPlan) -> Option<String> {
    if !repo_plan.source_exists {
        return None;
    }

    let target_canonical = canonicalize_existing_or_parent(&repo_plan.worktree_path);
    match worktree_metadata_for_path(repo_plan, target_canonical.as_deref()) {
        Ok(Some(metadata)) if metadata.locked => Some(format!(
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

fn forest_dir_dry_run_cleanup_error(
    plan: &RmPlan,
    repos: &[RepoRmResult],
    force: bool,
) -> Option<String> {
    if force || !plan.forest_dir.exists() {
        return None;
    }

    if path_is_symlink(&plan.forest_dir) {
        return Some(format!(
            "forest directory not removed: {} is a symlink",
            plan.forest_dir.display()
        ));
    }

    let removable_repo_names: std::collections::BTreeSet<&str> = repos
        .iter()
        .filter_map(|repo| match &repo.worktree_removed {
            RmOutcome::Success => Some(repo.name.as_str()),
            RmOutcome::Skipped { .. } | RmOutcome::Failed { .. } => None,
        })
        .collect();

    let entries = match std::fs::read_dir(&plan.forest_dir) {
        Ok(entries) => entries,
        Err(e) => {
            return Some(format!(
                "forest directory not removed (not empty): failed to inspect {}: {}",
                plan.forest_dir.display(),
                e
            ));
        }
    };

    let mut remaining = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return Some(format!(
                "forest directory not removed (not empty): failed to inspect an entry in {}",
                plan.forest_dir.display()
            ));
        };
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name == META_FILENAME || removable_repo_names.contains(file_name.as_ref()) {
            continue;
        }
        remaining.push(file_name.into_owned());
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

pub fn cmd_rm(
    forest_dir: &std::path::Path,
    meta: &ForestMeta,
    force: bool,
    dry_run: bool,
    on_progress: Option<&dyn Fn(RmProgress)>,
) -> Result<RmResult> {
    let plan = plan_rm(forest_dir, meta);

    if dry_run {
        return Ok(plan_to_dry_run_result(&plan, force));
    }

    Ok(execute_rm(&plan, force, on_progress))
}

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
        lines.push(format!(
            "Would remove forest {:?}",
            result.forest_name.as_str()
        ));
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
        for error in &result.errors {
            lines.push(format!("  {}", format_error_single_line(error)));
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

    if result.forest_dir_removed {
        lines.push("Forest directory removed.".to_string());
    } else {
        lines.push("Forest directory not removed (not empty).".to_string());
    }

    if !result.errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in &result.errors {
            lines.push(format!("  {}", format_error_single_line(error)));
        }
    }

    lines.join("\n")
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

// --- rm --all ---

struct RmAllPlan {
    forest_plans: Vec<(PathBuf, RmPlan)>,
}

fn plan_rm_all(worktree_bases: &[&Path]) -> Result<RmAllPlan> {
    let mut forests = Vec::new();

    for base in worktree_bases {
        forests.extend(discover_forests_with_dirs(base)?);
    }
    let forest_plans = dedupe_discovered_forests(forests)
        .into_iter()
        .map(|forest| {
            let plan = plan_rm(&forest.dir, &forest.meta);
            (forest.dir, plan)
        })
        .collect();

    Ok(RmAllPlan { forest_plans })
}

fn execute_rm_all(
    all_plan: &RmAllPlan,
    force: bool,
    on_progress: Option<&dyn Fn(RmAllProgress)>,
) -> RmAllResult {
    let mut results = Vec::new();
    let total = all_plan.forest_plans.len();

    for (_forest_dir, plan) in &all_plan.forest_plans {
        if let Some(cb) = &on_progress {
            cb(RmAllProgress::ForestStarting {
                name: &plan.forest_name,
            });
        }

        let result = execute_rm(plan, force, None);

        if let Some(cb) = &on_progress {
            cb(RmAllProgress::ForestDone(&plan.forest_name, &result));
        }

        results.push(result);
    }

    let succeeded = results.iter().filter(|r| r.errors.is_empty()).count();
    let failed = total - succeeded;

    RmAllResult {
        dry_run: false,
        force,
        results,
        total_forests: total,
        succeeded,
        failed,
    }
}

pub fn cmd_rm_all(
    worktree_bases: &[&Path],
    force: bool,
    dry_run: bool,
    on_progress: Option<&dyn Fn(RmAllProgress)>,
) -> Result<RmAllResult> {
    let all_plan = plan_rm_all(worktree_bases)?;

    if all_plan.forest_plans.is_empty() {
        bail!("no forests found\n  hint: run `git forest ls` to verify");
    }

    if dry_run {
        let results: Vec<RmResult> = all_plan
            .forest_plans
            .iter()
            .map(|(_, plan)| plan_to_dry_run_result(plan, force))
            .collect();
        let total = results.len();
        let succeeded = results.iter().filter(|r| r.errors.is_empty()).count();
        return Ok(RmAllResult {
            dry_run: true,
            force,
            total_forests: total,
            succeeded,
            failed: total - succeeded,
            results,
        });
    }

    Ok(execute_rm_all(&all_plan, force, on_progress))
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

    let all_errors: Vec<&String> = result.results.iter().flat_map(|r| &r.errors).collect();
    if !all_errors.is_empty() {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in all_errors {
            lines.push(format!("  {}", format_error_single_line(error)));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::super::branch_state::ActualBranchState;
    use super::*;
    use crate::commands::cmd_ls;
    use crate::commands::cmd_new;
    use crate::commands::NewInputs;
    use crate::config::{ResolvedRepo, ResolvedTemplate};
    use crate::meta::ForestMode;
    use crate::paths::{AbsolutePath, ForestName, RepoName};
    use crate::testutil::{make_meta, make_repo, TestEnv};

    fn make_new_inputs(name: &str, mode: ForestMode) -> NewInputs {
        NewInputs {
            name: name.to_string(),
            mode,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        }
    }

    fn switch_forest_worktree_to_main(forest_dir: &Path, meta: &ForestMeta) {
        let source = &meta.repos[0].source;
        crate::git::git(source, &["checkout", "-b", "source-other"]).unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["checkout", "main"]).unwrap();
    }

    fn newline_worktree_template(env: &TestEnv, repo_names: &[&str]) -> ResolvedTemplate {
        let repos = repo_names
            .iter()
            .map(|name| ResolvedRepo {
                path: env.repo_path(name),
                name: RepoName::new(name.to_string()).unwrap(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            })
            .collect();

        ResolvedTemplate {
            worktree_base: env.worktree_base().join("newline\nbase"),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos,
        }
    }

    // --- Plan tests ---

    #[test]
    fn plan_rm_basic() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("plan-basic", ForestMode::Feature);
        let result = cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = result.forest_dir;
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let plan = plan_rm(&forest_dir, &meta);

        assert_eq!(plan.forest_name.as_str(), "plan-basic");
        assert_eq!(*plan.forest_dir, *forest_dir);
        assert_eq!(plan.repo_plans.len(), 2);
        assert_eq!(plan.repo_plans[0].name.as_str(), "foo-api");
        assert_eq!(plan.repo_plans[1].name.as_str(), "foo-web");
    }

    #[test]
    fn plan_rm_detects_worktree_exists() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("wt-exists", ForestMode::Feature);
        let result = cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = result.forest_dir;
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let plan = plan_rm(&forest_dir, &meta);

        assert!(plan.repo_plans[0].worktree_exists);
    }

    #[test]
    fn plan_rm_detects_worktree_missing() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("wt-missing", ForestMode::Feature);
        let result = cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = result.forest_dir;
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Manually delete the worktree directory
        std::fs::remove_dir_all(forest_dir.join("foo-api")).unwrap();

        let plan = plan_rm(&forest_dir, &meta);
        assert!(!plan.repo_plans[0].worktree_exists);
    }

    #[test]
    fn plan_rm_records_branch_created() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        // Review mode with exception: foo-web gets an existing branch (branch_created=false)
        let repo = env.repo_path("foo-web");
        crate::git::git(&repo, &["branch", "sue/fix-dialog"]).unwrap();

        let mut inputs = make_new_inputs("branch-created", ForestMode::Review);
        inputs.repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];
        let result = cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = result.forest_dir;
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let plan = plan_rm(&forest_dir, &meta);

        assert!(plan.repo_plans[0].branch_created); // foo-api: new branch
        assert!(!plan.repo_plans[1].branch_created); // foo-web: existing branch
    }

    // --- Execute tests ---

    #[test]
    fn rm_removes_worktrees() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("rm-wt", ForestMode::Feature);
        let new_result = cmd_new(inputs, &tmpl).unwrap();
        let forest_dir = new_result.forest_dir.clone();

        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join("foo-web").exists());

        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(!forest_dir.join("foo-api").exists());
        assert!(!forest_dir.join("foo-web").exists());
        assert!(rm_result.errors.is_empty());
    }

    #[test]
    fn rm_deletes_created_branches() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-branch", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-branch");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Verify branch exists before rm
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        assert!(crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        // Branch should be gone
        assert!(!crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Success
        ));
    }

    #[test]
    fn rm_skips_uncreated_branches() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        // Create a local branch in foo-web so it's ExistingLocal (branch_created=false)
        let web_repo = env.repo_path("foo-web");
        crate::git::git(&web_repo, &["branch", "sue/fix-dialog"]).unwrap();

        let mut inputs = make_new_inputs("rm-skip-branch", ForestMode::Review);
        inputs.repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-skip-branch");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        // foo-api branch_created=true → Success
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        // foo-web branch_created=false → Skipped
        assert!(matches!(
            rm_result.repos[1].branch_deleted,
            RmOutcome::Skipped { .. }
        ));
        // The branch should still exist
        assert!(crate::git::ref_exists(&web_repo, "refs/heads/sue/fix-dialog").unwrap());
    }

    #[test]
    fn rm_removes_forest_dir() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-dir", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dir");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        assert!(forest_dir.exists());
        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(!forest_dir.exists());
        assert!(rm_result.forest_dir_removed);
    }

    #[test]
    fn rm_removes_meta_file() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-meta", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-meta");
        let meta_path = forest_dir.join(META_FILENAME);
        assert!(meta_path.exists());

        let meta = ForestMeta::read(&meta_path).unwrap();
        cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(!meta_path.exists());
    }

    #[test]
    fn rm_dirty_repo_blocks_all_removal() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("rm-dirty-block", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dirty-block");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make foo-api dirty so preflight blocks removal
        let dirty_file = forest_dir.join("foo-api").join("dirty.txt");
        std::fs::write(&dirty_file, "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        // foo-api should fail (dirty)
        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        // foo-web should be skipped (blocked by dirty repos), not attempted
        assert!(matches!(
            rm_result.repos[1].worktree_removed,
            RmOutcome::Skipped { .. }
        ));
        assert!(!rm_result.errors.is_empty());
        // Meta file should still exist — forest remains discoverable
        assert!(forest_dir.join(META_FILENAME).exists());
        // Forest directory should still exist
        assert!(forest_dir.exists());
        // Both worktrees should still exist (nothing was touched)
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join("foo-web").exists());
        assert!(!rm_result.forest_dir_removed);
    }

    #[test]
    fn rm_force_removes_dirty_worktree() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-force-dirty", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-force-dirty");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make it dirty
        let dirty_file = forest_dir.join("foo-api").join("dirty.txt");
        std::fs::write(&dirty_file, "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(!forest_dir.join("foo-api").exists());
    }

    #[test]
    fn rm_force_deletes_unmerged_branch() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-force-unmerged", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-force-unmerged");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Add a commit to the worktree branch to make it unmerged
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        std::fs::write(wt_dir.join("new-file.txt"), "content").unwrap();
        crate::git::git(&wt_dir, &["add", "new-file.txt"]).unwrap();
        crate::git::git(&wt_dir, &["commit", "-m", "unmerged commit"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        // With --force, branch should be deleted even if unmerged
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        assert!(!crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
    }

    #[test]
    fn rm_missing_worktree_skips_gracefully() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-missing-wt", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-missing-wt");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Prune the worktree from git's perspective and delete the directory
        let source = &meta.repos[0].source;
        std::fs::remove_dir_all(forest_dir.join("foo-api")).unwrap();
        crate::git::git(source, &["worktree", "prune"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Skipped { .. }
        ));
        assert_eq!(
            rm_result.repos[0].branch_state.expected_branch,
            "testuser/rm-missing-wt"
        );
        assert!(!rm_result.repos[0].branch_state.branch_drift);
        let json = serde_json::to_value(&rm_result).unwrap();
        assert_eq!(
            json["repos"][0]["branch_state"]["actual_type"],
            "missing_worktree"
        );
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], false);
        // Branch should still be deleted since worktree was "successfully" handled
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        assert!(rm_result.errors.is_empty());
    }

    #[test]
    fn rm_source_repo_missing_present_worktree_requires_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-src-missing", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-src-missing");
        let mut meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Point source to a nonexistent path
        meta.repos[0].source = AbsolutePath::new(PathBuf::from("/nonexistent/repo")).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(rm_result
            .errors
            .iter()
            .any(|error| error.contains("source repo missing")));
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(!rm_result.forest_dir_removed);

        let forced = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            forced.errors.is_empty(),
            "unexpected errors: {:?}",
            forced.errors
        );
        assert!(matches!(
            forced.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            forced.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "source repo missing"
        ));
        assert!(forced.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    fn rm_source_repo_missing_unknown_worktree_blocks_without_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-src-missing-unknown", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-src-missing-unknown");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = meta.repos[0].source.clone();
        std::fs::remove_dir_all(source.as_ref()).unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("source repo missing")));
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(!result.forest_dir_removed);

        let retry = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();
        assert!(
            retry.errors.is_empty(),
            "unexpected errors: {:?}",
            retry.errors
        );
        assert!(matches!(
            retry.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            retry.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "source repo missing"
        ));
        assert!(retry.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    // --- cmd_rm / format tests ---

    #[test]
    fn cmd_rm_dry_run_no_changes() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-dry", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dry");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(rm_result.dry_run);
        // Everything should still exist
        assert!(forest_dir.exists());
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
    }

    #[test]
    fn cmd_rm_dry_run_reports_extra_forest_files_prevent_non_force_directory_removal() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-dry-extra-file", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dry-extra-file");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        std::fs::write(forest_dir.join("notes.txt"), "keep me").unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(rm_result.dry_run);
        assert!(!rm_result.forest_dir_removed);
        assert!(rm_result
            .errors
            .iter()
            .any(|error| error.contains("would still contain notes.txt")));
        assert!(forest_dir.join("notes.txt").exists());
        let human = format_rm_human(&rm_result);
        assert!(human.contains("Would not remove forest directory"));
        assert!(human.contains("notes.txt"));
    }

    #[test]
    fn cmd_rm_preserves_meta_when_extra_forest_files_prevent_non_force_directory_removal() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-extra-file", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-extra-file");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        std::fs::write(forest_dir.join("notes.txt"), "keep me").unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(!result.errors.is_empty());
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("would still contain notes.txt")));
        assert!(!result.forest_dir_removed);
        assert!(forest_dir.exists());
        assert!(forest_dir.join("notes.txt").exists());
        assert!(
            forest_dir.join(META_FILENAME).exists(),
            "failed non-force cleanup must leave metadata discoverable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_dry_run_reports_symlink_forest_dir_cleanup_failure() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-linked-forest-dry-run", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-linked-forest-dry-run");
        let target_parent = tmpl
            .worktree_base
            .as_ref()
            .parent()
            .unwrap()
            .join("linked-targets");
        let target_dir = target_parent.join("rm-linked-forest-dry-run");
        std::fs::create_dir_all(&target_parent).unwrap();
        std::fs::rename(&forest_dir, &target_dir).unwrap();
        std::os::unix::fs::symlink(&target_dir, &forest_dir).unwrap();
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(result.dry_run);
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(!result.forest_dir_removed);
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("forest directory not removed")
                && error.contains("is a symlink")));
        assert!(forest_dir.symlink_metadata().is_ok());
        assert!(target_dir.join(META_FILENAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_preserves_meta_when_symlink_forest_dir_blocks_non_force_cleanup() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-linked-forest", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-linked-forest");
        let target_parent = tmpl
            .worktree_base
            .as_ref()
            .parent()
            .unwrap()
            .join("linked-targets");
        let target_dir = target_parent.join("rm-linked-forest");
        std::fs::create_dir_all(&target_parent).unwrap();
        std::fs::rename(&forest_dir, &target_dir).unwrap();
        std::os::unix::fs::symlink(&target_dir, &forest_dir).unwrap();
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(!result.forest_dir_removed);
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("forest directory not removed")
                && error.contains("is a symlink")));
        assert!(forest_dir.symlink_metadata().is_ok());
        assert!(
            target_dir.join(META_FILENAME).exists(),
            "failed non-force cleanup must not delete target metadata through a symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_dry_run_reports_dangling_symlink_prevents_non_force_directory_removal() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-dry-dangling-symlink", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dry-dangling-symlink");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        crate::git::git(source, &["worktree", "prune", "--expire", "now"]).unwrap();
        std::os::unix::fs::symlink("/missing/target", &worktree).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(rm_result.dry_run);
        assert!(!rm_result.forest_dir_removed);
        assert!(rm_result
            .errors
            .iter()
            .any(|error| error.contains("branch lookup failed")));
        let human = format_rm_human(&rm_result);
        assert!(human.contains("Would not remove forest directory"));
        assert!(human.contains("branch lookup failed"));
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_blocks_dangling_symlink_before_branch_or_meta_removal() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-dangling-symlink", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dangling-symlink");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        crate::git::git(source, &["worktree", "prune", "--expire", "now"]).unwrap();
        std::os::unix::fs::symlink("/missing/target", &worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("branch lookup failed")));
        assert!(worktree.symlink_metadata().is_ok());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_blocks_repo_symlink_without_following_target() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-symlink-non-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-symlink-non-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        let worktree_arg = worktree.to_string_lossy().into_owned();
        crate::git::git(source, &["worktree", "remove", "--force", &worktree_arg]).unwrap();
        std::os::unix::fs::symlink(source.as_ref(), &worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("worktree path is a symlink")));
        assert!(worktree.symlink_metadata().is_ok());
        assert!(source.exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_force_dry_run_reports_repo_symlink_removable() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-symlink-force-dry-run", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-symlink-force-dry-run");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let worktree = forest_dir.join("foo-api");
        let worktree_arg = worktree.to_string_lossy().into_owned();
        crate::git::git(source, &["worktree", "remove", "--force", &worktree_arg]).unwrap();
        std::os::unix::fs::symlink(source.as_ref(), &worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(worktree.symlink_metadata().is_ok());
        assert!(source.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_force_dry_run_refuses_locked_repo_symlink_metadata() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-locked-symlink-force-dry-run", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-locked-symlink-force-dry-run");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        crate::git::git(source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();
        std::fs::remove_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(source.as_ref(), &worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();

        assert!(result.dry_run);
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("metadata is locked")));
        assert!(worktree.symlink_metadata().is_ok());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_force_refuses_locked_repo_symlink_metadata() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-locked-symlink-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-locked-symlink-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        crate::git::git(source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();
        std::fs::remove_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(source.as_ref(), &worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("metadata is locked")));
        assert!(worktree.symlink_metadata().is_ok());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[cfg(unix)]
    #[test]
    fn cmd_rm_force_removes_repo_symlink_without_following_target() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-symlink-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-symlink-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        let worktree_arg = worktree.to_string_lossy().into_owned();
        crate::git::git(source, &["worktree", "remove", "--force", &worktree_arg]).unwrap();
        std::os::unix::fs::symlink(source.as_ref(), &worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(source.exists());
        assert!(!crate::git::ref_exists(source, &branch_ref).unwrap());
    }

    #[test]
    fn cmd_rm_dry_run_reports_unmerged_branch_failure_without_mutating() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-dry-unmerged", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dry-unmerged");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        let branch_ref = format!("refs/heads/{}", branch);

        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&wt_dir, "local.txt", "local only", "local commit");

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(dry_run.dry_run);
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(
            matches!(dry_run.repos[0].branch_deleted, RmOutcome::Failed { .. }),
            "expected dry-run branch deletion failure, got: {:?}",
            dry_run.repos[0].branch_deleted
        );
        assert!(!dry_run.errors.is_empty());
        assert!(dry_run.errors[0].contains("not fully merged"));
        assert!(dry_run.errors[0].contains("dry-run could not prove"));
        assert!(!dry_run.forest_dir_removed);

        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());

        let actual = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(
            matches!(actual.repos[0].branch_deleted, RmOutcome::Failed { .. }),
            "expected actual branch deletion failure, got: {:?}",
            actual.repos[0].branch_deleted
        );
        assert!(!actual.errors.is_empty());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
    }

    #[test]
    fn cmd_rm_dry_run_reports_branch_metadata_drift_without_blocking_recovery() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-drift", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-drift");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        let branch_ref = format!("refs/heads/{}", branch);
        switch_forest_worktree_to_main(&forest_dir, &meta);

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(dry_run.dry_run);
        assert!(dry_run.errors.is_empty());
        assert_eq!(
            dry_run.repos[0].branch_state.expected_branch,
            branch.as_str()
        );
        assert!(dry_run.repos[0].branch_state.branch_drift);
        assert!(matches!(
            &dry_run.repos[0].branch_state.actual,
            ActualBranchState::Branch { actual_branch } if actual_branch == "main"
        ));
        let json = serde_json::to_value(&dry_run).unwrap();
        assert_eq!(
            json["repos"][0]["branch_state"]["expected_branch"],
            "testuser/rm-drift"
        );
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "branch");
        assert_eq!(json["repos"][0]["branch_state"]["actual_branch"], "main");
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], true);
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            dry_run.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        assert!(forest_dir.join("foo-api").exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());

        let human = format_rm_human(&dry_run);
        assert!(human.contains("branch drift"));
        assert!(human.contains("expected testuser/rm-drift"));
        assert!(human.contains("actual main"));

        let actual = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(actual.errors.is_empty());
        assert!(actual.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(!crate::git::ref_exists(source, &branch_ref).unwrap());
    }

    #[test]
    fn cmd_rm_dry_run_reports_branch_checked_out_in_source_repo() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-branch-checked-out", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-branch-checked-out");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        let worktree = forest_dir.join("foo-api");
        crate::git::git(source, &["checkout", "-b", "source-other"]).unwrap();
        crate::git::git(&worktree, &["checkout", "main"]).unwrap();
        crate::git::git(source, &["checkout", branch]).unwrap();

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(dry_run.repos[0].branch_state.branch_drift);
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            dry_run.repos[0].branch_deleted,
            RmOutcome::Failed { .. }
        ));
        assert!(dry_run
            .errors
            .iter()
            .any(|error| error.contains("checked out")));
        assert!(!dry_run.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_reports_reachable_detached_head_drift_without_blocking_rm() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-detached", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-detached");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let worktree = forest_dir.join("foo-api");
        let head = crate::git::git(&worktree, &["rev-parse", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(dry_run.errors.is_empty());
        assert!(dry_run.repos[0].branch_state.branch_drift);
        assert!(matches!(
            &dry_run.repos[0].branch_state.actual,
            ActualBranchState::Detached {
                actual_detached_head
            } if actual_detached_head == &head
        ));
        let json = serde_json::to_value(&dry_run).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "detached");
        assert_eq!(
            json["repos"][0]["branch_state"]["actual_detached_head"],
            head
        );
        assert_eq!(json["repos"][0]["branch_state"]["branch_drift"], true);
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            dry_run.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        assert!(forest_dir.join("foo-api").exists());

        let human = format_rm_human(&dry_run);
        assert!(human.contains("branch drift"));
        assert!(human.contains("detached HEAD"));

        let actual = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(actual.errors.is_empty());
        assert!(actual.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    fn cmd_rm_blocks_unique_detached_head_without_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-detached-unique", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-detached-unique");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        let branch_ref = format!("refs/heads/{}", branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&worktree, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &worktree,
            "detached.txt",
            "unique detached work",
            "detached commit",
        );
        let head = crate::git::git(&worktree, &["rev-parse", "HEAD"]).unwrap();

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            dry_run.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(!dry_run.errors.is_empty());
        assert!(dry_run.errors[0].contains("detached HEAD"));
        assert!(dry_run.errors[0].contains(&head));
        assert!(dry_run.errors[0].contains("not reachable"));
        assert!(!dry_run.forest_dir_removed);
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());

        let actual = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(matches!(
            actual.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            actual.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(!actual.errors.is_empty());
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!actual.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_removes_unique_detached_head() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-detached-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-detached-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        let branch_ref = format!("refs/heads/{}", branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&worktree, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &worktree,
            "detached.txt",
            "unique detached work",
            "detached commit",
        );

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(!crate::git::ref_exists(source, &branch_ref).unwrap());
    }

    #[test]
    fn cmd_rm_blocks_detached_head_when_source_missing_without_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-detached-missing-source", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-detached-missing-source");
        let mut meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let real_source = meta.repos[0].source.clone();
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(&worktree, &["checkout", "--detach", "HEAD"]).unwrap();
        crate::git::git(&worktree, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&worktree, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &worktree,
            "detached.txt",
            "unique detached work",
            "detached commit",
        );
        meta.repos[0].source = AbsolutePath::new(
            tmpl.worktree_base
                .as_ref()
                .join("missing-source")
                .join("foo-api"),
        )
        .unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("source repo missing")));
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(&real_source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);

        let retry = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();
        assert!(
            retry.errors.is_empty(),
            "unexpected errors: {:?}",
            retry.errors
        );
        assert!(matches!(
            retry.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            retry.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "source repo missing"
        ));
        assert!(retry.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(crate::git::ref_exists(&real_source, &branch_ref).unwrap());
    }

    #[test]
    fn cmd_rm_force_removes_source_missing_file_worktree_entry() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-src-missing-file", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-src-missing-file");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = meta.repos[0].source.clone();
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(source.as_ref()).unwrap();
        replace_worktree_with_plain_file(&worktree);

        let dry_run = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();
        assert!(
            dry_run.errors.is_empty(),
            "dry-run errors: {:?}",
            dry_run.errors
        );
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            dry_run.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "source repo missing"
        ));

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "source repo missing"
        ));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    fn cmd_rm_dry_run_reports_branch_lookup_failure_in_human_output() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("rm-lookup-error");
        let repo_dir = forest_dir.join("not-git");
        std::fs::create_dir_all(&repo_dir).unwrap();

        let mut repo = make_repo("not-git", "dliv/rm-lookup-error");
        repo.source = AbsolutePath::new(tmp.path().join("missing-source").join("not-git")).unwrap();
        let meta = make_meta(
            "rm-lookup-error",
            chrono::Utc::now(),
            ForestMode::Feature,
            vec![repo],
        );

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(!dry_run.repos[0].branch_state.branch_drift);
        assert!(matches!(
            &dry_run.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        let json = serde_json::to_value(&dry_run).unwrap();
        assert_eq!(json["repos"][0]["branch_state"]["actual_type"], "unknown");
        assert!(json["repos"][0]["branch_state"]["branch_lookup_error"]
            .as_str()
            .unwrap()
            .contains("git rev-parse --show-toplevel failed"));

        let human = format_rm_human(&dry_run);
        assert!(human.contains("branch lookup failed"));
        assert!(human.contains("not-git"));

        let repo_done = format_repo_done(&dry_run.repos[0]);
        assert!(repo_done.contains("branch lookup failed"));
    }

    #[test]
    fn cmd_rm_non_force_blocks_present_non_git_worktree() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-corrupt-non-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-corrupt-non-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = meta.repos[0].source.clone();
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        replace_worktree_with_plain_dir(&forest_dir.join("foo-api"));

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("branch lookup failed")));
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(&source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_recovers_present_non_git_worktree() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-corrupt-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-corrupt-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        replace_worktree_with_plain_dir(&forest_dir.join("foo-api"));

        let first = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(matches!(
            first.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(forest_dir.join("foo-api").exists());

        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(!crate::git::ref_exists(source, &branch_ref).unwrap());
        let worktrees = crate::git::git(source, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(
            !worktrees.contains("rm-corrupt-force"),
            "stale worktree metadata should be pruned: {}",
            worktrees
        );
    }

    #[test]
    fn cmd_rm_force_refuses_locked_corrupt_worktree_metadata() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-corrupt-locked", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-corrupt-locked");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();
        replace_worktree_with_plain_dir(&worktree);

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("metadata is locked")));
        let human = format_rm_human(&result);
        assert!(
            !human
                .lines()
                .any(|line| line.trim_start().starts_with("stderr:")),
            "rm human output should keep git diagnostics indented and single-line: {}",
            human
        );
        assert!(worktree.exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_dry_run_reports_locked_corrupt_worktree_metadata_failure() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-corrupt-locked-dry-run", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-corrupt-locked-dry-run");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();
        replace_worktree_with_plain_dir(&worktree);

        let result = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();

        assert!(result.dry_run);
        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("metadata is locked")));
        assert!(worktree.exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_dry_run_matches_actual_with_newline_worktree_base() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = newline_worktree_template(&env, &["foo-api"]);

        let inputs = make_new_inputs("rm-newline-base", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-newline-base");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(
            dry_run.errors.is_empty(),
            "dry-run should parse newline paths in worktree metadata: {:?}",
            dry_run.errors
        );
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            dry_run.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        assert!(dry_run.forest_dir_removed);
        assert!(forest_dir.exists());

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "actual rm should still succeed: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    fn cmd_rm_force_refuses_locked_corrupt_worktree_metadata_with_newline_base() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = newline_worktree_template(&env, &["foo-api"]);

        let inputs = make_new_inputs("rm-newline-locked-corrupt", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-newline-locked-corrupt");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();
        replace_worktree_with_plain_dir(&worktree);

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("metadata is locked")));
        assert!(worktree.exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_dry_run_reports_locked_valid_worktree_metadata_failure() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-locked-valid-dry-run", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-locked-valid-dry-run");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");

        crate::git::git(source, &["worktree", "lock", worktree.to_str().unwrap()]).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();

        assert!(result.dry_run);
        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Branch { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("metadata is locked")));
        assert!(worktree.exists());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_dry_run_reports_invalid_source_before_corrupt_direct_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("rm-invalid-source-dry-run");
        let repo_dir = forest_dir.join("not-git");
        let source_dir = tmp.path().join("plain-source");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(repo_dir.join("README.txt"), "not a git worktree").unwrap();

        let mut repo = make_repo("not-git", "dliv/rm-invalid-source-dry-run");
        repo.source = AbsolutePath::new(source_dir).unwrap();
        let meta = make_meta(
            "rm-invalid-source-dry-run",
            chrono::Utc::now(),
            ForestMode::Feature,
            vec![repo],
        );

        let result = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();

        assert!(result.dry_run);
        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("could not inspect git worktree metadata")));
        assert!(repo_dir.exists());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_refuses_invalid_source_before_corrupt_direct_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("rm-invalid-source");
        let repo_dir = forest_dir.join("not-git");
        let source_dir = tmp.path().join("plain-source");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(repo_dir.join("README.txt"), "not a git worktree").unwrap();

        let mut repo = make_repo("not-git", "dliv/rm-invalid-source");
        repo.source = AbsolutePath::new(source_dir).unwrap();
        let meta = make_meta(
            "rm-invalid-source",
            chrono::Utc::now(),
            ForestMode::Feature,
            vec![repo],
        );

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("could not inspect git worktree metadata")));
        assert!(repo_dir.exists());
        assert!(forest_dir.exists());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_refuses_invalid_source_for_missing_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("rm-invalid-source-missing");
        let source_dir = tmp.path().join("plain-source");
        std::fs::create_dir_all(&forest_dir).unwrap();
        std::fs::create_dir_all(&source_dir).unwrap();

        let mut repo = make_repo("missing-repo", "dliv/rm-invalid-source-missing");
        repo.source = AbsolutePath::new(source_dir).unwrap();
        let meta = make_meta(
            "rm-invalid-source-missing",
            chrono::Utc::now(),
            ForestMode::Feature,
            vec![repo],
        );
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("not a git repository")));
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_dry_run_reports_stale_metadata_for_missing_worktree() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-stale-missing-dry-run", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-stale-missing-dry-run");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        assert!(meta.repos[0].branch_created);
        std::fs::remove_dir_all(&worktree).unwrap();
        let worktrees = crate::git::git(source, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(
            worktrees.contains(&format!("branch {}", branch_ref)),
            "expected stale metadata for {} in:\n{}",
            branch_ref,
            worktrees
        );

        let result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(result.dry_run);
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("still lists missing worktree metadata")));
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_prunes_stale_metadata_for_missing_worktree() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-stale-missing-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-stale-missing-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Skipped { ref reason } if reason == "worktree already missing"
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(!crate::git::ref_exists(source, &branch_ref).unwrap());
        let worktrees = crate::git::git(source, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(
            !worktrees.contains("rm-stale-missing-force"),
            "stale worktree metadata should be pruned: {}",
            worktrees
        );
    }

    #[test]
    fn cmd_rm_dry_run_reports_checked_out_source_branch_after_missing_worktree_pruned() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-missing-checked-out", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-missing-checked-out");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();
        crate::git::git(source, &["worktree", "prune", "--expire", "now"]).unwrap();
        crate::git::git(source, &["checkout", branch]).unwrap();

        let result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Skipped { ref reason } if reason == "worktree already missing"
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Failed { .. }
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("checked out")));
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_reports_stale_missing_worktree_metadata_for_uncreated_branch() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-stale-uncreated", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-stale-uncreated");
        let mut meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        meta.repos[0].branch_created = false;
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        std::fs::remove_dir_all(&worktree).unwrap();

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(dry_run
            .errors
            .iter()
            .any(|error| error.contains("still lists missing worktree metadata")));

        let result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            result.repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "worktree not removed"
        ));
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("still lists missing worktree metadata")));
        assert!(crate::git::ref_exists(source, &branch_ref).unwrap());
        assert!(forest_dir.join(META_FILENAME).exists());
        assert!(!result.forest_dir_removed);
    }

    #[test]
    fn cmd_rm_force_recovers_file_at_corrupt_worktree_path() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-corrupt-file-force", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-corrupt-file-force");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch_ref = format!("refs/heads/{}", meta.repos[0].branch);
        let worktree = forest_dir.join("foo-api");
        replace_worktree_with_plain_file(&worktree);

        let dry_run = cmd_rm(&forest_dir, &meta, true, true, None).unwrap();
        assert!(
            dry_run.errors.is_empty(),
            "dry-run errors: {:?}",
            dry_run.errors
        );
        assert!(matches!(
            dry_run.repos[0].worktree_removed,
            RmOutcome::Success
        ));

        let result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            result.errors.is_empty(),
            "unexpected errors: {:?}",
            result.errors
        );
        assert!(matches!(
            result.repos[0].branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));
        assert!(matches!(
            result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(result.repos[0].branch_deleted, RmOutcome::Success));
        assert!(result.forest_dir_removed);
        assert!(!forest_dir.exists());
        assert!(!crate::git::ref_exists(source, &branch_ref).unwrap());
    }

    #[test]
    fn cmd_rm_reports_branch_lookup_failure_in_dirty_preflight_progress() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("rm-dirty-lookup", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-dirty-lookup");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        std::fs::write(forest_dir.join("foo-api").join("dirty.txt"), "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let lookup_error_repo = forest_dir.join("foo-web");
        std::fs::remove_dir_all(&lookup_error_repo).unwrap();
        std::fs::create_dir_all(&lookup_error_repo).unwrap();

        let progress_lines = std::cell::RefCell::new(Vec::new());
        let result = cmd_rm(
            &forest_dir,
            &meta,
            false,
            false,
            Some(&|progress| match progress {
                RmProgress::RepoStarting { name } => {
                    progress_lines
                        .borrow_mut()
                        .push(format!("{}: starting", name));
                }
                RmProgress::RepoDone(repo) => {
                    progress_lines.borrow_mut().push(format!(
                        "{}: {}",
                        repo.name,
                        format_repo_done(repo)
                    ));
                }
            }),
        )
        .unwrap();

        assert!(!result.errors.is_empty());
        let lookup_error_result = result
            .repos
            .iter()
            .find(|repo| repo.name.as_str() == "foo-web")
            .unwrap();
        assert!(matches!(
            &lookup_error_result.branch_state.actual,
            ActualBranchState::Unknown { .. }
        ));

        let progress_lines = progress_lines.borrow();
        assert!(
            progress_lines
                .iter()
                .any(|line| line.contains("foo-web") && line.contains("branch lookup failed")),
            "progress lines should include foo-web branch lookup failure: {:?}",
            *progress_lines
        );
    }

    #[test]
    fn cmd_rm_returns_result() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-result", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-result");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert_eq!(rm_result.forest_name.as_str(), "rm-result");
        assert!(!rm_result.dry_run);
        assert!(!rm_result.force);
        assert!(rm_result.forest_dir_removed);
        assert!(rm_result.errors.is_empty());
        assert_eq!(rm_result.repos.len(), 1);
    }

    #[test]
    fn format_rm_human_success() {
        let result = RmResult {
            forest_name: ForestName::new("test-forest".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/worktrees/test-forest"),
            dry_run: false,
            force: false,
            repos: vec![
                RepoRmResult {
                    name: RepoName::new("foo-api".to_string()).unwrap(),
                    branch_state: WorktreeBranchState::missing_worktree("dliv/test-forest"),
                    worktree_removed: RmOutcome::Success,
                    branch_deleted: RmOutcome::Success,
                },
                RepoRmResult {
                    name: RepoName::new("foo-web".to_string()).unwrap(),
                    branch_state: WorktreeBranchState::missing_worktree("dliv/test-forest"),
                    worktree_removed: RmOutcome::Success,
                    branch_deleted: RmOutcome::Skipped {
                        reason: "branch not created by forest".to_string(),
                    },
                },
            ],
            forest_dir_removed: true,
            errors: vec![],
        };

        let output = format_rm_human(&result);
        assert!(output.contains("Removed forest"));
        assert!(output.contains("foo-api"));
        assert!(output.contains("worktree removed"));
        assert!(output.contains("branch deleted"));
        assert!(output.contains("branch not ours"));
        assert!(output.contains("Forest directory removed"));
    }

    #[test]
    fn format_rm_human_dry_run() {
        let result = RmResult {
            forest_name: ForestName::new("test-forest".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/worktrees/test-forest"),
            dry_run: true,
            force: false,
            repos: vec![RepoRmResult {
                name: RepoName::new("foo-api".to_string()).unwrap(),
                branch_state: WorktreeBranchState::missing_worktree("dliv/test-forest"),
                worktree_removed: RmOutcome::Success,
                branch_deleted: RmOutcome::Success,
            }],
            forest_dir_removed: true,
            errors: vec![],
        };

        let output = format_rm_human(&result);
        assert!(output.contains("Dry run"));
        assert!(output.contains("Would remove forest"));
        assert!(output.contains("remove worktree"));
        assert!(output.contains("delete branch"));
    }

    #[test]
    fn format_rm_human_with_errors() {
        let result = RmResult {
            forest_name: ForestName::new("test-forest".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/worktrees/test-forest"),
            dry_run: false,
            force: false,
            repos: vec![RepoRmResult {
                name: RepoName::new("foo-api".to_string()).unwrap(),
                branch_state: WorktreeBranchState::missing_worktree("dliv/test-forest"),
                worktree_removed: RmOutcome::Failed {
                    error: "git worktree remove failed".to_string(),
                },
                branch_deleted: RmOutcome::Skipped {
                    reason: "worktree still exists, cannot delete branch".to_string(),
                },
            }],
            forest_dir_removed: false,
            errors: vec!["foo-api: git worktree remove failed".to_string()],
        };

        let output = format_rm_human(&result);
        assert!(output.contains("with errors"));
        assert!(output.contains("worktree FAILED"));
        assert!(output.contains("Errors:"));
    }

    // --- Round-trip test ---

    #[test]
    fn new_then_rm_then_ls_empty() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        // Create
        let inputs = make_new_inputs("roundtrip", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let ls1 = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls1.forests.len(), 1);

        // Remove
        let forest_dir = tmpl.worktree_base.join("roundtrip");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(rm_result.errors.is_empty());

        // ls should show empty
        let ls2 = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls2.forests.len(), 0);
    }

    // --- Branch deletion fallback tests ---

    /// Helper: commit a file in a git directory.
    fn commit_file(dir: &std::path::Path, filename: &str, content: &str, message: &str) {
        std::fs::write(dir.join(filename), content).unwrap();
        crate::git::git(dir, &["add", filename]).unwrap();
        crate::git::git(dir, &["commit", "-m", message]).unwrap();
    }

    fn replace_worktree_with_plain_dir(worktree: &std::path::Path) {
        std::fs::remove_dir_all(worktree).unwrap();
        std::fs::create_dir_all(worktree).unwrap();
        std::fs::write(worktree.join("README.txt"), "not a git worktree").unwrap();
    }

    fn replace_worktree_with_plain_file(worktree: &std::path::Path) {
        std::fs::remove_dir_all(worktree).unwrap();
        std::fs::write(worktree, "not a directory").unwrap();
    }

    #[test]
    fn rm_fully_pushed_branch_succeeds_via_upstream_check() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-pushed", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-pushed");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        // Add a commit to the feature branch and push it
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&wt_dir, "feature.txt", "feature work", "feat: add feature");
        crate::git::git(&wt_dir, &["push", "-u", "origin", branch]).unwrap();

        // `git branch -d` will fail because the commit isn't merged into HEAD.
        // But the upstream check should detect no unpushed commits, making deletion safe.
        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Success),
            "expected Success, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(!crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
        assert!(rm_result.errors.is_empty());
    }

    #[test]
    fn rm_merged_into_base_succeeds_via_ancestor_check() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-ancestor", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-ancestor");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        // Add a commit on the feature branch
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&wt_dir, "feature.txt", "feature work", "feat: add feature");

        // Merge the feature branch into main in the source repo (real merge, not squash).
        // After this, branch is ancestor of main, but `-d` will fail because HEAD in
        // the source repo is on main and the worktree branch check uses a different HEAD.
        crate::git::git(source, &["merge", branch]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Success),
            "expected Success via ancestor check, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(!crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
        assert!(rm_result.errors.is_empty());
    }

    #[test]
    fn rm_stale_local_base_succeeds_via_remote_tracking_base_check() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-stale-base", ForestMode::Review);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-stale-base");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        // Add a commit on the forest-created branch.
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &wt_dir,
            "review.txt",
            "review work",
            "feat: add review work",
        );

        // Advance origin/main to include the forest branch, but leave local main stale.
        let push_ref = format!("refs/heads/{}:refs/heads/main", branch);
        crate::git::git(&wt_dir, &["push", "origin", &push_ref]).unwrap();
        crate::git::git(source, &["fetch", "origin", "main"]).unwrap();

        assert!(
            crate::git::git(source, &["merge-base", "--is-ancestor", branch, "main"]).is_err(),
            "local main should remain stale"
        );
        assert!(
            crate::git::git(
                source,
                &["merge-base", "--is-ancestor", branch, "origin/main"]
            )
            .is_ok(),
            "origin/main should contain the forest branch"
        );

        let dry_run = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(
            matches!(dry_run.repos[0].branch_deleted, RmOutcome::Success),
            "expected dry-run Success via remote-tracking base check, got: {:?}",
            dry_run.repos[0].branch_deleted
        );
        assert!(dry_run.errors.is_empty());
        assert!(dry_run.forest_dir_removed);
        assert!(forest_dir.join("foo-api").exists());
        assert!(crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Success),
            "expected Success via remote-tracking base check, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(!crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
        assert!(rm_result.errors.is_empty());
        assert!(rm_result.forest_dir_removed);
    }

    #[test]
    fn rm_stale_local_base_succeeds_via_recorded_non_origin_remote() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let source = env.repo_path("foo-api");
        crate::git::git(&source, &["remote", "rename", "origin", "upstream"]).unwrap();
        crate::git::git(&source, &["fetch", "upstream", "main"]).unwrap();

        let mut tmpl = env.default_template(&["foo-api"]);
        tmpl.repos[0].remote = "upstream".to_string();

        let inputs = make_new_inputs("rm-upstream-base", ForestMode::Review);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-upstream-base");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let branch = &meta.repos[0].branch;

        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &wt_dir,
            "review.txt",
            "review work",
            "feat: add review work",
        );

        let push_ref = format!("refs/heads/{}:refs/heads/main", branch);
        crate::git::git(&wt_dir, &["push", "upstream", &push_ref]).unwrap();
        crate::git::git(&source, &["fetch", "upstream", "main"]).unwrap();

        assert!(
            crate::git::git(&source, &["merge-base", "--is-ancestor", branch, "main"]).is_err(),
            "local main should remain stale"
        );
        assert!(
            crate::git::git(
                &source,
                &["merge-base", "--is-ancestor", branch, "upstream/main"]
            )
            .is_ok(),
            "upstream/main should contain the forest branch"
        );

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Success),
            "expected Success via recorded non-origin remote, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(!crate::git::ref_exists(&source, &format!("refs/heads/{}", branch)).unwrap());
        assert!(rm_result.errors.is_empty());
    }

    #[test]
    fn rm_does_not_use_unrecorded_origin_when_base_remote_differs() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let source = env.repo_path("foo-api");
        crate::git::git(&source, &["remote", "rename", "origin", "upstream"]).unwrap();
        crate::git::git(&source, &["fetch", "upstream", "main"]).unwrap();

        let origin_dir = tempfile::tempdir().unwrap();
        let origin_path = origin_dir.path().join("origin.git");
        std::fs::create_dir_all(&origin_path).unwrap();
        crate::git::git(&origin_path, &["init", "--bare", "-b", "main"]).unwrap();
        crate::git::git(
            &source,
            &["remote", "add", "origin", origin_path.to_str().unwrap()],
        )
        .unwrap();

        let mut tmpl = env.default_template(&["foo-api"]);
        tmpl.repos[0].remote = "upstream".to_string();

        let inputs = make_new_inputs("rm-wrong-origin", ForestMode::Review);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-wrong-origin");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let branch = &meta.repos[0].branch;

        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &wt_dir,
            "review.txt",
            "review work",
            "feat: add review work",
        );

        let push_ref = format!("refs/heads/{}:refs/heads/main", branch);
        crate::git::git(&wt_dir, &["push", "origin", &push_ref]).unwrap();
        crate::git::git(&source, &["fetch", "origin", "main"]).unwrap();

        assert!(
            crate::git::git(
                &source,
                &["merge-base", "--is-ancestor", branch, "origin/main"]
            )
            .is_ok(),
            "origin/main should contain the forest branch"
        );
        assert!(
            crate::git::git(
                &source,
                &["merge-base", "--is-ancestor", branch, "upstream/main"]
            )
            .is_err(),
            "recorded upstream/main should not contain the forest branch"
        );

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Failed { .. }),
            "expected Failed because recorded base remote lacks the branch, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(crate::git::ref_exists(&source, &format!("refs/heads/{}", branch)).unwrap());
    }

    #[test]
    fn rm_remote_base_check_ignores_ambiguous_local_branch_name() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-ambiguous-origin", ForestMode::Review);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-ambiguous-origin");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(
            &wt_dir,
            "review.txt",
            "review work",
            "feat: add review work",
        );

        crate::git::git(source, &["branch", "origin/main", branch]).unwrap();

        assert!(crate::git::ref_exists(source, "refs/heads/origin/main").unwrap());
        assert!(
            crate::git::git(
                source,
                &[
                    "merge-base",
                    "--is-ancestor",
                    branch,
                    "refs/heads/origin/main"
                ]
            )
            .is_ok(),
            "local origin/main should contain the forest branch"
        );
        assert!(
            crate::git::git(
                source,
                &[
                    "merge-base",
                    "--is-ancestor",
                    branch,
                    "refs/remotes/origin/main"
                ]
            )
            .is_err(),
            "remote-tracking origin/main should not contain the forest branch"
        );

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Failed { .. }),
            "expected Failed because only the ambiguous local branch contains the work, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
    }

    #[test]
    fn rm_unpushed_commits_fails_without_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-unpushed", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-unpushed");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        // Add a commit but don't push — no upstream, no merge into base
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&wt_dir, "local.txt", "local only", "local commit");

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Failed { .. }),
            "expected Failed, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        // Branch should still exist
        assert!(crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
        // Error message should hint about squash-merge and include original git error
        assert!(rm_result.errors[0].contains("squash-merge"));
        assert!(rm_result.errors[0].contains("not fully merged"));
        let human = format_rm_human(&rm_result);
        assert!(human.contains("git forest rm --force"));
    }

    #[test]
    fn rm_no_remote_tracking_fails_without_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-no-remote", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-no-remote");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        // Add a commit but only push without -u (no upstream tracking)
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&wt_dir, "feature.txt", "feature work", "feat: add feature");
        // Push without -u so there's no upstream tracking
        crate::git::git(&wt_dir, &["push", "origin", branch]).unwrap();
        // Add another local commit that isn't pushed
        commit_file(&wt_dir, "local.txt", "local only", "local commit");

        let rm_result = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Failed { .. }),
            "expected Failed, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
    }

    #[test]
    fn rm_force_still_deletes_unmerged_branch_directly() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-force-direct", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-force-direct");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let source = &meta.repos[0].source;
        let branch = &meta.repos[0].branch;

        // Add an unpushed commit — would fail without --force
        let wt_dir = forest_dir.join("foo-api");
        crate::git::git(&wt_dir, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&wt_dir, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&wt_dir, "local.txt", "local only", "local commit");

        let rm_result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        assert!(
            matches!(rm_result.repos[0].branch_deleted, RmOutcome::Success),
            "expected Success with --force, got: {:?}",
            rm_result.repos[0].branch_deleted
        );
        assert!(!crate::git::ref_exists(source, &format!("refs/heads/{}", branch)).unwrap());
    }

    #[test]
    fn rm_force_bypasses_dirty_check() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("rm-force-dirty2", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-force-dirty2");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make foo-api dirty
        let dirty_file = forest_dir.join("foo-api").join("dirty.txt");
        std::fs::write(&dirty_file, "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, true, false, None).unwrap();

        // --force bypasses dirty check, everything removed
        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        assert!(matches!(
            rm_result.repos[1].worktree_removed,
            RmOutcome::Success
        ));
        assert!(rm_result.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    fn rm_retry_after_cleaning_dirty_repo() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-retry", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-retry");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make dirty
        let dirty_file = forest_dir.join("foo-api").join("dirty.txt");
        std::fs::write(&dirty_file, "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        // First attempt fails
        let rm1 = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(!rm1.errors.is_empty());
        assert!(forest_dir.exists());
        assert!(forest_dir.join(META_FILENAME).exists());

        // Clean the dirty state: unstage and remove the file
        crate::git::git(&forest_dir.join("foo-api"), &["reset", "HEAD", "dirty.txt"]).unwrap();
        std::fs::remove_file(&dirty_file).unwrap();

        // Re-read meta (still there) and retry — re-plan to pick up clean state
        let meta2 = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let rm2 = cmd_rm(&forest_dir, &meta2, false, false, None).unwrap();
        assert!(rm2.errors.is_empty());
        assert!(rm2.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    fn plan_rm_detects_dirty_files() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("plan-dirty", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("plan-dirty");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make foo-api dirty
        std::fs::write(forest_dir.join("foo-api").join("dirty.txt"), "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let plan = plan_rm(&forest_dir, &meta);
        assert!(plan.repo_plans[0].has_dirty_files);
        assert!(!plan.repo_plans[1].has_dirty_files);
    }

    #[test]
    fn dry_run_shows_dirty_rejection() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("dry-dirty", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("dry-dirty");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make foo-api dirty
        std::fs::write(forest_dir.join("foo-api").join("dirty.txt"), "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, true, None).unwrap();

        assert!(rm_result.dry_run);
        // Dirty repo should show as failed
        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        // Clean repo should show as skipped (blocked)
        assert!(matches!(
            rm_result.repos[1].worktree_removed,
            RmOutcome::Skipped { .. }
        ));
        assert!(!rm_result.forest_dir_removed);
        assert!(!rm_result.errors.is_empty());
        // Nothing should have been touched
        assert!(forest_dir.exists());
        assert!(forest_dir.join("foo-api").exists());
        assert!(forest_dir.join("foo-web").exists());
    }

    #[test]
    fn rm_idempotent_after_partial_failure() {
        // Reproduces the bug: first rm partially fails (unmerged branch in one repo),
        // second rm with --force should skip already-deleted branches and clean up fully.
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("rm-idempotent", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-idempotent");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        let api_source = &meta.repos[0].source;
        let api_branch = &meta.repos[0].branch;
        let web_source = &meta.repos[1].source;
        let web_branch = &meta.repos[1].branch;

        // Add an unpushed commit to foo-web so `git branch -d` fails ("not fully merged")
        let web_wt = forest_dir.join("foo-web");
        crate::git::git(&web_wt, &["config", "user.name", "Test"]).unwrap();
        crate::git::git(&web_wt, &["config", "user.email", "test@test.com"]).unwrap();
        commit_file(&web_wt, "local.txt", "local only", "local commit");

        // First rm (no --force): foo-api branch deletes, foo-web branch fails
        let rm1 = cmd_rm(&forest_dir, &meta, false, false, None).unwrap();
        assert!(matches!(rm1.repos[0].branch_deleted, RmOutcome::Success));
        assert!(matches!(
            rm1.repos[1].branch_deleted,
            RmOutcome::Failed { .. }
        ));
        assert!(!rm1.errors.is_empty());
        // foo-api branch gone, foo-web branch still exists
        assert!(
            !crate::git::ref_exists(api_source, &format!("refs/heads/{}", api_branch)).unwrap()
        );
        assert!(crate::git::ref_exists(web_source, &format!("refs/heads/{}", web_branch)).unwrap());
        // Forest dir still present (errors blocked cleanup)
        assert!(forest_dir.join(META_FILENAME).exists());

        // Second rm (--force): should skip foo-api (already deleted), force-delete foo-web
        let meta2 = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();
        let rm2 = cmd_rm(&forest_dir, &meta2, true, false, None).unwrap();

        // foo-api branch should be skipped (already deleted), not failed
        assert!(
            matches!(rm2.repos[0].branch_deleted, RmOutcome::Skipped { .. }),
            "expected Skipped for already-deleted branch, got: {:?}",
            rm2.repos[0].branch_deleted
        );
        // foo-web branch should be force-deleted
        assert!(
            matches!(rm2.repos[1].branch_deleted, RmOutcome::Success),
            "expected Success for force-deleted branch, got: {:?}",
            rm2.repos[1].branch_deleted
        );
        // No errors — forest dir should be cleaned up
        assert!(rm2.errors.is_empty(), "unexpected errors: {:?}", rm2.errors);
        assert!(rm2.forest_dir_removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    #[should_panic(expected = "is not inside forest dir")]
    fn execute_rm_panics_on_path_escape() {
        let plan = RmPlan {
            forest_name: ForestName::new("test".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/forests/test"),
            repo_plans: vec![RepoRmPlan {
                name: RepoName::new("evil".to_string()).unwrap(),
                worktree_path: PathBuf::from("/home/user/real-work"),
                source: AbsolutePath::new(PathBuf::from("/tmp/src")).unwrap(),
                branch: "main".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: false,
                branch_state: WorktreeBranchState::missing_worktree("main"),
                detached_head_safety: DetachedHeadSafety::NotDetached,
                worktree_exists: false,
                source_exists: false,
                has_dirty_files: false,
            }],
        };
        execute_rm(&plan, false, None);
    }

    #[test]
    #[should_panic(expected = "is not inside forest dir")]
    fn execute_rm_panics_on_parent_dir_path_escape() {
        let plan = RmPlan {
            forest_name: ForestName::new("test".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/forests/test"),
            repo_plans: vec![RepoRmPlan {
                name: RepoName::new("evil".to_string()).unwrap(),
                worktree_path: PathBuf::from("/tmp/forests/test/../victim"),
                source: AbsolutePath::new(PathBuf::from("/tmp/src")).unwrap(),
                branch: "main".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: false,
                branch_state: WorktreeBranchState::missing_worktree("main"),
                detached_head_safety: DetachedHeadSafety::NotDetached,
                worktree_exists: false,
                source_exists: false,
                has_dirty_files: false,
            }],
        };
        execute_rm(&plan, false, None);
    }

    #[test]
    #[should_panic(expected = "is not inside forest dir")]
    fn execute_rm_validates_path_escape_before_dirty_preflight() {
        let plan = RmPlan {
            forest_name: ForestName::new("test".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/forests/test"),
            repo_plans: vec![RepoRmPlan {
                name: RepoName::new("evil".to_string()).unwrap(),
                worktree_path: PathBuf::from("/tmp/forests/test/../victim"),
                source: AbsolutePath::new(PathBuf::from("/tmp/src")).unwrap(),
                branch: "main".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: false,
                branch_state: WorktreeBranchState::missing_worktree("main"),
                detached_head_safety: DetachedHeadSafety::NotDetached,
                worktree_exists: true,
                source_exists: false,
                has_dirty_files: true,
            }],
        };
        execute_rm(&plan, false, None);
    }

    #[test]
    #[should_panic(expected = "is not inside forest dir")]
    fn dry_run_panics_on_parent_dir_path_escape() {
        let plan = RmPlan {
            forest_name: ForestName::new("test".to_string()).unwrap(),
            forest_dir: PathBuf::from("/tmp/forests/test"),
            repo_plans: vec![RepoRmPlan {
                name: RepoName::new("evil".to_string()).unwrap(),
                worktree_path: PathBuf::from("/tmp/forests/test/../victim"),
                source: AbsolutePath::new(PathBuf::from("/tmp/src")).unwrap(),
                branch: "main".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: false,
                branch_state: WorktreeBranchState::missing_worktree("main"),
                detached_head_safety: DetachedHeadSafety::NotDetached,
                worktree_exists: true,
                source_exists: false,
                has_dirty_files: true,
            }],
        };
        plan_to_dry_run_result(&plan, false);
    }

    #[test]
    fn plan_rm_accepts_canonical_forest_dir_with_parent_component() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("base-parent");
        let worktrees = tmp.path().join("worktrees");
        let forest_dir = parent.join("../worktrees/forest");
        let repo_dir = forest_dir.join("not-git");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir_all(&repo_dir).unwrap();

        let meta = make_meta(
            "forest",
            chrono::Utc::now(),
            ForestMode::Feature,
            vec![make_repo("not-git", "dliv/forest")],
        );

        let plan = plan_rm(&forest_dir, &meta);

        assert_eq!(plan.repo_plans.len(), 1);
        assert!(plan.repo_plans[0].worktree_exists);
        assert!(plan.repo_plans[0]
            .worktree_path
            .canonicalize()
            .unwrap()
            .starts_with(worktrees.canonicalize().unwrap()));
    }

    // --- rm --all tests ---

    #[test]
    fn rm_all_removes_multiple_forests() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        // Create two forests
        let inputs_a = make_new_inputs("forest-a", ForestMode::Feature);
        cmd_new(inputs_a, &tmpl).unwrap();
        let inputs_b = make_new_inputs("forest-b", ForestMode::Feature);
        cmd_new(inputs_b, &tmpl).unwrap();

        // Verify both exist
        let ls = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls.forests.len(), 2);

        // rm --all
        let result = cmd_rm_all(&[tmpl.worktree_base.as_ref()], false, false, None).unwrap();
        assert_eq!(result.total_forests, 2);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 0);
        assert!(!result.dry_run);

        // Verify all gone
        let ls = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls.forests.len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn rm_all_deduplicates_symlinked_worktree_bases() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("rm-all-symlink-base", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();
        let base_link = tmpl
            .worktree_base
            .as_ref()
            .parent()
            .unwrap()
            .join("worktrees-link");
        std::os::unix::fs::symlink(tmpl.worktree_base.as_ref(), &base_link).unwrap();

        let result = cmd_rm_all(
            &[tmpl.worktree_base.as_ref(), base_link.as_path()],
            false,
            false,
            None,
        )
        .unwrap();

        assert_eq!(result.total_forests, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert!(result.results.iter().all(|result| result.errors.is_empty()));
        let ls = cmd_ls(&[tmpl.worktree_base.as_ref(), base_link.as_path()]).unwrap();
        assert_eq!(ls.forests.len(), 0);
    }

    #[test]
    fn rm_all_dry_run_preserves_forests() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("dry-test", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let result = cmd_rm_all(&[tmpl.worktree_base.as_ref()], false, true, None).unwrap();
        assert!(result.dry_run);
        assert_eq!(result.total_forests, 1);

        // Forest should still exist
        let ls = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls.forests.len(), 1);
    }

    #[test]
    fn rm_all_dry_run_summary_reports_failed_previews() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("dry-test-extra-file", ForestMode::Feature);
        let new_result = cmd_new(inputs, &tmpl).unwrap();
        std::fs::write(new_result.forest_dir.join("notes.txt"), "keep me").unwrap();

        let result = cmd_rm_all(&[tmpl.worktree_base.as_ref()], false, true, None).unwrap();
        assert!(result.dry_run);
        assert_eq!(result.total_forests, 1);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);

        let human = format_rm_all_human(&result);
        assert!(human.contains("Would not remove forest directory"));
        assert!(human.contains("Would remove 0/1 forest(s)."));
    }

    #[test]
    fn rm_all_no_forests_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let result = cmd_rm_all(&[tmp.path()], false, false, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no forests found"));
    }

    #[test]
    fn rm_all_with_force() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = env.default_template(&["foo-api"]);

        let inputs = make_new_inputs("force-test", ForestMode::Feature);
        let new_result = cmd_new(inputs, &tmpl).unwrap();

        // Make the worktree dirty
        std::fs::write(new_result.forest_dir.join("foo-api/dirty.txt"), "dirty").unwrap();

        // Without force, should fail
        let result = cmd_rm_all(&[tmpl.worktree_base.as_ref()], false, false, None).unwrap();
        assert_eq!(result.failed, 1);

        // With force, should succeed
        let result = cmd_rm_all(&[tmpl.worktree_base.as_ref()], true, false, None).unwrap();
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);

        let ls = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls.forests.len(), 0);
    }
}
