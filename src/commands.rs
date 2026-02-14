use anyhow::{bail, Result};
use chrono::Utc;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{ResolvedConfig, ResolvedRepo};
use crate::forest::discover_forests;
use crate::meta::{ForestMeta, ForestMode};
use crate::paths::expand_tilde;

/// Result structs for command output. Commands return these instead of printing
/// directly — main.rs formats them as human-readable or JSON based on --json.
/// See architecture-decisions.md, Decision 8.

#[derive(Debug, Serialize)]
pub struct LsResult {
    pub forests: Vec<ForestSummary>,
}

#[derive(Debug, Serialize)]
pub struct ForestSummary {
    pub name: String,
    pub age_seconds: i64,
    pub age_display: String,
    pub mode: ForestMode,
    pub branch_summary: Vec<BranchCount>,
}

#[derive(Debug, Serialize)]
pub struct BranchCount {
    pub branch: String,
    pub count: usize,
}

pub fn cmd_ls(worktree_base: &Path) -> Result<LsResult> {
    let mut forests = discover_forests(worktree_base)?;
    forests.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let summaries = forests.iter().map(summarize_forest).collect();
    Ok(LsResult { forests: summaries })
}

fn summarize_forest(forest: &ForestMeta) -> ForestSummary {
    let age_seconds = (Utc::now() - forest.created_at).num_seconds();
    let branch_summary = branch_counts(&forest.repos);
    ForestSummary {
        name: forest.name.clone(),
        age_seconds,
        age_display: format_age(age_seconds),
        mode: forest.mode.clone(),
        branch_summary,
    }
}

fn branch_counts(repos: &[crate::meta::RepoMeta]) -> Vec<BranchCount> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for repo in repos {
        *counts.entry(repo.branch.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(branch, count)| BranchCount {
            branch: branch.to_string(),
            count,
        })
        .collect()
}

fn format_age(seconds: i64) -> String {
    let days = seconds / 86400;
    let hours = seconds / 3600;
    let minutes = seconds / 60;

    if days > 0 {
        format!("{}d ago", days)
    } else if hours > 0 {
        format!("{}h ago", hours)
    } else {
        format!("{}m ago", minutes.max(1))
    }
}

fn format_branches(branch_summary: &[BranchCount]) -> String {
    branch_summary
        .iter()
        .map(|bc| {
            if bc.count == 1 {
                bc.branch.clone()
            } else {
                format!("{} ({})", bc.branch, bc.count)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn format_ls_human(result: &LsResult) -> String {
    if result.forests.is_empty() {
        return "No forests found. Create one with `git forest new <name>`.".to_string();
    }

    let name_width = result
        .forests
        .iter()
        .map(|f| f.name.len())
        .max()
        .unwrap_or(0)
        .max(4);

    let mut lines = Vec::new();
    lines.push(format!(
        "{:<name_width$}  {:<10}  {:<8}  BRANCHES",
        "NAME", "AGE", "MODE"
    ));

    for forest in &result.forests {
        let branches = format_branches(&forest.branch_summary);
        lines.push(format!(
            "{:<name_width$}  {:<10}  {:<8}  {}",
            forest.name, forest.age_display, forest.mode, branches
        ));
    }

    lines.join("\n")
}

#[derive(Debug, Serialize)]
pub struct StatusResult {
    pub forest_name: String,
    pub repos: Vec<RepoStatus>,
}

#[derive(Debug, Serialize)]
pub struct RepoStatus {
    pub name: String,
    pub status: RepoStatusKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RepoStatusKind {
    Ok { output: String },
    Missing { path: String },
    Error { message: String },
}

pub fn cmd_status(forest_dir: &Path, meta: &ForestMeta) -> Result<StatusResult> {
    let mut repos = Vec::new();

    for repo in &meta.repos {
        let worktree = forest_dir.join(&repo.name);

        let status = if !worktree.exists() {
            RepoStatusKind::Missing {
                path: worktree.display().to_string(),
            }
        } else {
            match crate::git::git(&worktree, &["status", "-sb"]) {
                Ok(output) => RepoStatusKind::Ok { output },
                Err(e) => RepoStatusKind::Error {
                    message: e.to_string(),
                },
            }
        };

        repos.push(RepoStatus {
            name: repo.name.clone(),
            status,
        });
    }

    Ok(StatusResult {
        forest_name: meta.name.clone(),
        repos,
    })
}

pub fn format_status_human(result: &StatusResult) -> String {
    let mut lines = Vec::new();
    for repo in &result.repos {
        lines.push(format!("=== {} ===", repo.name));
        match &repo.status {
            RepoStatusKind::Ok { output } => lines.push(output.clone()),
            RepoStatusKind::Missing { path } => {
                lines.push(format!("  warning: worktree missing at {}", path));
            }
            RepoStatusKind::Error { message } => {
                lines.push(format!("  warning: {}", message));
            }
        }
    }
    lines.join("\n")
}

#[derive(Debug, Serialize)]
pub struct ExecResult {
    pub forest_name: String,
    pub failures: Vec<String>,
}

pub fn cmd_exec(forest_dir: &Path, meta: &ForestMeta, cmd: &[String]) -> Result<ExecResult> {
    if cmd.is_empty() {
        anyhow::bail!("no command specified");
    }

    let mut failures = Vec::new();

    for repo in &meta.repos {
        let worktree = forest_dir.join(&repo.name);
        eprintln!("=== {} ===", repo.name);

        if !worktree.exists() {
            eprintln!("  warning: worktree missing at {}", worktree.display());
            failures.push(repo.name.clone());
            continue;
        }

        let status = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .current_dir(&worktree)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();

        match status {
            Ok(s) if !s.success() => {
                failures.push(repo.name.clone());
            }
            Err(e) => {
                eprintln!("  error: {}", e);
                failures.push(repo.name.clone());
            }
            _ => {}
        }
    }

    Ok(ExecResult {
        forest_name: meta.name.clone(),
        failures,
    })
}

pub fn format_exec_human(result: &ExecResult) -> String {
    if result.failures.is_empty() {
        String::new()
    } else {
        format!("\nFailed in: {}", result.failures.join(", "))
    }
}

// --- init ---

pub struct InitInputs {
    pub worktree_base: String,
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
    pub repos: Vec<RepoInput>,
}

pub struct RepoInput {
    pub path: String,
    pub name: Option<String>,
    pub base_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InitResult {
    pub config_path: PathBuf,
    pub worktree_base: PathBuf,
    pub repos: Vec<InitRepoSummary>,
}

#[derive(Debug, Serialize)]
pub struct InitRepoSummary {
    pub name: String,
    pub path: PathBuf,
    pub base_branch: String,
}

pub fn validate_init_inputs(inputs: &InitInputs) -> Result<ResolvedConfig> {
    if inputs.username.is_empty() {
        bail!("--username is required\nHint: git forest init --username <your-name> --repo <path>");
    }

    if inputs.repos.is_empty() {
        bail!("at least one --repo is required\nHint: git forest init --username <your-name> --repo <path>");
    }

    if !inputs.branch_template.contains("{name}") {
        bail!("--branch-template must contain {{name}}");
    }

    let worktree_base = expand_tilde(&inputs.worktree_base);

    let mut resolved_repos = Vec::new();
    let mut names = HashSet::new();

    for repo_input in &inputs.repos {
        let path = expand_tilde(&repo_input.path);

        if !path.exists() {
            bail!(
                "repo path does not exist: {}\nHint: provide an absolute path to a git repository",
                path.display()
            );
        }

        // Verify it's a git repo
        let git_check = std::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match git_check {
            Ok(s) if !s.success() => {
                bail!(
                    "not a git repository: {}\nHint: provide a path to a git repository",
                    path.display()
                );
            }
            Err(e) => {
                bail!("failed to check git repo at {}: {}", path.display(), e);
            }
            _ => {}
        }

        let name = repo_input.name.clone().unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        if name.is_empty() {
            bail!("repo has empty name (path: {})", path.display());
        }

        if !names.insert(name.clone()) {
            bail!(
                "duplicate repo name: {}\nHint: use --repo /path:custom-name to disambiguate",
                name
            );
        }

        let base_branch = repo_input
            .base_branch
            .clone()
            .unwrap_or_else(|| inputs.base_branch.clone());

        resolved_repos.push(ResolvedRepo {
            path,
            name,
            base_branch,
            remote: "origin".to_string(),
        });
    }

    debug_assert!(
        worktree_base.is_absolute() || inputs.worktree_base.starts_with("~/"),
        "worktree_base should be absolute after tilde expansion"
    );
    debug_assert!(
        resolved_repos.iter().all(|r| !r.name.is_empty()),
        "all repo names must be non-empty"
    );

    Ok(ResolvedConfig {
        general: crate::config::GeneralConfig {
            worktree_base,
            base_branch: inputs.base_branch.clone(),
            branch_template: inputs.branch_template.clone(),
            username: inputs.username.clone(),
        },
        repos: resolved_repos,
    })
}

pub fn cmd_init(inputs: InitInputs, config_path: &Path, force: bool) -> Result<InitResult> {
    let resolved = validate_init_inputs(&inputs)?;
    crate::config::write_config_atomic(config_path, &resolved, force)?;

    let repos = resolved
        .repos
        .iter()
        .map(|r| InitRepoSummary {
            name: r.name.clone(),
            path: r.path.clone(),
            base_branch: r.base_branch.clone(),
        })
        .collect();

    Ok(InitResult {
        config_path: config_path.to_path_buf(),
        worktree_base: resolved.general.worktree_base,
        repos,
    })
}

pub fn format_init_human(result: &InitResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Config written to {}",
        result.config_path.display()
    ));
    lines.push(format!("Worktree base: {}", result.worktree_base.display()));
    lines.push(format!("Repos ({}): ", result.repos.len()));
    for repo in &result.repos {
        lines.push(format!(
            "  {} ({}, base: {})",
            repo.name,
            repo.path.display(),
            repo.base_branch
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{ForestMode, RepoMeta};
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn make_meta(
        name: &str,
        created_at: chrono::DateTime<Utc>,
        mode: ForestMode,
        repos: Vec<RepoMeta>,
    ) -> ForestMeta {
        ForestMeta {
            name: name.to_string(),
            created_at,
            mode,
            repos,
        }
    }

    fn make_repo(name: &str, branch: &str) -> RepoMeta {
        RepoMeta {
            name: name.to_string(),
            source: PathBuf::from(format!("/tmp/src/{}", name)),
            branch: branch.to_string(),
            base_branch: "dev".to_string(),
            branch_created: true,
        }
    }

    // --- format_age tests (refactored to take i64 seconds) ---

    #[test]
    fn format_age_days() {
        assert_eq!(format_age(3 * 86400), "3d ago");
    }

    #[test]
    fn format_age_hours() {
        assert_eq!(format_age(5 * 3600), "5h ago");
    }

    #[test]
    fn format_age_minutes() {
        assert_eq!(format_age(15 * 60), "15m ago");
    }

    #[test]
    fn format_age_just_created() {
        assert_eq!(format_age(0), "1m ago");
    }

    // --- format_branches tests (refactored to take &[BranchCount]) ---

    #[test]
    fn format_branches_single_branch_all_repos() {
        let counts = vec![BranchCount {
            branch: "dliv/feature".to_string(),
            count: 3,
        }];
        assert_eq!(format_branches(&counts), "dliv/feature (3)");
    }

    #[test]
    fn format_branches_mixed() {
        let counts = vec![
            BranchCount {
                branch: "forest/review-pr".to_string(),
                count: 2,
            },
            BranchCount {
                branch: "sue/fix-dialog".to_string(),
                count: 1,
            },
        ];
        assert_eq!(
            format_branches(&counts),
            "forest/review-pr (2), sue/fix-dialog"
        );
    }

    #[test]
    fn format_branches_all_different() {
        let counts = vec![
            BranchCount {
                branch: "branch-a".to_string(),
                count: 1,
            },
            BranchCount {
                branch: "branch-b".to_string(),
                count: 1,
            },
        ];
        assert_eq!(format_branches(&counts), "branch-a, branch-b");
    }

    // --- cmd_ls tests ---

    #[test]
    fn cmd_ls_empty_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        let result = cmd_ls(tmp.path()).unwrap();
        assert!(result.forests.is_empty());
    }

    #[test]
    fn cmd_ls_nonexistent_dir() {
        let result = cmd_ls(Path::new("/nonexistent/path")).unwrap();
        assert!(result.forests.is_empty());
    }

    #[test]
    fn cmd_ls_with_forests() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        let meta_a = make_meta(
            "feature-a",
            Utc.with_ymd_and_hms(2026, 2, 10, 12, 0, 0).unwrap(),
            ForestMode::Feature,
            vec![
                make_repo("api", "dliv/feature-a"),
                make_repo("web", "dliv/feature-a"),
            ],
        );
        let meta_b = make_meta(
            "review-pr",
            Utc.with_ymd_and_hms(2026, 2, 12, 8, 0, 0).unwrap(),
            ForestMode::Review,
            vec![
                make_repo("api", "forest/review-pr"),
                make_repo("web", "sue/fix"),
            ],
        );

        let dir_a = base.join("feature-a");
        let dir_b = base.join("review-pr");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        meta_a.write(&dir_a.join(".forest-meta.toml")).unwrap();
        meta_b.write(&dir_b.join(".forest-meta.toml")).unwrap();

        let result = cmd_ls(base).unwrap();
        assert_eq!(result.forests.len(), 2);

        let names: Vec<&str> = result.forests.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"feature-a"));
        assert!(names.contains(&"review-pr"));

        let feature_a = result
            .forests
            .iter()
            .find(|f| f.name == "feature-a")
            .unwrap();
        assert_eq!(feature_a.mode, ForestMode::Feature);
        assert_eq!(feature_a.branch_summary.len(), 1);
        assert_eq!(feature_a.branch_summary[0].branch, "dliv/feature-a");
        assert_eq!(feature_a.branch_summary[0].count, 2);

        let review_pr = result
            .forests
            .iter()
            .find(|f| f.name == "review-pr")
            .unwrap();
        assert_eq!(review_pr.mode, ForestMode::Review);
        assert_eq!(review_pr.branch_summary.len(), 2);
    }

    #[test]
    fn format_ls_human_empty() {
        let result = LsResult { forests: vec![] };
        let text = format_ls_human(&result);
        assert!(text.contains("No forests found"));
    }

    #[test]
    fn format_ls_human_with_data() {
        let result = LsResult {
            forests: vec![ForestSummary {
                name: "my-feature".to_string(),
                age_seconds: 7200,
                age_display: "2h ago".to_string(),
                mode: ForestMode::Feature,
                branch_summary: vec![BranchCount {
                    branch: "dliv/my-feature".to_string(),
                    count: 2,
                }],
            }],
        };
        let text = format_ls_human(&result);
        assert!(text.contains("NAME"));
        assert!(text.contains("my-feature"));
        assert!(text.contains("2h ago"));
        assert!(text.contains("feature"));
        assert!(text.contains("dliv/my-feature (2)"));
    }

    // --- status tests ---

    fn setup_forest_with_git_repos(base: &Path) -> (PathBuf, ForestMeta) {
        let forest_dir = base.join("test-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();

        // Create real git repos as worktrees
        for name in &["api", "web"] {
            let repo_dir = forest_dir.join(name);
            std::fs::create_dir_all(&repo_dir).unwrap();
            let run = |args: &[&str]| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&repo_dir)
                    .env("GIT_AUTHOR_NAME", "Test")
                    .env("GIT_AUTHOR_EMAIL", "test@test.com")
                    .env("GIT_COMMITTER_NAME", "Test")
                    .env("GIT_COMMITTER_EMAIL", "test@test.com")
                    .output()
                    .unwrap();
            };
            run(&["init", "-b", "main"]);
            run(&["commit", "--allow-empty", "-m", "initial"]);
        }

        let meta = make_meta(
            "test-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("api", "main"), make_repo("web", "main")],
        );

        (forest_dir, meta)
    }

    #[test]
    fn cmd_status_runs_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());
        let result = cmd_status(&forest_dir, &meta).unwrap();
        assert_eq!(result.repos.len(), 2);
        assert!(matches!(result.repos[0].status, RepoStatusKind::Ok { .. }));
        assert!(matches!(result.repos[1].status, RepoStatusKind::Ok { .. }));
    }

    #[test]
    fn cmd_status_missing_worktree_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let forest_dir = tmp.path().join("test-forest");
        std::fs::create_dir_all(&forest_dir).unwrap();

        let meta = make_meta(
            "test-forest",
            Utc::now(),
            ForestMode::Feature,
            vec![make_repo("missing-repo", "main")],
        );

        let result = cmd_status(&forest_dir, &meta).unwrap();
        assert_eq!(result.repos.len(), 1);
        assert!(matches!(
            result.repos[0].status,
            RepoStatusKind::Missing { .. }
        ));
    }

    // --- exec tests ---

    #[test]
    fn cmd_exec_runs_command_in_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let cmd = vec!["echo".to_string(), "hello".to_string()];
        let result = cmd_exec(&forest_dir, &meta, &cmd).unwrap();
        assert!(result.failures.is_empty());
    }

    #[test]
    fn cmd_exec_empty_cmd_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (forest_dir, meta) = setup_forest_with_git_repos(tmp.path());

        let result = cmd_exec(&forest_dir, &meta, &[]);
        assert!(result.is_err());
    }

    // --- init tests ---

    fn create_test_git_repo(base: &Path, name: &str) -> PathBuf {
        let repo_dir = base.join(name);
        std::fs::create_dir_all(&repo_dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_dir)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap();
        };
        run(&["init", "-b", "main"]);
        run(&["commit", "--allow-empty", "-m", "initial"]);
        repo_dir
    }

    fn make_init_inputs(repos: Vec<RepoInput>) -> InitInputs {
        InitInputs {
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/{name}".to_string(),
            username: "testuser".to_string(),
            repos,
        }
    }

    #[test]
    fn validate_init_valid_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");

        let inputs = make_init_inputs(vec![RepoInput {
            path: repo.display().to_string(),
            name: None,
            base_branch: None,
        }]);

        let config = validate_init_inputs(&inputs).unwrap();
        assert_eq!(config.repos.len(), 1);
        assert_eq!(config.repos[0].name, "my-repo");
        assert_eq!(config.repos[0].base_branch, "dev");
        assert_eq!(config.general.username, "testuser");
    }

    #[test]
    fn validate_init_missing_username() {
        let inputs = InitInputs {
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/{name}".to_string(),
            username: String::new(),
            repos: vec![RepoInput {
                path: "/tmp/foo".to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--username"));
    }

    #[test]
    fn validate_init_empty_repos() {
        let inputs = make_init_inputs(vec![]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--repo"));
    }

    #[test]
    fn validate_init_duplicate_repo_names() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_a = create_test_git_repo(tmp.path(), "repo");
        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let repo_b = create_test_git_repo(&sub, "repo");

        let inputs = make_init_inputs(vec![
            RepoInput {
                path: repo_a.display().to_string(),
                name: None,
                base_branch: None,
            },
            RepoInput {
                path: repo_b.display().to_string(),
                name: None,
                base_branch: None,
            },
        ]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn validate_init_not_a_git_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let not_git = tmp.path().join("not-git");
        std::fs::create_dir_all(&not_git).unwrap();

        let inputs = make_init_inputs(vec![RepoInput {
            path: not_git.display().to_string(),
            name: None,
            base_branch: None,
        }]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a git"));
    }

    #[test]
    fn validate_init_branch_template_missing_name() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            worktree_base: "/tmp/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/feature".to_string(),
            username: "testuser".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("branch-template"));
    }

    #[test]
    fn validate_init_tilde_expansion() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "repo");

        let inputs = InitInputs {
            worktree_base: "~/worktrees".to_string(),
            base_branch: "dev".to_string(),
            branch_template: "{user}/{name}".to_string(),
            username: "testuser".to_string(),
            repos: vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }],
        };

        let config = validate_init_inputs(&inputs).unwrap();
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            config.general.worktree_base,
            PathBuf::from(&home).join("worktrees")
        );
    }

    #[test]
    fn cmd_init_creates_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let inputs = make_init_inputs(vec![RepoInput {
            path: repo.display().to_string(),
            name: None,
            base_branch: None,
        }]);

        let result = cmd_init(inputs, &config_path, false).unwrap();
        assert_eq!(result.config_path, config_path);
        assert_eq!(result.repos.len(), 1);
        assert!(config_path.exists());

        // Verify it's valid TOML that can be parsed back
        let loaded = crate::config::load_config(&config_path).unwrap();
        assert_eq!(loaded.repos[0].name, "my-repo");
    }

    #[test]
    fn cmd_init_force_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let make = || {
            make_init_inputs(vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }])
        };

        cmd_init(make(), &config_path, false).unwrap();
        // Second call with force should succeed
        cmd_init(make(), &config_path, true).unwrap();
    }

    #[test]
    fn cmd_init_without_force_errors_on_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = create_test_git_repo(tmp.path(), "my-repo");
        let config_path = tmp.path().join("config").join("config.toml");

        let make = || {
            make_init_inputs(vec![RepoInput {
                path: repo.display().to_string(),
                name: None,
                base_branch: None,
            }])
        };

        cmd_init(make(), &config_path, false).unwrap();
        // Second call without force should fail
        let result = cmd_init(make(), &config_path, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn validate_init_repo_path_not_exists() {
        let inputs = make_init_inputs(vec![RepoInput {
            path: "/nonexistent/repo/path".to_string(),
            name: None,
            base_branch: None,
        }]);

        let result = validate_init_inputs(&inputs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }
}
