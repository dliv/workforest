use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

use crate::meta::{ForestMeta, META_FILENAME};
use crate::paths::AbsolutePath;

// --- Types ---

pub struct RmPlan {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub repo_plans: Vec<RepoRmPlan>,
}

pub struct RepoRmPlan {
    pub name: String,
    pub worktree_path: PathBuf,
    pub source: AbsolutePath,
    pub branch: String,
    pub branch_created: bool,
    pub worktree_exists: bool,
    pub source_exists: bool,
}

#[derive(Debug, Serialize)]
pub struct RmResult {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub dry_run: bool,
    pub force: bool,
    pub repos: Vec<RepoRmResult>,
    pub forest_dir_removed: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RepoRmResult {
    pub name: String,
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

// --- Planning (read-only) ---

pub fn plan_rm(forest_dir: &std::path::Path, meta: &ForestMeta) -> RmPlan {
    let repo_plans = meta
        .repos
        .iter()
        .map(|repo| {
            let worktree_path = forest_dir.join(&repo.name);
            RepoRmPlan {
                name: repo.name.clone(),
                worktree_path: worktree_path.clone(),
                source: repo.source.clone(),
                branch: repo.branch.clone(),
                branch_created: repo.branch_created,
                worktree_exists: worktree_path.exists(),
                source_exists: repo.source.is_dir(),
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

pub fn execute_rm(plan: &RmPlan, force: bool) -> RmResult {
    let mut repos = Vec::new();
    let mut errors = Vec::new();

    for repo_plan in &plan.repo_plans {
        let (worktree_removed, wt_succeeded) = remove_worktree(repo_plan, force, &mut errors);

        let branch_deleted = delete_branch(repo_plan, force, wt_succeeded, &mut errors);

        repos.push(RepoRmResult {
            name: repo_plan.name.clone(),
            worktree_removed,
            branch_deleted,
        });
    }

    let forest_dir_removed = remove_forest_dir(&plan.forest_dir, force, &mut errors);

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

    let delete_flag = if force { "-D" } else { "-d" };
    match crate::git::git(
        &repo_plan.source,
        &["branch", delete_flag, &repo_plan.branch],
    ) {
        Ok(_) => RmOutcome::Success,
        Err(e) => {
            let msg = format!(
                "{}: git branch {} failed: {}",
                repo_plan.name, delete_flag, e
            );
            errors.push(msg.clone());
            RmOutcome::Failed { error: msg }
        }
    }
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

fn plan_to_dry_run_result(plan: &RmPlan) -> RmResult {
    let repos = plan
        .repo_plans
        .iter()
        .map(|rp| {
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

    RmResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        dry_run: true,
        force: false,
        repos,
        forest_dir_removed: true,
        errors: vec![],
    }
}

pub fn cmd_rm(
    forest_dir: &std::path::Path,
    meta: &ForestMeta,
    force: bool,
    dry_run: bool,
) -> Result<RmResult> {
    let plan = plan_rm(forest_dir, meta);

    if dry_run {
        return Ok(plan_to_dry_run_result(&plan));
    }

    Ok(execute_rm(&plan, force))
}

pub fn format_rm_human(result: &RmResult) -> String {
    let mut lines = Vec::new();

    if result.dry_run {
        lines.push("Dry run — no changes will be made.".to_string());
        lines.push(String::new());
        lines.push(format!("Would remove forest {:?}", result.forest_name));
    } else if result.errors.is_empty() {
        lines.push(format!("Removed forest {:?}", result.forest_name));
    } else {
        lines.push(format!(
            "Removed forest {:?} (with errors)",
            result.forest_name
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cmd_ls;
    use crate::commands::cmd_new;
    use crate::commands::NewInputs;
    use crate::meta::ForestMode;
    use crate::paths::AbsolutePath;
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

        assert_eq!(plan.forest_name, "plan-basic");
        assert_eq!(*plan.forest_dir, *forest_dir);
        assert_eq!(plan.repo_plans.len(), 2);
        assert_eq!(plan.repo_plans[0].name, "foo-api");
        assert_eq!(plan.repo_plans[1].name, "foo-web");
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
        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

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

        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

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

        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

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
        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

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
        cmd_rm(&forest_dir, &meta, false, false).unwrap();

        assert!(!meta_path.exists());
    }

    #[test]
    fn rm_best_effort_continues_on_failure() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = env.default_template(&["foo-api", "foo-web"]);

        let inputs = make_new_inputs("rm-best-effort", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let forest_dir = tmpl.worktree_base.join("rm-best-effort");
        let meta = ForestMeta::read(&forest_dir.join(META_FILENAME)).unwrap();

        // Make foo-api dirty so worktree remove fails without --force
        let dirty_file = forest_dir.join("foo-api").join("dirty.txt");
        std::fs::write(&dirty_file, "dirty").unwrap();
        crate::git::git(&forest_dir.join("foo-api"), &["add", "dirty.txt"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

        // foo-api should fail (dirty), foo-web should succeed
        assert!(matches!(
            rm_result.repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(matches!(
            rm_result.repos[1].worktree_removed,
            RmOutcome::Success
        ));
        assert!(!rm_result.errors.is_empty());
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

        let rm_result = cmd_rm(&forest_dir, &meta, true, false).unwrap();

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
        std::fs::write(wt_dir.join("new-file.txt"), "content").unwrap();
        crate::git::git(&wt_dir, &["add", "new-file.txt"]).unwrap();
        crate::git::git(&wt_dir, &["commit", "-m", "unmerged commit"]).unwrap();

        let rm_result = cmd_rm(&forest_dir, &meta, true, false).unwrap();

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

        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

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

        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

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

        let rm_result = cmd_rm(&forest_dir, &meta, false, true).unwrap();

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

        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();

        assert_eq!(rm_result.forest_name, "rm-result");
        assert!(!rm_result.dry_run);
        assert!(!rm_result.force);
        assert!(rm_result.forest_dir_removed);
        assert!(rm_result.errors.is_empty());
        assert_eq!(rm_result.repos.len(), 1);
    }

    #[test]
    fn format_rm_human_success() {
        let result = RmResult {
            forest_name: "test-forest".to_string(),
            forest_dir: PathBuf::from("/tmp/worktrees/test-forest"),
            dry_run: false,
            force: false,
            repos: vec![
                RepoRmResult {
                    name: "foo-api".to_string(),
                    worktree_removed: RmOutcome::Success,
                    branch_deleted: RmOutcome::Success,
                },
                RepoRmResult {
                    name: "foo-web".to_string(),
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
            forest_name: "test-forest".to_string(),
            forest_dir: PathBuf::from("/tmp/worktrees/test-forest"),
            dry_run: true,
            force: false,
            repos: vec![RepoRmResult {
                name: "foo-api".to_string(),
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
            forest_name: "test-forest".to_string(),
            forest_dir: PathBuf::from("/tmp/worktrees/test-forest"),
            dry_run: false,
            force: false,
            repos: vec![RepoRmResult {
                name: "foo-api".to_string(),
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
        let rm_result = cmd_rm(&forest_dir, &meta, false, false).unwrap();
        assert!(rm_result.errors.is_empty());

        // ls should show empty
        let ls2 = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls2.forests.len(), 0);
    }
}
