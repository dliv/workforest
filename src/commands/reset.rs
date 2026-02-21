use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::forest::discover_forests;
use crate::paths::{sanitize_forest_name, AbsolutePath};

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
}

#[derive(Debug, Serialize)]
pub struct ForestResetEntry {
    pub name: String,
    pub path: PathBuf,
    pub removed: bool,
}

pub enum ResetProgress<'a> {
    ForestStarting { name: &'a str, path: &'a Path },
    ForestDone(&'a ForestResetEntry),
}

// --- Planning (read-only) ---

struct ForestRepoInfo {
    source: AbsolutePath,
    worktree_dir: PathBuf,
}

struct ForestInfo {
    name: String,
    path: PathBuf,
    repos: Vec<ForestRepoInfo>,
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
                for base in bases {
                    match discover_forests(base) {
                        Ok(metas) => {
                            for meta in metas {
                                let dir_name = sanitize_forest_name(meta.name.as_str());
                                let forest_path = base.join(&dir_name);
                                let repos = meta
                                    .repos
                                    .iter()
                                    .map(|r| ForestRepoInfo {
                                        source: r.source.clone(),
                                        worktree_dir: forest_path.join(r.name.as_str()),
                                    })
                                    .collect();
                                forests.push(ForestInfo {
                                    name: meta.name.to_string(),
                                    path: forest_path,
                                    repos,
                                });
                            }
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
        // Best-effort: log failures as warnings but don't block deletion.
        for repo in &forest.repos {
            assert!(
                repo.worktree_dir.starts_with(&forest.path),
                "worktree dir {:?} is not inside forest dir {:?}",
                repo.worktree_dir,
                forest.path
            );
            let wt_str = repo.worktree_dir.to_string_lossy();
            if let Err(e) =
                crate::git::git(&repo.source, &["worktree", "remove", "--force", &wt_str])
            {
                errors.push(format!(
                    "warning: git worktree remove failed for {}: {}",
                    wt_str, e
                ));
            }
        }

        let entry = if !path.exists() {
            ForestResetEntry {
                name: name.clone(),
                path: path.clone(),
                removed: false,
            }
        } else {
            match std::fs::remove_dir_all(path) {
                Ok(()) => ForestResetEntry {
                    name: name.clone(),
                    path: path.clone(),
                    removed: true,
                },
                Err(e) => {
                    errors.push(format!("failed to remove forest {}: {}", name, e));
                    ForestResetEntry {
                        name: name.clone(),
                        path: path.clone(),
                        removed: false,
                    }
                }
            }
        };

        if let Some(cb) = &on_progress {
            cb(ResetProgress::ForestDone(&entry));
        }
        forest_entries.push(entry);
    }

    let config_deleted = if plan.config_exists {
        delete_file(&plan.config_path, &mut errors)
    } else {
        false
    };

    let state_deleted = if plan.state_exists {
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
        },
        state_file: FileResetEntry {
            path: plan.state_path.clone(),
            existed: plan.state_exists,
            deleted: state_deleted,
        },
        forests: forest_entries,
        warnings: plan.warnings.clone(),
        errors,
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

// --- Orchestrator ---

fn plan_to_dry_run(plan: &ResetPlan) -> ResetResult {
    ResetResult {
        dry_run: true,
        confirm_required: false,
        config_only: plan.config_only,
        config_file: FileResetEntry {
            path: plan.config_path.clone(),
            existed: plan.config_exists,
            deleted: plan.config_exists,
        },
        state_file: FileResetEntry {
            path: plan.state_path.clone(),
            existed: plan.state_exists,
            deleted: plan.state_exists,
        },
        forests: plan
            .forests
            .iter()
            .map(|f| ForestResetEntry {
                name: f.name.clone(),
                path: f.path.clone(),
                removed: f.path.exists(),
            })
            .collect(),
        warnings: plan.warnings.clone(),
        errors: vec![],
    }
}

fn plan_to_confirm_required(plan: &ResetPlan) -> ResetResult {
    let mut result = plan_to_dry_run(plan);
    result.dry_run = false;
    result.confirm_required = true;
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

    if result.confirm_required {
        lines.push("The following would be deleted:".to_string());
    } else if result.dry_run {
        lines.push("Dry run — no changes will be made.".to_string());
    } else if result.errors.is_empty() {
        lines.push("Reset complete.".to_string());
    } else {
        lines.push("Reset completed with errors.".to_string());
    }

    let is_preview = result.dry_run || result.confirm_required;

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
            }
        }
    }

    lines.push(String::new());

    let config_status = format_file_status(&result.config_file, is_preview);
    lines.push(format!(
        "Config: {} ({})",
        config_status,
        result.config_file.path.display()
    ));

    let state_status = format_file_status(&result.state_file, is_preview);
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

    if result.confirm_required {
        lines.push(String::new());
        lines.push("Pass --confirm to proceed.".to_string());
    }

    lines.join("\n")
}

pub fn format_reset_summary(result: &ResetResult) -> String {
    let mut lines = Vec::new();

    if result.errors.is_empty() {
        lines.push("Reset complete.".to_string());
    } else {
        lines.push("Reset completed with errors.".to_string());
    }

    let is_preview = false;

    lines.push(String::new());
    let config_status = format_file_status(&result.config_file, is_preview);
    lines.push(format!(
        "Config: {} ({})",
        config_status,
        result.config_file.path.display()
    ));
    let state_status = format_file_status(&result.state_file, is_preview);
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

fn format_file_status(entry: &FileResetEntry, is_preview: bool) -> &'static str {
    if is_preview {
        if entry.existed {
            "would delete"
        } else {
            "not found"
        }
    } else if entry.deleted {
        "deleted"
    } else if entry.existed {
        "failed"
    } else {
        "not found"
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
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state/git-forest/state.toml"),
                existed: true,
                deleted: true,
            },
            forests: vec![ForestResetEntry {
                name: "my-feature".to_string(),
                path: PathBuf::from("/tmp/worktrees/my-feature"),
                removed: true,
            }],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("would be deleted"));
        assert!(output.contains("would remove"));
        assert!(output.contains("would delete"));
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
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: false,
                deleted: false,
            },
            forests: vec![],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Dry run"));
        assert!(output.contains("would delete"));
        assert!(output.contains("not found"));
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
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: true,
                deleted: true,
            },
            forests: vec![ForestResetEntry {
                name: "test".to_string(),
                path: PathBuf::from("/tmp/worktrees/test"),
                removed: true,
            }],
            warnings: vec![],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Reset complete"));
        assert!(output.contains("removed"));
        assert!(output.contains("deleted"));
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
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: true,
                deleted: true,
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
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: true,
                deleted: true,
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
            },
            state_file: FileResetEntry {
                path: PathBuf::from("/tmp/state.toml"),
                existed: false,
                deleted: false,
            },
            forests: vec![],
            warnings: vec!["config could not be parsed".to_string()],
            errors: vec![],
        };

        let output = format_reset_human(&result);
        assert!(output.contains("Warnings:"));
        assert!(output.contains("config could not be parsed"));
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
