use anyhow::Result;
use std::path::{Path, PathBuf};

use super::rm::{self, CleanupIssue, CleanupIssueCode, RepoRmPlan, RmOutcome};
use crate::forest::{dedupe_discovered_forests, discover_forests_with_dirs};
use crate::plan::reset::{
    decide_reset_plan, plan_to_confirm_required, plan_to_dry_run, repo_reset_entry, root_refused,
    BaseScanFailure, ConfigObservation, DiscoveryObservation, FileResetEntry, ForestObservation,
    ForestResetEntry, ResetFilesObservation, ResetObservation, ResetPlan,
};
pub use crate::plan::reset::{format_reset_human, format_reset_summary, ResetResult};

pub enum ResetProgress<'a> {
    ForestStarting { name: &'a str, path: &'a Path },
    ForestDone(&'a ForestResetEntry),
}

// --- Planning (read-only) ---

/// Plans a reset: observes the config, the state file and the forests, then decides.
fn plan_reset(config_only: bool) -> Result<ResetPlan> {
    decide_reset_plan(config_only, observe_reset(config_only))
}

// --- Observation (read-only) ---

/// Reads what planning a reset needs: the config and state paths and whether they exist, and,
/// unless `config_only`, the forests under the config's worktree bases. Every read error is
/// kept for `decide_reset_plan`.
fn observe_reset(config_only: bool) -> ResetObservation {
    let config_path = crate::config::default_config_path();
    let state_path = crate::config::xdg_state_dir().map(|dir| dir.join("state.toml"));
    let files = match (&config_path, &state_path) {
        (Ok(config_path), Ok(state_path)) => {
            Some(observe_reset_files(config_path, state_path, config_only))
        }
        _ => None,
    };
    ResetObservation {
        config_path,
        state_path,
        files,
    }
}

fn observe_reset_files(
    config_path: &Path,
    state_path: &Path,
    config_only: bool,
) -> ResetFilesObservation {
    let config_exists = config_path.exists();
    let state_exists = state_path.exists();
    let config =
        (!config_only && config_exists).then(|| match crate::config::load_config(config_path) {
            Ok(config) => {
                ConfigObservation::Scanned(observe_discovery(config.all_worktree_bases()))
            }
            Err(error) => ConfigObservation::Unparsable(error.to_string()),
        });
    ResetFilesObservation {
        config_exists,
        state_exists,
        config,
    }
}

/// Scans each worktree base for forests, keeping each base's error, and observes every forest
/// found, after deduplication, as a dry-run of its removal would.
fn observe_discovery(bases: Vec<&Path>) -> DiscoveryObservation {
    let mut base_failures = Vec::new();
    let mut discovered_forests = Vec::new();
    for base in bases {
        match discover_forests_with_dirs(base) {
            Ok(mut discovered) => discovered_forests.append(&mut discovered),
            Err(error) => base_failures.push(BaseScanFailure {
                base: base.to_path_buf(),
                error: error.to_string(),
            }),
        }
    }
    let forests = dedupe_discovered_forests(discovered_forests)
        .into_iter()
        .map(|discovered| {
            let rm = rm::observe_rm_planning(&discovered.dir, &discovered.meta);
            let dry_run = rm::observe_rm_dry_run(&discovered.dir, &discovered.meta, &rm.repos);
            ForestObservation {
                path_exists: discovered.dir.exists(),
                path: discovered.dir,
                meta: discovered.meta,
                rm,
                dry_run,
            }
        })
        .collect();
    DiscoveryObservation {
        base_failures,
        forests,
    }
}

// --- Execution (impure) ---

fn execute_reset(
    plan: &ResetPlan,
    now: chrono::DateTime<chrono::Utc>,
    on_progress: Option<&dyn Fn(ResetProgress)>,
) -> ResetResult {
    let mut errors = plan.discovery_errors.clone();
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
        if let Some(error) = forest
            .root_rejection
            .clone()
            .or_else(|| rm::current_forest_root_safety_error(path).map(root_refused))
        {
            errors.push(error);
        } else {
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
        }

        let removed = if errors.len() > forest_error_start || !path.exists() {
            false
        } else if let Some(error) = rm::current_forest_root_safety_error(path) {
            errors.push(CleanupIssue::new(
                CleanupIssueCode::ForestRootRefused,
                error,
            ));
            false
        } else {
            rm::remove_forest_dir_recursively_preserving_meta(path, &mut errors)
        };
        let entry = ForestResetEntry {
            name: name.clone(),
            path: path.clone(),
            removed,
            repos,
            path_exists: path.exists(),
        };

        if let Some(cb) = &on_progress {
            cb(ResetProgress::ForestDone(&entry));
        }
        forest_entries.push(entry);
    }

    let cleanup_succeeded = errors.is_empty();

    let (config_deleted, config_backed_up_to) = if cleanup_succeeded && plan.config_exists {
        backup_file(&plan.config_path, now, &mut errors)
    } else {
        (false, None)
    };

    let state_deleted = if cleanup_succeeded && plan.state_exists {
        delete_file(&plan.state_path, &mut errors)
    } else {
        false
    };

    // The renderers tell a blocked forest from an already missing one by whether its path
    // exists once every step has run.
    for entry in &mut forest_entries {
        entry.path_exists = entry.path.exists();
    }

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

fn remove_reset_worktree(repo: &RepoRmPlan, errors: &mut Vec<CleanupIssue>) -> (RmOutcome, bool) {
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
            errors.push(CleanupIssue::new(
                CleanupIssueCode::WorktreeRemoveFailed,
                msg.clone(),
            ));
            (RmOutcome::Failed { error: msg }, false)
        }
    }
}

fn remove_missing_reset_worktree(
    repo: &RepoRmPlan,
    errors: &mut Vec<CleanupIssue>,
) -> (RmOutcome, bool) {
    if let Some(msg) = rm::stale_missing_worktree_metadata_error(repo, true) {
        errors.push(CleanupIssue::new(
            CleanupIssueCode::WorktreeMetadataStale,
            msg.clone(),
        ));
        return (RmOutcome::Failed { error: msg }, false);
    }

    (
        RmOutcome::Skipped {
            reason: "worktree already missing".to_string(),
        },
        true,
    )
}

fn delete_file(path: &Path, errors: &mut Vec<CleanupIssue>) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => {
            errors.push(CleanupIssue::new(
                CleanupIssueCode::StateDeleteFailed,
                format!("failed to delete {}: {}", path.display(), e),
            ));
            false
        }
    }
}

/// Rename a file to `<name>.<timestamp>.bak` instead of deleting it.
/// Returns `(removed_from_original_path, backup_path)`.
fn backup_file(
    path: &Path,
    now: chrono::DateTime<chrono::Utc>,
    errors: &mut Vec<CleanupIssue>,
) -> (bool, Option<PathBuf>) {
    let timestamp = now.format("%Y%m%d%H%M%S");
    let file_stem = path.file_name().unwrap_or_default().to_string_lossy();
    let backup_name = format!("{}.{}.bak", file_stem, timestamp);
    let backup_path = path.with_file_name(&backup_name);

    match std::fs::rename(path, &backup_path) {
        Ok(()) => (true, Some(backup_path)),
        Err(e) => {
            errors.push(CleanupIssue::new(
                CleanupIssueCode::ConfigBackupFailed,
                format!(
                    "failed to back up {} to {}: {}",
                    path.display(),
                    backup_path.display(),
                    e
                ),
            ));
            (false, None)
        }
    }
}

// --- Orchestrator ---

pub fn cmd_reset(
    confirm: bool,
    config_only: bool,
    dry_run: bool,
    now: chrono::DateTime<chrono::Utc>,
    on_progress: Option<&dyn Fn(ResetProgress)>,
) -> Result<ResetResult> {
    let plan = plan_reset(config_only)?;

    if dry_run {
        return Ok(plan_to_dry_run(&plan));
    }

    if !confirm {
        return Ok(plan_to_confirm_required(&plan));
    }

    Ok(execute_reset(&plan, now, on_progress))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel;
    use crate::paths::RepoName;
    use crate::plan::reset::RepoResetEntry;
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
                path_exists: false,
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
                path_exists: false,
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
            errors: vec![CleanupIssue::new(
                CleanupIssueCode::ConfigBackupFailed,
                "failed to delete config".to_string(),
            )],
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
                path_exists: false,
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None);
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

        let result = cmd_reset(true, true, false, chrono::Utc::now(), None).unwrap();

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

        let result = cmd_reset(false, false, false, chrono::Utc::now(), None).unwrap();
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

        let result = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
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
            disposable_root_entries: vec![],
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

        assert!(!result.dry_run);
        assert_eq!(result.forests.len(), 1);
        assert!(result.forests[0].removed);
        assert!(!forest_dir.exists());
        assert!(result.errors.is_empty());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_preserves_metadata_when_parent_prevents_final_directory_removal() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config").join(channel::APP_NAME);
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&worktree_base).unwrap();

        let forest_dir = worktree_base.join("recoverable-reset");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("recoverable-reset".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            disposable_root_entries: vec![],
            repos: vec![],
        };
        let meta_path = forest_dir.join(crate::meta::META_FILENAME);
        meta.write(&meta_path).unwrap();
        std::fs::write(forest_dir.join("residue.txt"), "discard during reset").unwrap();

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
        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, &config_content).unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let original_permissions = std::fs::metadata(&worktree_base).unwrap().permissions();
        std::fs::set_permissions(&worktree_base, std::fs::Permissions::from_mode(0o555)).unwrap();
        let permission_probe = worktree_base.join("permission-probe");
        if std::fs::write(&permission_probe, "probe").is_ok() {
            std::fs::set_permissions(&worktree_base, original_permissions).unwrap();
            std::fs::remove_file(permission_probe).unwrap();
            return;
        }

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
        std::fs::set_permissions(&worktree_base, original_permissions).unwrap();

        assert!(!result.forests[0].removed);
        assert!(rm::legacy_error_lines(&result.errors)
            .iter()
            .any(|error| error.contains("failed to stage forest metadata")));
        assert!(meta_path.exists());
        assert!(forest_dir.join("residue.txt").exists());
        assert!(config_path.exists());

        let retry = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
        assert_eq!(retry.forests.len(), 1);
        assert_eq!(retry.forests[0].path, forest_dir);
    }

    #[test]
    #[serial]
    fn reset_corrupt_metadata_blocks_config_and_state_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree_base = tmp.path().join("worktrees");
        let forest_dir = worktree_base.join("corrupt-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();
        std::fs::write(
            forest_dir.join(crate::meta::META_FILENAME),
            "not valid metadata",
        )
        .unwrap();
        write_reset_config(
            tmp.path(),
            &worktree_base,
            Path::new("/tmp/nonexistent-repo"),
            "repo-a",
        );
        let config_path = tmp
            .path()
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let state_path = tmp
            .path()
            .join("state")
            .join(channel::APP_NAME)
            .join("state.toml");
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(&state_path, "").unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let dry_run = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();

        assert!(!dry_run.config_file.deleted);
        assert!(!dry_run.state_file.deleted);
        assert!(rm::legacy_error_lines(&dry_run.errors)
            .iter()
            .any(
                |error| error.contains("could not safely scan worktree base")
                    && error.contains("could not read forest metadata")
            ));

        let actual = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

        assert!(!actual.config_file.deleted);
        assert!(!actual.state_file.deleted);
        assert!(config_path.exists());
        assert!(state_path.exists());
        assert!(forest_dir.exists());
    }

    #[test]
    #[serial]
    fn reset_staged_metadata_blocks_config_and_state_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree_base = tmp.path().join("worktrees");
        std::fs::create_dir_all(worktree_base.join("interrupted-forest")).unwrap();
        let staged_meta = worktree_base.join(format!("{}999999", crate::meta::STAGED_META_PREFIX));
        std::fs::write(&staged_meta, "recoverable metadata").unwrap();
        write_reset_config(
            tmp.path(),
            &worktree_base,
            Path::new("/tmp/nonexistent-repo"),
            "repo-a",
        );
        let config_path = tmp
            .path()
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

        assert!(!result.config_file.deleted);
        assert!(rm::legacy_error_lines(&result.errors).iter().any(|error| {
            error.contains("could not safely scan worktree base")
                && error.contains("found staged forest metadata")
        }));
        assert!(config_path.exists());
        assert!(staged_meta.exists());
        assert!(worktree_base.join("interrupted-forest").exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_inaccessible_worktree_base_blocks_config_and_state_cleanup() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let blocked_parent = tmp.path().join("blocked");
        let worktree_base = blocked_parent.join("worktrees");
        let forest_dir = worktree_base.join("hidden-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("hidden-forest".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            disposable_root_entries: vec![],
            repos: vec![],
        };
        meta.write(&forest_dir.join(crate::meta::META_FILENAME))
            .unwrap();
        write_reset_config(
            tmp.path(),
            &worktree_base,
            Path::new("/tmp/nonexistent-repo"),
            "repo-a",
        );
        let config_path = tmp
            .path()
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let state_path = tmp
            .path()
            .join("state")
            .join(channel::APP_NAME)
            .join("state.toml");
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(&state_path, "").unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));

        let original_permissions = std::fs::metadata(&blocked_parent).unwrap().permissions();
        std::fs::set_permissions(&blocked_parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&worktree_base).is_ok() {
            std::fs::set_permissions(&blocked_parent, original_permissions).unwrap();
            return;
        }

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
        std::fs::set_permissions(&blocked_parent, original_permissions).unwrap();

        assert!(!result.config_file.deleted);
        assert!(!result.state_file.deleted);
        assert!(rm::legacy_error_lines(&result.errors).iter().any(|error| {
            error.contains("could not safely scan worktree base")
                && error.contains("could not read worktree base")
        }));
        assert!(config_path.exists());
        assert!(state_path.exists());
        assert!(forest_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_dangling_worktree_base_symlink_blocks_config_and_state_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree_target = tmp.path().join("worktrees");
        let worktree_base = tmp.path().join("worktrees-link");
        let forest_dir = worktree_target.join("offline-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();
        let meta = crate::meta::ForestMeta {
            name: crate::paths::ForestName::new("offline-forest".to_string()).unwrap(),
            created_at: chrono::Utc::now(),
            mode: crate::meta::ForestMode::Feature,
            disposable_root_entries: vec![],
            repos: vec![],
        };
        meta.write(&forest_dir.join(crate::meta::META_FILENAME))
            .unwrap();
        std::os::unix::fs::symlink(&worktree_target, &worktree_base).unwrap();
        write_reset_config(
            tmp.path(),
            &worktree_base,
            Path::new("/tmp/nonexistent-repo"),
            "repo-a",
        );
        let config_path = tmp
            .path()
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let state_path = tmp
            .path()
            .join("state")
            .join(channel::APP_NAME)
            .join("state.toml");
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(&state_path, "").unwrap();
        let _env = XdgEnvGuard::set(tmp.path().join("config"), tmp.path().join("state"));
        let offline_target = tmp.path().join("worktrees-offline");
        std::fs::rename(&worktree_target, &offline_target).unwrap();

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

        assert!(!result.config_file.deleted);
        assert!(!result.state_file.deleted);
        assert!(rm::legacy_error_lines(&result.errors).iter().any(|error| {
            error.contains("could not safely scan worktree base")
                && error.contains("could not read worktree base")
        }));
        assert!(config_path.exists());
        assert!(state_path.exists());
        assert!(offline_target.join("offline-forest").exists());
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
            disposable_root_entries: vec![],
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

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
            disposable_root_entries: vec![],
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.forests.len(), 1);
        assert_eq!(result.forests[0].path, forest_dir);
        assert!(result.forests[0].removed);
        assert!(!forest_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn reset_refuses_symlinked_forest_before_mutation() {
        use crate::commands::new::{cmd_new, NewInputs};
        use crate::meta::{ForestMode, META_FILENAME};
        use crate::testutil::TestEnv;

        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("repo-reset-symlink");
        let tmpl = env.default_template(&["repo-reset-symlink"]);
        cmd_new(
            NewInputs {
                name: "reset-symlink".to_string(),
                mode: ForestMode::Feature,
                branch_override: None,
                repo_branches: vec![],
                no_fetch: true,
                dry_run: false,
            },
            &tmpl,
        )
        .unwrap();

        let base = env.worktree_base();
        let forest_alias = base.join("reset-symlink");
        let target = env.root().join("relocated-reset-symlink");
        std::fs::rename(&forest_alias, &target).unwrap();
        std::os::unix::fs::symlink(&target, &forest_alias).unwrap();

        let tmp_root = base.parent().unwrap();
        write_reset_config(tmp_root, &base, &repo, "repo-reset-symlink");
        let config_file = tmp_root
            .join("config")
            .join(channel::APP_NAME)
            .join("config.toml");
        let state_dir = tmp_root.join("state").join(channel::APP_NAME);
        std::fs::create_dir_all(&state_dir).unwrap();
        let state_file = state_dir.join("state.toml");
        std::fs::write(&state_file, "").unwrap();
        let _env_guard = XdgEnvGuard::set(tmp_root.join("config"), tmp_root.join("state"));
        let branch_ref = "refs/heads/testuser/reset-symlink";

        let dry_run = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
        assert!(!dry_run.forests[0].removed);
        assert!(dry_run.forests[0].repos.is_empty());
        assert!(rm::legacy_error_lines(&dry_run.errors)
            .iter()
            .any(|error| error.contains("forest removal refused") && error.contains("symlink")));
        assert!(!dry_run.config_file.deleted);
        assert!(!dry_run.state_file.deleted);

        let actual = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
        assert!(!actual.forests[0].removed);
        assert!(actual.forests[0].repos.is_empty());
        assert!(rm::legacy_error_lines(&actual.errors)
            .iter()
            .any(|error| error.contains("forest removal refused") && error.contains("symlink")));
        assert!(forest_alias
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(target.join("repo-reset-symlink").exists());
        assert!(target.join(META_FILENAME).exists());
        assert!(crate::git::ref_exists(&repo, branch_ref).unwrap());
        assert!(config_file.exists());
        assert!(state_file.exists());
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
            disposable_root_entries: vec![],
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

        let result = cmd_reset(true, true, false, chrono::Utc::now(), None).unwrap();

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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();

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
        let reset_result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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

        let reset_result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
        assert!(
            rm::legacy_error_lines(&result.errors)
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

        let result = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
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
            disposable_root_entries: vec![],
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

        let result = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
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

        let confirm_result = cmd_reset(false, false, false, chrono::Utc::now(), None).unwrap();
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
            disposable_root_entries: vec![],
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

        let dry_run = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
        assert!(dry_run.errors.is_empty(), "{:?}", dry_run.errors);
        assert!(dry_run.forests[0].removed);
        assert!(dry_run.config_file.deleted);

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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
            disposable_root_entries: vec![],
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

        let dry_run = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
        assert!(!dry_run.errors.is_empty());
        assert!(!dry_run.forests[0].removed);

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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
            disposable_root_entries: vec![],
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

        let dry_run = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
        assert!(!dry_run.errors.is_empty());
        assert!(matches!(
            dry_run.forests[0].repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(!dry_run.config_file.deleted);

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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
            disposable_root_entries: vec![],
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

        let dry_run = cmd_reset(false, false, true, chrono::Utc::now(), None).unwrap();
        assert!(!dry_run.errors.is_empty());
        assert!(matches!(
            dry_run.forests[0].repos[0].worktree_removed,
            RmOutcome::Failed { .. }
        ));
        assert!(!dry_run.config_file.deleted);

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
        assert!(
            rm::legacy_error_lines(&result.errors).iter().any(|error| {
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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

        let result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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

        let reset_result = cmd_reset(true, false, false, chrono::Utc::now(), None).unwrap();
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
