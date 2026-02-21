use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

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
    pub branch_created: bool,
    pub worktree_exists: bool,
    pub source_exists: bool,
    pub has_dirty_files: bool,
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

// --- Planning (read-only) ---

pub fn plan_rm(forest_dir: &std::path::Path, meta: &ForestMeta) -> RmPlan {
    let repo_plans = meta
        .repos
        .iter()
        .map(|repo| {
            let worktree_path = forest_dir.join(repo.name.as_str());
            let worktree_exists = worktree_path.exists();
            let has_dirty_files = worktree_exists
                && crate::git::git(&worktree_path, &["status", "--porcelain"])
                    .map(|output| !output.is_empty())
                    .unwrap_or(false);
            RepoRmPlan {
                name: repo.name.clone(),
                worktree_path: worktree_path.clone(),
                source: repo.source.clone(),
                branch: repo.branch.clone(),
                base_branch: repo.base_branch.clone(),
                branch_created: repo.branch_created,
                worktree_exists,
                source_exists: repo.source.is_dir(),
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

// --- Execution (impure) ---

pub fn execute_rm(
    plan: &RmPlan,
    force: bool,
    on_progress: Option<&dyn Fn(RmProgress)>,
) -> RmResult {
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
                repos.push(RepoRmResult {
                    name: rp.name.clone(),
                    worktree_removed,
                    branch_deleted,
                });
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
        assert!(
            repo_plan.worktree_path.starts_with(&plan.forest_dir),
            "worktree path {:?} is not inside forest dir {:?}",
            repo_plan.worktree_path,
            plan.forest_dir
        );

        if let Some(cb) = &on_progress {
            cb(RmProgress::RepoStarting {
                name: &repo_plan.name,
            });
        }

        let (worktree_removed, wt_succeeded) = remove_worktree(repo_plan, force, &mut errors);

        let branch_deleted = delete_branch(repo_plan, force, wt_succeeded, &mut errors);

        let result = RepoRmResult {
            name: repo_plan.name.clone(),
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

fn remove_worktree(
    repo_plan: &RepoRmPlan,
    force: bool,
    errors: &mut Vec<String>,
) -> (RmOutcome, bool) {
    if !repo_plan.worktree_exists {
        return (
            RmOutcome::Skipped {
                reason: "worktree already missing".to_string(),
            },
            true, // treat as success for branch deletion purposes
        );
    }

    if !repo_plan.source_exists {
        // Source repo is gone — can't use git, remove directory directly
        match std::fs::remove_dir_all(&repo_plan.worktree_path) {
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

    let wt_path_str = repo_plan.worktree_path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&wt_path_str);

    match crate::git::git(&repo_plan.source, &args) {
        Ok(_) => (RmOutcome::Success, true),
        Err(e) => {
            let msg = format!("{}: git worktree remove failed: {}", repo_plan.name, e);
            errors.push(msg.clone());
            (RmOutcome::Failed { error: msg }, false)
        }
    }
}

fn delete_branch(
    repo_plan: &RepoRmPlan,
    force: bool,
    wt_succeeded: bool,
    errors: &mut Vec<String>,
) -> RmOutcome {
    if !repo_plan.branch_created {
        return RmOutcome::Skipped {
            reason: "branch not created by forest".to_string(),
        };
    }

    if !wt_succeeded {
        return RmOutcome::Skipped {
            reason: "worktree still exists, cannot delete branch".to_string(),
        };
    }

    if !repo_plan.source_exists {
        return RmOutcome::Skipped {
            reason: "source repo missing".to_string(),
        };
    }

    if force {
        return match crate::git::git(&repo_plan.source, &["branch", "-D", &repo_plan.branch]) {
            Ok(_) => RmOutcome::Success,
            Err(e) => {
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
            // -d failed — check if the branch was merged via a different mechanism
            // (e.g. fully merged but HEAD isn't on base, or fully pushed to upstream)
            if can_safely_force_delete(&repo_plan.source, &repo_plan.branch, &repo_plan.base_branch)
            {
                match crate::git::git(&repo_plan.source, &["branch", "-D", &repo_plan.branch]) {
                    Ok(_) => RmOutcome::Success,
                    Err(e) => {
                        let msg = format!("{}: git branch -D failed: {}", repo_plan.name, e);
                        errors.push(msg.clone());
                        RmOutcome::Failed { error: msg }
                    }
                }
            } else {
                let msg = format!(
                    "{}: branch {:?} is not fully merged ({})\n  \
                     hint: if the branch was merged (e.g. squash-merge) and the remote \
                     branch was deleted, use `git forest rm --force`",
                    repo_plan.name, repo_plan.branch, original_err,
                );
                errors.push(msg.clone());
                RmOutcome::Failed { error: msg }
            }
        }
    }
}

/// Check whether a branch can be safely force-deleted after `git branch -d` fails.
///
/// Two checks, tried in order:
/// 1. `git merge-base --is-ancestor <branch> <base_branch>` — the branch is fully
///    merged into the base branch (catches cases where `-d` failed due to HEAD position).
/// 2. The branch has an upstream and all local commits are pushed to it
///    (`git rev-list --count <upstream>..<branch>` == 0). This proves no local-only
///    commits exist, making deletion safe even when the remote branch was squash-merged
///    and then deleted.
fn can_safely_force_delete(source: &AbsolutePath, branch: &str, base_branch: &str) -> bool {
    // Fallback 1: branch is ancestor of base_branch (fully merged)
    if crate::git::git(
        source,
        &["merge-base", "--is-ancestor", branch, base_branch],
    )
    .is_ok()
    {
        return true;
    }

    // Fallback 2: branch has upstream tracking and no unpushed commits
    if let Ok(upstream) = crate::git::git(
        source,
        &[
            "for-each-ref",
            "--format=%(upstream:short)",
            &format!("refs/heads/{}", branch),
        ],
    ) {
        if !upstream.is_empty() {
            let range = format!("{}..{}", upstream, branch);
            if let Ok(count) = crate::git::git(source, &["rev-list", "--count", &range]) {
                if count.trim() == "0" {
                    return true;
                }
            }
        }
    }

    false
}

fn remove_forest_dir(forest_dir: &std::path::Path, force: bool, errors: &mut Vec<String>) -> bool {
    if !forest_dir.exists() {
        return true;
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
        // Remove meta file first, then try non-recursive remove_dir
        let meta_path = forest_dir.join(META_FILENAME);
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

// --- Orchestrator ---

fn plan_to_dry_run_result(plan: &RmPlan, force: bool) -> RmResult {
    let has_dirty = !force && plan.repo_plans.iter().any(|rp| rp.has_dirty_files);

    let mut errors = Vec::new();
    let repos = plan
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
                    worktree_removed,
                    branch_deleted,
                };
            }

            let worktree_removed = if !rp.worktree_exists {
                RmOutcome::Skipped {
                    reason: "worktree already missing".to_string(),
                }
            } else {
                RmOutcome::Success
            };

            let branch_deleted = if !rp.branch_created {
                RmOutcome::Skipped {
                    reason: "branch not created by forest".to_string(),
                }
            } else if !rp.worktree_exists {
                // worktree missing is treated as success, so branch delete would proceed
                RmOutcome::Success
            } else {
                RmOutcome::Success
            };

            RepoRmResult {
                name: rp.name.clone(),
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

    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run: true,
        force,
        repos,
        forest_dir_removed: !has_dirty,
        errors,
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

        lines.push(format!("  {}: {}{}", repo.name, wt, br));
    }

    if result.dry_run {
        lines.push("  Would remove forest directory".to_string());
    } else if result.forest_dir_removed {
        lines.push("Forest directory removed.".to_string());
    } else {
        lines.push("Forest directory not removed (not empty).".to_string());
    }

    if !result.errors.is_empty() && !result.dry_run {
        lines.push(String::new());
        lines.push("Errors:".to_string());
        for error in &result.errors {
            lines.push(format!("  {}", error));
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

    format!("{}{}", wt, br)
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
            lines.push(format!("  {}", error));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cmd_ls;
    use crate::commands::cmd_new;
    use crate::commands::NewInputs;
    use crate::meta::ForestMode;
    use crate::paths::{AbsolutePath, ForestName, RepoName};
    use crate::testutil::TestEnv;

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
        // Branch should still be deleted since worktree was "successfully" handled
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Success
        ));
        assert!(rm_result.errors.is_empty());
    }

    #[test]
    fn rm_source_repo_missing_handles_gracefully() {
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

        // Worktree should still be removed (via direct fs removal since source is missing)
        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Success
        ));
        // Branch should be skipped since source is missing
        assert!(matches!(
            rm_result.repos[0].branch_deleted,
            RmOutcome::Skipped { .. }
        ));
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
                    worktree_removed: RmOutcome::Success,
                    branch_deleted: RmOutcome::Success,
                },
                RepoRmResult {
                    name: RepoName::new("foo-web".to_string()).unwrap(),
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
                branch_created: false,
                worktree_exists: false,
                source_exists: false,
                has_dirty_files: false,
            }],
        };
        execute_rm(&plan, false, None);
    }
}
