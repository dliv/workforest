use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use super::branch_state::ActualBranchState;
use super::rm::{self, RepoRmPlan, RmOutcome};
use crate::forest::{dedupe_discovered_forests, discover_forests_with_dirs};
use crate::paths::RepoName;

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
    pub errors: Vec<String>,
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
}

#[derive(Debug, Serialize)]
pub struct RepoResetEntry {
    pub name: RepoName,
    pub branch: String,
    pub branch_created: bool,
    pub worktree_removed: RmOutcome,
    pub branch_deleted: RmOutcome,
}

pub enum ResetProgress<'a> {
    ForestStarting { name: &'a str, path: &'a Path },
    ForestDone(&'a ForestResetEntry),
}

// --- Planning (read-only) ---

struct ForestInfo {
    name: String,
    path: PathBuf,
    repos: Vec<RepoRmPlan>,
}

struct ResetPlan {
    config_path: PathBuf,
    config_exists: bool,
    state_path: PathBuf,
    state_exists: bool,
    config_only: bool,
    forests: Vec<ForestInfo>,
    warnings: Vec<String>,
}

fn plan_reset(config_only: bool) -> Result<ResetPlan> {
    let config_path = crate::config::default_config_path()?;
    let state_path = crate::config::xdg_state_dir()?.join("state.toml");

    let config_exists = config_path.exists();
    let state_exists = state_path.exists();

    let mut forests = Vec::new();
    let mut warnings = Vec::new();

    if !config_only && config_exists {
        match crate::config::load_config(&config_path) {
            Ok(config) => {
                let bases = config.all_worktree_bases();
                let mut discovered_forests = Vec::new();
                for base in bases {
                    match discover_forests_with_dirs(base) {
                        Ok(mut discovered) => {
                            discovered_forests.append(&mut discovered);
                        }
                        Err(e) => {
                            warnings.push(format!(
                                "could not scan worktree base {}: {}",
                                base.display(),
                                e
                            ));
                        }
                    }
                }
                for discovered in dedupe_discovered_forests(discovered_forests) {
                    let forest_path = discovered.dir;
                    let meta = discovered.meta;
                    let name = meta.name.to_string();
                    let rm_plan = rm::plan_rm(&forest_path, &meta);
                    forests.push(ForestInfo {
                        name,
                        path: forest_path,
                        repos: rm_plan.repo_plans,
                    });
                }
            }
            Err(e) => {
                warnings.push(format!(
                    "config exists but could not be parsed: {}\n  worktree directories may need manual cleanup",
                    e
                ));
            }
        }
    }

    if !config_exists && !state_exists && forests.is_empty() {
        bail!("nothing to reset\n  hint: no config, state, or forests found");
    }

    Ok(ResetPlan {
        config_path,
        config_exists,
        state_path,
        state_exists,
        config_only,
        forests,
        warnings,
    })
}

// --- Execution (impure) ---

fn execute_reset(plan: &ResetPlan, on_progress: Option<&dyn Fn(ResetProgress)>) -> ResetResult {
    let mut errors = Vec::new();
    let mut forest_entries = Vec::new();

    // Remove forests first (while config still exists for reference)
    for forest in &plan.forests {
        let (name, path) = (&forest.name, &forest.path);
        if let Some(cb) = &on_progress {
            cb(ResetProgress::ForestStarting { name, path });
        }

        // Unregister git worktrees before deleting the directory.
        // If repo cleanup fails, keep the forest metadata so users can retry
        // with `git forest rm --force` or inspect the remaining state.
        let mut repos = Vec::new();
        let forest_error_start = errors.len();
        for repo in &forest.repos {
            assert!(
                repo.worktree_path.starts_with(&forest.path),
                "worktree dir {:?} is not inside forest dir {:?}",
                repo.worktree_path,
                forest.path
            );
            let (worktree_removed, wt_succeeded) = remove_reset_worktree(repo, &mut errors);
            let branch_deleted = rm::delete_branch(repo, false, wt_succeeded, &mut errors);
            repos.push(repo_reset_entry(repo, worktree_removed, branch_deleted));
        }

        let entry = if errors.len() > forest_error_start || !path.exists() {
            ForestResetEntry {
                name: name.clone(),
                path: path.clone(),
                removed: false,
                repos,
            }
        } else {
            match std::fs::remove_dir_all(path) {
                Ok(()) => ForestResetEntry {
                    name: name.clone(),
                    path: path.clone(),
                    removed: true,
                    repos,
                },
                Err(e) => {
                    errors.push(format!("failed to remove forest {}: {}", name, e));
                    ForestResetEntry {
                        name: name.clone(),
                        path: path.clone(),
                        removed: false,
                        repos,
                    }
                }
            }
        };

        if let Some(cb) = &on_progress {
            cb(ResetProgress::ForestDone(&entry));
        }
        forest_entries.push(entry);
    }

    let cleanup_succeeded = errors.is_empty();

    let (config_deleted, config_backed_up_to) = if cleanup_succeeded && plan.config_exists {
        backup_file(&plan.config_path, &mut errors)
    } else {
        (false, None)
    };

    let state_deleted = if cleanup_succeeded && plan.state_exists {
        delete_file(&plan.state_path, &mut errors)
    } else {
        false
    };

    ResetResult {
        dry_run: false,
        confirm_required: false,
        config_only: plan.config_only,
        config_file: FileResetEntry {
            path: plan.config_path.clone(),
            existed: plan.config_exists,
            deleted: config_deleted,
            backed_up_to: config_backed_up_to,
        },
        state_file: FileResetEntry {
            path: plan.state_path.clone(),
            existed: plan.state_exists,
            deleted: state_deleted,
            backed_up_to: None,
        },
        forests: forest_entries,
        warnings: plan.warnings.clone(),
        errors,
    }
}

fn remove_reset_worktree(repo: &RepoRmPlan, errors: &mut Vec<String>) -> (RmOutcome, bool) {
    if !repo.worktree_exists {
        return remove_missing_reset_worktree(repo, errors);
    }

    let wt_str = repo.worktree_path.to_string_lossy();
    match crate::git::git(&repo.source, &["worktree", "remove", "--force", &wt_str]) {
        Ok(_) => (RmOutcome::Success, true),
        Err(_e) if repo.worktree_path.symlink_metadata().is_err() => {
            remove_missing_reset_worktree(repo, errors)
        }
        Err(e) => {
            let msg = format!("warning: git worktree remove failed for {}: {}", wt_str, e);
            errors.push(msg.clone());
            (RmOutcome::Failed { error: msg }, false)
        }
    }
}

fn remove_missing_reset_worktree(repo: &RepoRmPlan, errors: &mut Vec<String>) -> (RmOutcome, bool) {
    if let Some(msg) = rm::stale_missing_worktree_metadata_error(repo, true) {
        errors.push(msg.clone());
        return (RmOutcome::Failed { error: msg }, false);
    }

    (
        RmOutcome::Skipped {
            reason: "worktree already missing".to_string(),
        },
        true,
    )
}

fn plan_reset_worktree_outcome(repo: &RepoRmPlan, errors: &mut Vec<String>) -> (RmOutcome, bool) {
    if !repo.worktree_exists {
        if repo.source_exists {
            if let Some(msg) = rm::worktree_metadata_dry_run_error(repo, true) {
                errors.push(msg.clone());
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
        errors.push(msg.clone());
        return (RmOutcome::Failed { error: msg }, false);
    }

    if let Some(msg) = rm::worktree_metadata_dry_run_error(repo, true) {
        errors.push(msg.clone());
        return (RmOutcome::Failed { error: msg }, false);
    }

    if matches!(repo.branch_state.actual, ActualBranchState::Unknown { .. }) {
        let msg = format!(
            "{}: branch lookup failed; dry-run cannot prove worktree removal",
            repo.name
        );
        errors.push(msg.clone());
        return (RmOutcome::Failed { error: msg }, false);
    }

    (RmOutcome::Success, true)
}

fn repo_reset_entry(
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

fn delete_file(path: &Path, errors: &mut Vec<String>) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("failed to delete {}: {}", path.display(), e));
            false
        }
    }
}

/// Rename a file to `<name>.<timestamp>.bak` instead of deleting it.
/// Returns `(removed_from_original_path, backup_path)`.
fn backup_file(path: &Path, errors: &mut Vec<String>) -> (bool, Option<PathBuf>) {
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let file_stem = path.file_name().unwrap_or_default().to_string_lossy();
    let backup_name = format!("{}.{}.bak", file_stem, timestamp);
    let backup_path = path.with_file_name(&backup_name);

    match std::fs::rename(path, &backup_path) {
        Ok(()) => (true, Some(backup_path)),
        Err(e) => {
            errors.push(format!(
                "failed to back up {} to {}: {}",
                path.display(),
                backup_path.display(),
                e
            ));
            (false, None)
        }
    }
}

// --- Orchestrator ---

fn plan_to_dry_run(plan: &ResetPlan) -> ResetResult {
    let mut errors = Vec::new();
    let forests: Vec<ForestResetEntry> = plan
        .forests
        .iter()
        .map(|f| {
            let forest_error_start = errors.len();
            let repos = f
                .repos
                .iter()
                .map(|repo| {
                    let (worktree_removed, wt_succeeded) =
                        plan_reset_worktree_outcome(repo, &mut errors);
                    let branch_deleted =
                        rm::plan_branch_delete_outcome(repo, false, wt_succeeded, &mut errors);
                    repo_reset_entry(repo, worktree_removed, branch_deleted)
                })
                .collect();
            ForestResetEntry {
                name: f.name.clone(),
                path: f.path.clone(),
                removed: f.path.exists() && errors.len() == forest_error_start,
                repos,
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

fn plan_to_confirm_required(plan: &ResetPlan) -> ResetResult {
    let mut result = plan_to_dry_run(plan);
    result.dry_run = false;
    result.confirm_required = result.errors.is_empty();
    result
}

pub fn cmd_reset(
    confirm: bool,
    config_only: bool,
    dry_run: bool,
    on_progress: Option<&dyn Fn(ResetProgress)>,
) -> Result<ResetResult> {
    let plan = plan_reset(config_only)?;

    if dry_run {
        return Ok(plan_to_dry_run(&plan));
    }

    if !confirm {
        return Ok(plan_to_confirm_required(&plan));
    }

    Ok(execute_reset(&plan, on_progress))
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
                    } else if forest.path.exists() {
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
        for error in &result.errors {
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
        for error in &result.errors {
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
    result.errors.iter().any(|error| !file_cleanup_error(error))
        && ((result.config_file.existed
            && !result.config_file.deleted
            && result.config_file.backed_up_to.is_none())
            || (result.state_file.existed && !result.state_file.deleted)
            || result
                .forests
                .iter()
                .any(|forest| !forest.removed && forest.path.exists()))
}

fn file_cleanup_error(error: &str) -> bool {
    error.starts_with("failed to back up ") || error.starts_with("failed to delete ")
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
    use crate::channel;
    use serial_test::serial;
    use std::path::PathBuf;

    /// RAII guard that sets XDG env vars for the test and restores them on drop.
    struct XdgEnvGuard {
        saved_config: Option<String>,
        saved_state: Option<String>,
    }

    impl XdgEnvGuard {
        fn set(config: impl AsRef<std::path::Path>, state: impl AsRef<std::path::Path>) -> Self {
            let saved_config = std::env::var("XDG_CONFIG_HOME").ok();
            let saved_state = std::env::var("XDG_STATE_HOME").ok();
            std::env::set_var("XDG_CONFIG_HOME", config.as_ref());
            std::env::set_var("XDG_STATE_HOME", state.as_ref());
            Self {
                saved_config,
                saved_state,
            }
        }
    }

    impl Drop for XdgEnvGuard {
        fn drop(&mut self) {
            match &self.saved_config {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match &self.saved_state {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }

    fn write_reset_config(
        tmp_root: &std::path::Path,
        worktree_base: &std::path::Path,
        repo_path: &std::path::Path,
        repo_name: &str,
    ) {
        let config_dir = tmp_root.join("config").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "testuser/{{name}}"

[[template.default.repos]]
path = "{}"
name = "{}"
"#,
            worktree_base.display(),
            repo_path.display(),
            repo_name,
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();
    }

    #[test]
    fn format_reset_human_confirm_required() {
        let result = ResetResult {
            dry_run: false,
            confirm_required: true,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config/git-forest/config.toml"),
                existed: true,
                deleted: true,
                backed_up_to: Some(PathBuf::from("/tmp/config/git-forest/config.toml.bak")),
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state/git-forest/state.toml"),
                existed: true,
                deleted: true,
                backed_up_to: None,
            },
            forests: vec![ForestResetEntry {
                name: "my-feature".to_string(),
                path: PathBuf::from("/tmp/worktrees/my-feature"),
                removed: true,
                repos: vec![],
            }],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("would be deleted"));
        assert!(output.contains("would remove"));
        assert!(output.contains("would back up"), "output: {}", output);
        assert!(output.contains("would delete")); // state file
        assert!(output.contains("Pass --confirm to proceed"));
    }

    #[test]
    fn format_reset_human_dry_run() {
        let result = ResetResult {
            dry_run: true,
            confirm_required: false,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config.toml"),
                existed: true,
                deleted: true,
                backed_up_to: Some(PathBuf::from("/tmp/config.toml.bak")),
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: false,
                deleted: false,
                backed_up_to: None,
            },
            forests: vec![],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Dry run"));
        assert!(output.contains("would back up"), "output: {}", output);
        assert!(output.contains("not found")); // state file
    }

    #[test]
    fn format_reset_human_success() {
        let result = ResetResult {
            dry_run: false,
            confirm_required: false,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config.toml"),
                existed: true,
                deleted: true,
                backed_up_to: Some(PathBuf::from("/tmp/config.toml.20260101.bak")),
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: true,
                deleted: true,
                backed_up_to: None,
            },
            forests: vec![ForestResetEntry {
                name: "test".to_string(),
                path: PathBuf::from("/tmp/worktrees/test"),
                removed: true,
                repos: vec![],
            }],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Reset complete"));
        assert!(output.contains("removed"));
        assert!(
            output.contains("backed up to"),
            "config should be backed up: {}",
            output
        );
        assert!(output.contains("deleted")); // state file
    }

    #[test]
    fn format_reset_human_with_errors() {
        let result = ResetResult {
            dry_run: false,
            confirm_required: false,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config.toml"),
                existed: true,
                deleted: false,
                backed_up_to: None,
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: true,
                deleted: true,
                backed_up_to: None,
            },
            forests: vec![],
            warnings: vec![],
            errors: vec!["failed to delete config".to_string()],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("with errors"));
        assert!(output.contains("Errors:"));
    }

    #[test]
    fn format_reset_human_config_only() {
        let result = ResetResult {
            dry_run: false,
            confirm_required: false,
            config_only: true,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config.toml"),
                existed: true,
                deleted: true,
                backed_up_to: Some(PathBuf::from("/tmp/config.toml.20260101.bak")),
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: true,
                deleted: true,
                backed_up_to: None,
            },
            forests: vec![],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Reset complete"));
        assert!(!output.contains("Forests:"));
    }

    #[test]
    fn format_reset_human_with_warnings() {
        let result = ResetResult {
            dry_run: false,
            confirm_required: false,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config.toml"),
                existed: true,
                deleted: true,
                backed_up_to: None,
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: false,
                deleted: false,
                backed_up_to: None,
            },
            forests: vec![],
            warnings: vec!["config could not be parsed".to_string()],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Warnings:"));
        assert!(output.contains("config could not be parsed"));
    }

    #[test]
    fn format_reset_human_reports_repo_branch_cleanup() {
        let result = ResetResult {
            dry_run: true,
            confirm_required: false,
            config_only: false,
            config_file: FileResetEntry {
                path: PathBuf::from("/tmp/config.toml"),
                existed: true,
                deleted: true,
                backed_up_to: None,
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: false,
                deleted: false,
                backed_up_to: None,
            },
            forests: vec![ForestResetEntry {
                name: "reset-repro".to_string(),
                path: PathBuf::from("/tmp/worktrees/reset-repro"),
                removed: true,
                repos: vec![RepoResetEntry {
                    name: RepoName::new("repo-a".to_string()).unwrap(),
                    branch: "forest/reset-repro".to_string(),
                    branch_created: true,
                    worktree_removed: RmOutcome::Success,
                    branch_deleted: RmOutcome::Success,
                }],
            }],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output
            .contains("repo-a: would remove worktree, would delete branch forest/reset-repro"));
    }

    // --- Integration tests ---
    // These tests mutate XDG env vars (process-global), so they must run serially.

    #[test]
    #[serial]
    fn reset_nothing_to_do() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, false, false, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nothing to reset"));
    }

    #[test]
    #[serial]
    fn reset_deletes_config_and_state() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let state_dir = tmp.path().join("state").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();

        let config_content = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/nonexistent-worktrees"
base_branch = "main"
feature_branch_template = "test/{name}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#;
        std::fs::write(config_dir.join("config.toml"), config_content).unwrap();
        std::fs::write(state_dir.join("state.toml"), "").unwrap();

        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, true, false, None).unwrap();

        assert!(!result.dry_run);
        assert!(result.config_file.deleted);
        assert!(result.config_file.backed_up_to.is_some());
        assert!(result.config_file.backed_up_to.as_ref().unwrap().exists());
        assert!(result.state_file.deleted);
        assert!(!config_dir.join("config.toml").exists());
        assert!(!state_dir.join("state.toml").exists());
    }

    #[test]
    #[serial]
    fn reset_confirm_required_without_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();

        let config_content = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/nonexistent-worktrees"
base_branch = "main"
feature_branch_template = "test/{name}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#;
        std::fs::write(config_dir.join("config.toml"), config_content).unwrap();

        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(false, false, false, None).unwrap();
        assert!(result.confirm_required);
        assert!(config_dir.join("config.toml").exists());
    }

    #[test]
    #[serial]
    fn reset_dry_run_no_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();

        let config_content = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/nonexistent-worktrees"
base_branch = "main"
feature_branch_template = "test/{name}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#;
        std::fs::write(config_dir.join("config.toml"), config_content).unwrap();

        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(false, false, true, None).unwrap();
        assert!(result.dry_run);
        assert!(config_dir.join("config.toml").exists());
    }

    #[test]
    #[serial]
    fn reset_removes_forests() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("my-feature");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("my-feature".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            repos: vec![],
        };
        meta.write(&forest_dir.join(crate::meta::META_FILENAME))
            .unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "test/{{name}}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#,
            worktree_base.display()
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();

        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();

        assert!(!result.dry_run);
        assert_eq!(result.forests.len(), 1);
        assert!(result.forests[0].removed);
        assert!(!forest_dir.exists());
        assert!(result.errors.is_empty());
    }

    #[test]
    #[serial]
    fn reset_uses_discovered_forest_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("actual-dir");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("metadata-name".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            repos: vec![],
        };
        meta.write(&forest_dir.join(crate::meta::META_FILENAME))
            .unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "test/{{name}}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#,
            worktree_base.display()
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();

        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.forests[0].path, forest_dir);
        assert!(result.forests[0].removed);
        assert!(!forest_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_deduplicates_symlinked_worktree_bases() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        let worktree_base_alias = tmp.path().join("worktrees-alias");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();
        std::os::unix::fs::symlink(&worktree_base, &worktree_base_alias).unwrap();

        let forest_dir = worktree_base.join("dedupe-me");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("dedupe-me".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            repos: vec![],
        };
        meta.write(&forest_dir.join(crate::meta::META_FILENAME))
            .unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "test/{{name}}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"

[template.alias]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "test/{{name}}"

[[template.alias.repos]]
path = "/tmp/nonexistent-repo"
"#,
            worktree_base.display(),
            worktree_base_alias.display()
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();

        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.forests.len(), 1);
        assert_eq!(result.forests[0].path, forest_dir);
        assert!(result.forests[0].removed);
        assert!(!forest_dir.exists());
    }

    #[test]
    #[serial]
    fn reset_config_only_leaves_forests() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("my-feature");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("my-feature".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            repos: vec![],
        };
        meta.write(&forest_dir.join(crate::meta::META_FILENAME))
            .unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "test/{{name}}"

[[template.default.repos]]
path = "/tmp/nonexistent-repo"
"#,
            worktree_base.display()
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();

        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, true, false, None).unwrap();

        assert!(result.config_only);
        assert!(result.forests.is_empty());
        assert!(result.config_file.deleted);
        assert!(forest_dir.exists());
    }

    #[test]
    #[serial]
    fn reset_unparseable_config_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();

        std::fs::write(config_dir.join("config.toml"), "this is not valid toml [[[").unwrap();

        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();

        assert!(!result.warnings.is_empty());
        assert!(result.warnings[0].contains("could not be parsed"));
        assert!(result.config_file.deleted);
        assert!(result.config_file.backed_up_to.is_some());
        assert!(!config_dir.join("config.toml").exists());
    }

    // --- Bug reproduction tests ---
    // These tests document bugs where reset leaves stale git worktree
    // registrations, causing `git forest new` to fail on re-creation.

    /// After reset, git worktree registrations should be cleaned up so that
    /// creating a new forest with the same repo reuses the branch name cleanly.
    #[test]
    #[serial]
    fn reset_cleans_up_git_worktree_registrations() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo_a = env.create_repo_with_remote("repo-a");
        let tmpl = env.default_template(&["repo-a"]);

        // 1. Create a forest (registers worktrees in repo-a's .git/worktrees/)
        let inputs = NewInputs {
            name: "wt-cleanup-test".to_string(),
            mode: ForestMode::Feature,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        };
        let result = cmd_new(inputs, &tmpl).unwrap();
        assert!(!result.dry_run);

        // Verify the worktree is registered in git
        let wt_list = crate::git::git(&repo_a, &["worktree", "list"]).unwrap();
        assert!(
            wt_list.contains("wt-cleanup-test"),
            "worktree should be registered after new: {}",
            wt_list
        );

        // 2. Write config so reset can find the forest
        let base = env.worktree_base();
        let tmp_root = base.parent().unwrap();
        let config_dir = tmp_root.join("config").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "testuser/{{name}}"

[[template.default.repos]]
path = "{}"
name = "repo-a"
"#,
            base.display(),
            repo_a.display(),
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();

        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        // 3. Reset — should delete forest directory AND clean up worktree registrations
        let reset_result = cmd_reset(true, false, false, None).unwrap();
        assert_eq!(reset_result.forests.len(), 1);
        assert!(reset_result.forests[0].removed);

        // 4. Verify git no longer has a stale worktree registration
        let wt_list_after = crate::git::git(&repo_a, &["worktree", "list"]).unwrap();
        assert!(
            !wt_list_after.contains("wt-cleanup-test"),
            "stale worktree registration should be cleaned up after reset, but got: {}",
            wt_list_after
        );
    }

    #[test]
    #[serial]
    fn reset_deletes_forest_created_branches() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-reset-branch");
        let tmpl = env.default_template(&["repo-reset-branch"]);

        cmd_new(
            NewInputs {
                name: "reset-repro".to_string(),
                mode: ForestMode::Review,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let branch_ref = "refs/heads/forest/reset-repro";
        assert!(crate::git::ref_exists(&repo, branch_ref).unwrap());
        let branch_worktree = crate::git::git(
            &repo,
            &["for-each-ref", "--format=%(worktreepath)", branch_ref],
        )
        .unwrap();
        assert!(
            branch_worktree.ends_with("worktrees/reset-repro/repo-reset-branch"),
            "branch should be checked out in the forest worktree: {}",
            branch_worktree
        );

        let base = env.worktree_base();
        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-reset-branch");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let reset_result = cmd_reset(true, false, false, None).unwrap();
        assert!(reset_result.errors.is_empty(), "{:?}", reset_result.errors);
        assert!(reset_result.forests[0].removed);
        assert!(matches!(
            reset_result.forests[0].repos[0].branch_deleted,
            RmOutcome::Success
        ));
        assert!(!base.join("reset-repro").exists());
        assert!(!crate::git::ref_exists(&repo, branch_ref).unwrap());
    }

    #[test]
    #[serial]
    fn reset_preserves_metadata_when_branch_deletion_fails() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-unmerged-branch");
        let tmpl = env.default_template(&["repo-unmerged-branch"]);

        cmd_new(
            NewInputs {
                name: "reset-unmerged".to_string(),
                mode: ForestMode::Review,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let base = env.worktree_base();
        let forest_dir = base.join("reset-unmerged");
        let worktree = forest_dir.join("repo-unmerged-branch");
        std::fs::write(worktree.join("unmerged.txt"), "keep me").unwrap();
        crate::git::git(&worktree, &["add", "unmerged.txt"]).unwrap();
        crate::git::git(
            &worktree,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@test.com",
                "commit",
                "-m",
                "unmerged forest work",
            ],
        )
        .unwrap();

        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-unmerged-branch");
        let state_dir = tmp_root.join("state").join(channel::APP_NAME);
        std::fs::create_dir_all(&state_dir).unwrap();
        let state_file = state_dir.join("state.toml");
        std::fs::write(&state_file, "").unwrap();
        let config_file = tmp_root
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("not fully merged")),
            "{:?}",
            result.errors
        );
        assert!(!result.forests[0].removed);
        assert!(
            forest_dir.exists(),
            "forest metadata should remain retryable"
        );
        assert!(config_file.exists(), "config should remain for retry");
        assert!(
            state_file.exists(),
            "state should remain when reset is incomplete"
        );
        assert!(crate::git::ref_exists(&repo, "refs/heads/forest/reset-unmerged").unwrap());

        let summary = format_reset_summary(&result);
        assert!(summary.contains("Config: preserved"), "{}", summary);
        assert!(summary.contains("State:  preserved"), "{}", summary);
    }

    #[test]
    #[serial]
    fn reset_dry_run_reports_planned_branch_deletion_in_json() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-reset-dry-run");
        let tmpl = env.default_template(&["repo-reset-dry-run"]);

        cmd_new(
            NewInputs {
                name: "reset-dry-run".to_string(),
                mode: ForestMode::Review,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let base = env.worktree_base();
        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-reset-dry-run");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let result = cmd_reset(false, false, true, None).unwrap();
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["forests"][0]["repos"][0]["branch"],
            "forest/reset-dry-run"
        );
        assert_eq!(json["forests"][0]["repos"][0]["branch_created"], true);
        assert_eq!(
            json["forests"][0]["repos"][0]["branch_deleted"]["status"],
            "success"
        );
        assert!(crate::git::ref_exists(&repo, "refs/heads/forest/reset-dry-run").unwrap());
        assert!(base.join("reset-dry-run").exists());
    }

    #[test]
    #[serial]
    fn reset_dry_run_reports_source_missing_as_top_level_error() {
        use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
        use crate::paths::{AbsolutePath, ForestName};

        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("source-missing");
        std::fs::create_dir_all(forest_dir.join("repo-missing-source")).unwrap();
        let missing_source =
            AbsolutePath::new(tmp.path().join("src").join("missing-source")).unwrap();
        let meta = ForestMeta {
            name: ForestName::new("source-missing".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: ForestMode::Review,
            repos: vec![RepoMeta {
                name: RepoName::new("repo-missing-source".to_string()).unwrap(),
                source: missing_source,
                branch: "forest/source-missing".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: true,
            }],
        };
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "testuser/{{name}}"

[[template.default.repos]]
path = "{}"
name = "repo-missing-source"
"#,
            worktree_base.display(),
            tmp.path().join("src").join("missing-source").display(),
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();
        let _env_guard = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(false, false, true, None).unwrap();
        assert!(!result.errors.is_empty());
        assert!(matches!(
            result.forests[0].repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(!result.forests[0].removed);
        assert!(!result.config_file.deleted);

        let output = format_reset_human(&result);
        assert!(output.contains("source-missing: blocked"), "{}", output);
        assert!(output.contains("Config: blocked"), "{}", output);

        let confirm_result = cmd_reset(false, false, false, None).unwrap();
        assert!(!confirm_result.confirm_required);
        let confirm_output = format_reset_human(&confirm_result);
        assert!(
            !confirm_output.contains("Pass --confirm to proceed"),
            "{}",
            confirm_output
        );
        assert!(
            confirm_output.contains("Reset blocked by errors."),
            "{}",
            confirm_output
        );
    }

    #[test]
    #[serial]
    fn reset_source_missing_and_worktree_missing_matches_dry_run() {
        use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
        use crate::paths::{AbsolutePath, ForestName};

        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("source-and-worktree-missing");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let missing_source =
            AbsolutePath::new(tmp.path().join("src").join("missing-source")).unwrap();
        let meta = ForestMeta {
            name: ForestName::new("source-and-worktree-missing".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: ForestMode::Review,
            repos: vec![RepoMeta {
                name: RepoName::new("repo-missing-both".to_string()).unwrap(),
                source: missing_source,
                branch: "forest/source-and-worktree-missing".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: true,
            }],
        };
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "testuser/{{name}}"

[[template.default.repos]]
path = "{}"
name = "repo-missing-both"
"#,
            worktree_base.display(),
            tmp.path().join("src").join("missing-source").display(),
        );
        let config_file = config_dir.join("config.toml");
        std::fs::write(&config_file, &config_content).unwrap();
        let _env_guard = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let dry_run = cmd_reset(false, false, true, None).unwrap();
        assert!(dry_run.errors.is_empty(), "{:?}", dry_run.errors);
        assert!(dry_run.forests[0].removed);
        assert!(dry_run.config_file.deleted);

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.forests[0].removed);
        assert!(!forest_dir.exists());
        assert!(!config_file.exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_source_missing_and_dangling_symlink_worktree_blocks() {
        use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
        use crate::paths::{AbsolutePath, ForestName};

        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("dangling-source-missing");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let worktree = forest_dir.join("repo-dangling");
        std::os::unix::fs::symlink(tmp.path().join("missing-target"), &worktree).unwrap();

        let missing_source =
            AbsolutePath::new(tmp.path().join("src").join("missing-source")).unwrap();
        let meta = ForestMeta {
            name: ForestName::new("dangling-source-missing".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: ForestMode::Review,
            repos: vec![RepoMeta {
                name: RepoName::new("repo-dangling".to_string()).unwrap(),
                source: missing_source,
                branch: "forest/dangling-source-missing".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: true,
            }],
        };
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "testuser/{{name}}"

[[template.default.repos]]
path = "{}"
name = "repo-dangling"
"#,
            worktree_base.display(),
            tmp.path().join("src").join("missing-source").display(),
        );
        let config_file = config_dir.join("config.toml");
        std::fs::write(&config_file, &config_content).unwrap();
        let _env_guard = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let dry_run = cmd_reset(false, false, true, None).unwrap();
        assert!(!dry_run.errors.is_empty());
        assert!(!dry_run.forests[0].removed);

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(!result.errors.is_empty());
        assert!(!result.forests[0].removed);
        assert!(forest_dir.exists());
        assert!(config_file.exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_dry_run_blocks_source_present_dangling_symlink_worktree() {
        use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
        use crate::paths::ForestName;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-source-present-dangling");
        let base = env.worktree_base();
        let forest_dir = base.join("source-present-dangling");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let worktree = forest_dir.join("repo-source-present-dangling");
        std::os::unix::fs::symlink(base.join("missing-target"), &worktree).unwrap();

        let meta = ForestMeta {
            name: ForestName::new("source-present-dangling".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: ForestMode::Review,
            repos: vec![RepoMeta {
                name: RepoName::new("repo-source-present-dangling".to_string()).unwrap(),
                source: repo.clone(),
                branch: "forest/source-present-dangling".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: true,
            }],
        };
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-source-present-dangling");
        let config_file = tmp_root
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let dry_run = cmd_reset(false, false, true, None).unwrap();
        assert!(!dry_run.errors.is_empty());
        assert!(matches!(
            dry_run.forests[0].repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(!dry_run.config_file.deleted);

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(!result.errors.is_empty());
        assert!(!result.forests[0].removed);
        assert!(forest_dir.exists());
        assert!(config_file.exists());
    }

    #[test]
    #[serial]
    fn reset_dry_run_blocks_source_present_corrupt_worktree_dir() {
        use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
        use crate::paths::ForestName;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-source-present-corrupt");
        let base = env.worktree_base();
        let forest_dir = base.join("source-present-corrupt");
        let worktree = forest_dir.join("repo-source-present-corrupt");
        std::fs::create_dir_all(&worktree).unwrap();

        let meta = ForestMeta {
            name: ForestName::new("source-present-corrupt".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: ForestMode::Review,
            repos: vec![RepoMeta {
                name: RepoName::new("repo-source-present-corrupt".to_string()).unwrap(),
                source: repo.clone(),
                branch: "forest/source-present-corrupt".to_string(),
                base_branch: "main".to_string(),
                remote: Some("origin".to_string()),
                branch_created: true,
            }],
        };
        meta.write(&forest_dir.join(META_FILENAME)).unwrap();

        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-source-present-corrupt");
        let config_file = tmp_root
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let dry_run = cmd_reset(false, false, true, None).unwrap();
        assert!(!dry_run.errors.is_empty());
        assert!(matches!(
            dry_run.forests[0].repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(!dry_run.config_file.deleted);

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(!result.errors.is_empty());
        assert!(!result.forests[0].removed);
        assert!(forest_dir.exists());
        assert!(config_file.exists());
    }

    #[test]
    #[serial]
    fn reset_preserves_metadata_for_locked_missing_worktree_metadata() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-locked-missing");
        let tmpl = env.default_template(&["repo-locked-missing"]);

        cmd_new(
            NewInputs {
                name: "reset-locked-missing".to_string(),
                mode: ForestMode::Review,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let base = env.worktree_base();
        let forest_dir = base.join("reset-locked-missing");
        let worktree = forest_dir.join("repo-locked-missing");
        let worktree_str = worktree.to_string_lossy();
        crate::git::git(&repo, &["worktree", "lock", &worktree_str]).unwrap();
        std::fs::remove_dir_all(&worktree).unwrap();

        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-locked-missing");
        let config_file = tmp_root
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(
            result.errors.iter().any(|error| {
                error.contains("metadata still lists missing worktree")
                    || error.contains("failed to prune stale missing worktree metadata")
            }),
            "{:?}",
            result.errors
        );
        assert!(!result.forests[0].removed);
        assert!(
            forest_dir.exists(),
            "forest metadata should remain after stale metadata failure"
        );
        assert!(config_file.exists(), "config should remain for retry");
        assert!(matches!(
            result.forests[0].repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
    }

    #[test]
    #[serial]
    fn reset_reports_already_missing_branch_without_failing() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-missing-branch");
        let tmpl = env.default_template(&["repo-missing-branch"]);

        cmd_new(
            NewInputs {
                name: "reset-missing-branch".to_string(),
                mode: ForestMode::Review,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let base = env.worktree_base();
        let worktree = base
            .join("reset-missing-branch")
            .join("repo-missing-branch");
        let worktree_str = worktree.to_string_lossy();
        crate::git::git(&repo, &["worktree", "remove", "--force", &worktree_str]).unwrap();
        crate::git::git(&repo, &["branch", "-d", "forest/reset-missing-branch"]).unwrap();

        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-missing-branch");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.forests[0].removed);
        assert!(matches!(
            result.forests[0].repos[0].branch_deleted,
            RmOutcome::Skipped { ref reason } if reason == "branch already deleted"
        ));
        assert!(!base.join("reset-missing-branch").exists());
    }

    #[test]
    #[serial]
    fn reset_keeps_branches_not_created_by_forest() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-existing-branch");
        crate::git::git(&repo, &["branch", "forest/reset-existing", "origin/main"]).unwrap();
        let tmpl = env.default_template(&["repo-existing-branch"]);

        cmd_new(
            NewInputs {
                name: "reset-existing".to_string(),
                mode: ForestMode::Review,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let base = env.worktree_base();
        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-existing-branch");
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let result = cmd_reset(true, false, false, None).unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(
            !result.forests[0].repos[0].branch_created,
            "pre-existing branch should be marked user-owned"
        );
        assert!(matches!(
            result.forests[0].repos[0].branch_deleted,
            RmOutcome::Skipped { .. }
        ));
        assert!(crate::git::ref_exists(&repo, "refs/heads/forest/reset-existing").unwrap());
    }

    /// After reset, re-creating a forest with the same name should succeed.
    /// This is the end-to-end repro of the bug: reset → re-init → new fails.
    #[test]
    #[serial]
    fn reset_then_recreate_forest_succeeds() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::ForestMode;
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        env.create_repo_with_remote("repo-b");
        let tmpl = env.default_template(&["repo-b"]);

        // 1. Create a forest
        let inputs = NewInputs {
            name: "recreate-test".to_string(),
            mode: ForestMode::Feature,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        };
        cmd_new(inputs, &tmpl).unwrap();

        // 2. Write config and reset
        let base = env.worktree_base();
        let tmp_root = base.parent().unwrap();
        let config_dir = tmp_root.join("config").join(channel::APP_NAME);
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_content = format!(
            r#"
default_template = "default"

[template.default]
worktree_base = "{}"
base_branch = "main"
feature_branch_template = "testuser/{{name}}"

[[template.default.repos]]
path = "{}"
name = "repo-b"
"#,
            base.display(),
            env.repo_path("repo-b").display(),
        );
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();

        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));

        let reset_result = cmd_reset(true, false, false, None).unwrap();
        assert!(reset_result.forests[0].removed);
        assert!(reset_result.errors.is_empty());

        // 3. Re-write config (reset deleted it) and try to create the same forest again
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();

        let inputs2 = NewInputs {
            name: "recreate-test".to_string(),
            mode: ForestMode::Feature,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        };
        let result2 = cmd_new(inputs2, &tmpl);
        assert!(
            result2.is_ok(),
            "re-creating forest after reset should succeed, but got: {}",
            result2.unwrap_err()
        );
    }
}
