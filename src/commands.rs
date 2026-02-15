use anyhow::{bail, Result};
use chrono::Utc;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{ResolvedConfig, ResolvedRepo};
use crate::forest::discover_forests;
use crate::git::ref_exists;
use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
use crate::paths::{expand_tilde, forest_dir, sanitize_forest_name};

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

// --- new ---

pub struct NewInputs {
    pub name: String,
    pub mode: ForestMode,
    pub branch_override: Option<String>,
    pub repo_branches: Vec<(String, String)>,
    pub no_fetch: bool,
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct ForestPlan {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub mode: ForestMode,
    pub repo_plans: Vec<RepoPlan>,
}

#[derive(Debug)]
pub struct RepoPlan {
    pub name: String,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub branch: String,
    pub base_branch: String,
    pub remote: String,
    pub checkout: CheckoutKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutKind {
    /// Branch exists locally. `git worktree add <dest> <branch>`
    ExistingLocal,
    /// Branch exists on remote. `git worktree add <dest> -b <branch> <remote>/<branch>`
    TrackRemote,
    /// Branch doesn't exist. `git worktree add <dest> -b <branch> <remote>/<base_branch>`
    NewBranch,
}

#[derive(Debug, Serialize)]
pub struct NewResult {
    pub forest_name: String,
    pub forest_dir: PathBuf,
    pub mode: ForestMode,
    pub dry_run: bool,
    pub repos: Vec<NewRepoResult>,
}

#[derive(Debug, Serialize)]
pub struct NewRepoResult {
    pub name: String,
    pub branch: String,
    pub base_branch: String,
    pub branch_created: bool,
    pub checkout_kind: CheckoutKind,
    pub worktree_path: PathBuf,
}

fn compute_target_branch(
    repo_name: &str,
    forest_name: &str,
    mode: &ForestMode,
    branch_template: &str,
    username: &str,
    branch_override: &Option<String>,
    repo_branches: &[(String, String)],
) -> String {
    // Per-repo override takes highest priority
    if let Some((_, branch)) = repo_branches.iter().find(|(name, _)| name == repo_name) {
        return branch.clone();
    }

    // Global --branch override
    if let Some(branch) = branch_override {
        return branch.clone();
    }

    // Mode default
    match mode {
        ForestMode::Feature => branch_template
            .replace("{user}", username)
            .replace("{name}", forest_name),
        ForestMode::Review => format!("forest/{}", forest_name),
    }
}

fn validate_branch_name(branch: &str, remote: &str) -> Result<()> {
    if branch.starts_with("refs/") {
        bail!(
            "branch name {:?} looks like a ref path\n  hint: pass the branch name without the refs/ prefix",
            branch
        );
    }
    let remote_prefix = format!("{}/", remote);
    if branch.starts_with(&remote_prefix) {
        bail!(
            "branch name {:?} looks like a remote ref\n  hint: pass the branch name without the remote prefix: {:?}",
            branch,
            &branch[remote_prefix.len()..]
        );
    }
    Ok(())
}

pub fn plan_forest(inputs: &NewInputs, config: &ResolvedConfig) -> Result<ForestPlan> {
    // Validate forest name
    if inputs.name.is_empty() || inputs.name == "." || inputs.name == ".." {
        bail!(
            "invalid forest name: {:?}\n  hint: provide a descriptive name like \"java-84/refactor-auth\"",
            inputs.name
        );
    }
    let sanitized = sanitize_forest_name(&inputs.name);
    if sanitized.is_empty() {
        bail!(
            "forest name {:?} sanitizes to empty\n  hint: provide a name with at least one alphanumeric character",
            inputs.name
        );
    }

    // Validate config has repos
    if config.repos.is_empty() {
        bail!("no repos configured\n  hint: run `git forest init --repo <path>` to add repos");
    }

    // Validate --repo-branch keys: no duplicates
    {
        let mut seen = HashSet::new();
        for (repo_name, _) in &inputs.repo_branches {
            if !seen.insert(repo_name.as_str()) {
                bail!(
                    "duplicate repo-branch for: {}\n  hint: specify each repo at most once",
                    repo_name
                );
            }
        }
    }

    // Validate --repo-branch keys: all match config repos
    {
        let known_names: HashSet<&str> = config.repos.iter().map(|r| r.name.as_str()).collect();
        for (repo_name, _) in &inputs.repo_branches {
            if !known_names.contains(repo_name.as_str()) {
                let known: Vec<&str> = config.repos.iter().map(|r| r.name.as_str()).collect();
                bail!(
                    "unknown repo: {}\n  hint: known repos: {}",
                    repo_name,
                    known.join(", ")
                );
            }
        }
    }

    // Compute forest directory
    let fdir = forest_dir(&config.general.worktree_base, &inputs.name);

    // Create worktree_base if it doesn't exist (match discover_forests leniency)
    if !config.general.worktree_base.exists() {
        std::fs::create_dir_all(&config.general.worktree_base)?;
    }

    // Check for directory/name collision
    if fdir.exists() {
        bail!(
            "forest directory already exists: {}\n  hint: choose a different name, or remove the existing forest with `git forest rm`",
            fdir.display()
        );
    }
    // Also check for name collision via meta scan
    if let Some((existing_dir, existing_meta)) =
        crate::forest::find_forest(&config.general.worktree_base, &inputs.name)?
    {
        bail!(
            "forest name {:?} collides with existing forest {:?} at {}\n  hint: choose a different name",
            inputs.name,
            existing_meta.name,
            existing_dir.display()
        );
    }

    // Validate source repos exist and branch names
    for repo in &config.repos {
        if !repo.path.is_dir() {
            bail!(
                "source repo not found: {}\n  hint: check that the path exists, or update config with `git forest init --force`",
                repo.path.display()
            );
        }
    }

    // Validate all branch names (global override, per-repo overrides, and computed defaults)
    if let Some(ref branch) = inputs.branch_override {
        // Validate against all remotes — use first repo's remote as representative
        if let Some(repo) = config.repos.first() {
            validate_branch_name(branch, &repo.remote)?;
        }
    }
    for (repo_name, branch) in &inputs.repo_branches {
        let remote = config
            .repos
            .iter()
            .find(|r| r.name == *repo_name)
            .map(|r| r.remote.as_str())
            .unwrap_or("origin");
        validate_branch_name(branch, remote)?;
    }

    // Build repo plans
    let mut repo_plans = Vec::new();
    for repo in &config.repos {
        let branch = compute_target_branch(
            &repo.name,
            &inputs.name,
            &inputs.mode,
            &config.general.branch_template,
            &config.general.username,
            &inputs.branch_override,
            &inputs.repo_branches,
        );

        validate_branch_name(&branch, &repo.remote)?;

        // Branch resolution
        let local_ref = format!("refs/heads/{}", branch);
        let remote_ref = format!("refs/remotes/{}/{}", repo.remote, branch);

        let checkout = if ref_exists(&repo.path, &local_ref)? {
            CheckoutKind::ExistingLocal
        } else if ref_exists(&repo.path, &remote_ref)? {
            CheckoutKind::TrackRemote
        } else {
            // Verify base branch exists on remote
            let base_ref = format!("refs/remotes/{}/{}", repo.remote, repo.base_branch);
            if !ref_exists(&repo.path, &base_ref)? {
                bail!(
                    "{}/{} not found in {}\n  hint: check that base_branch {:?} exists on remote {:?}, or run `git fetch {}` in {}",
                    repo.remote,
                    repo.base_branch,
                    repo.name,
                    repo.base_branch,
                    repo.remote,
                    repo.remote,
                    repo.path.display()
                );
            }
            CheckoutKind::NewBranch
        };

        let dest = fdir.join(&repo.name);

        repo_plans.push(RepoPlan {
            name: repo.name.clone(),
            source: repo.path.clone(),
            dest,
            branch,
            base_branch: repo.base_branch.clone(),
            remote: repo.remote.clone(),
            checkout,
        });
    }

    Ok(ForestPlan {
        forest_name: inputs.name.clone(),
        forest_dir: fdir,
        mode: inputs.mode.clone(),
        repo_plans,
    })
}

fn branch_created(checkout: &CheckoutKind) -> bool {
    match checkout {
        CheckoutKind::ExistingLocal => false,
        CheckoutKind::TrackRemote => false,
        CheckoutKind::NewBranch => true,
    }
}

fn plan_to_result(plan: &ForestPlan, dry_run: bool) -> NewResult {
    let repos = plan
        .repo_plans
        .iter()
        .map(|rp| NewRepoResult {
            name: rp.name.clone(),
            branch: rp.branch.clone(),
            base_branch: rp.base_branch.clone(),
            branch_created: branch_created(&rp.checkout),
            checkout_kind: rp.checkout.clone(),
            worktree_path: rp.dest.clone(),
        })
        .collect();

    NewResult {
        forest_name: plan.forest_name.clone(),
        forest_dir: plan.forest_dir.clone(),
        mode: plan.mode.clone(),
        dry_run,
        repos,
    }
}

pub fn execute_plan(plan: &ForestPlan) -> Result<NewResult> {
    // Create forest directory
    std::fs::create_dir_all(&plan.forest_dir)?;

    // Write initial meta with empty repos
    let mut meta = ForestMeta {
        name: plan.forest_name.clone(),
        created_at: Utc::now(),
        mode: plan.mode.clone(),
        repos: vec![],
    };
    let meta_path = plan.forest_dir.join(META_FILENAME);
    meta.write(&meta_path)?;

    // Create worktrees incrementally
    for repo_plan in &plan.repo_plans {
        let dest_str = repo_plan.dest.to_string_lossy();

        match &repo_plan.checkout {
            CheckoutKind::ExistingLocal => {
                crate::git::git(
                    &repo_plan.source,
                    &["worktree", "add", &dest_str, &repo_plan.branch],
                )?;
            }
            CheckoutKind::TrackRemote => {
                let start = format!("{}/{}", repo_plan.remote, repo_plan.branch);
                crate::git::git(
                    &repo_plan.source,
                    &[
                        "worktree",
                        "add",
                        &dest_str,
                        "-b",
                        &repo_plan.branch,
                        &start,
                    ],
                )?;
            }
            CheckoutKind::NewBranch => {
                let start = format!("{}/{}", repo_plan.remote, repo_plan.base_branch);
                crate::git::git(
                    &repo_plan.source,
                    &[
                        "worktree",
                        "add",
                        &dest_str,
                        "-b",
                        &repo_plan.branch,
                        &start,
                    ],
                )?;
            }
        }

        // Update meta incrementally
        meta.repos.push(RepoMeta {
            name: repo_plan.name.clone(),
            source: repo_plan.source.clone(),
            branch: repo_plan.branch.clone(),
            base_branch: repo_plan.base_branch.clone(),
            branch_created: branch_created(&repo_plan.checkout),
        });
        meta.write(&meta_path)?;
    }

    Ok(plan_to_result(plan, false))
}

pub fn cmd_new(inputs: NewInputs, config: &ResolvedConfig) -> Result<NewResult> {
    // Fetch unless --no-fetch
    if !inputs.no_fetch {
        for repo in &config.repos {
            if repo.path.is_dir() {
                crate::git::git(&repo.path, &["fetch", &repo.remote])?;
            }
        }
    }

    let plan = plan_forest(&inputs, config)?;

    if inputs.dry_run {
        return Ok(plan_to_result(&plan, true));
    }

    execute_plan(&plan)
}

pub fn format_new_human(result: &NewResult) -> String {
    let mut lines = Vec::new();

    if result.dry_run {
        lines.push("Dry run — no changes made.".to_string());
        lines.push(String::new());
    }

    lines.push(format!(
        "Forest {:?} ({} mode)",
        result.forest_name, result.mode
    ));
    lines.push(format!("  {}", result.forest_dir.display()));
    lines.push(String::new());

    for repo in &result.repos {
        let kind = match &repo.checkout_kind {
            CheckoutKind::ExistingLocal => "existing",
            CheckoutKind::TrackRemote => "track remote",
            CheckoutKind::NewBranch => "new branch",
        };
        lines.push(format!("  {} → {} ({})", repo.name, repo.branch, kind));
    }

    lines.join("\n")
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
    use crate::config::GeneralConfig;
    use crate::meta::{ForestMode, RepoMeta};
    use crate::testutil::TestEnv;
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

    // === new command tests ===

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

    fn make_config_with_repos(env: &TestEnv, repo_names: &[&str]) -> ResolvedConfig {
        env.default_config(repo_names)
    }

    // --- Branch computation ---

    #[test]
    fn feature_mode_uses_branch_template() {
        let branch = compute_target_branch(
            "foo-api",
            "java-84/refactor-auth",
            &ForestMode::Feature,
            "{user}/{name}",
            "dliv",
            &None,
            &[],
        );
        assert_eq!(branch, "dliv/java-84/refactor-auth");
    }

    #[test]
    fn review_mode_uses_forest_prefix() {
        let branch = compute_target_branch(
            "foo-api",
            "review-sues-dialog",
            &ForestMode::Review,
            "{user}/{name}",
            "dliv",
            &None,
            &[],
        );
        assert_eq!(branch, "forest/review-sues-dialog");
    }

    #[test]
    fn branch_override_applies_to_all_repos() {
        let override_branch = Some("custom-branch".to_string());
        let b1 = compute_target_branch(
            "foo-api",
            "test",
            &ForestMode::Feature,
            "{user}/{name}",
            "dliv",
            &override_branch,
            &[],
        );
        let b2 = compute_target_branch(
            "foo-web",
            "test",
            &ForestMode::Feature,
            "{user}/{name}",
            "dliv",
            &override_branch,
            &[],
        );
        assert_eq!(b1, "custom-branch");
        assert_eq!(b2, "custom-branch");
    }

    #[test]
    fn repo_branch_override_applies_to_specific_repo() {
        let repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];
        let b1 = compute_target_branch(
            "foo-api",
            "review-pr",
            &ForestMode::Review,
            "{user}/{name}",
            "dliv",
            &None,
            &repo_branches,
        );
        let b2 = compute_target_branch(
            "foo-web",
            "review-pr",
            &ForestMode::Review,
            "{user}/{name}",
            "dliv",
            &None,
            &repo_branches,
        );
        assert_eq!(b1, "forest/review-pr");
        assert_eq!(b2, "sue/fix-dialog");
    }

    // --- Input validation ---

    #[test]
    fn plan_empty_name_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);
        let mut inputs = make_new_inputs("", ForestMode::Feature);
        inputs.no_fetch = true;

        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid forest name"));
    }

    #[test]
    fn plan_dot_name_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        for name in &[".", ".."] {
            let inputs = make_new_inputs(name, ForestMode::Feature);
            let result = plan_forest(&inputs, &config);
            assert!(result.is_err(), "expected error for name {:?}", name);
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("invalid forest name"));
        }
    }

    #[test]
    fn plan_forest_dir_collision_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        // Pre-create the forest directory
        let fdir = forest_dir(&config.general.worktree_base, "my-feature");
        std::fs::create_dir_all(&fdir).unwrap();

        let inputs = make_new_inputs("my-feature", ForestMode::Feature);
        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn plan_empty_config_repos_errors() {
        let env = TestEnv::new();
        let config = ResolvedConfig {
            general: GeneralConfig {
                worktree_base: env.worktree_base(),
                base_branch: "main".to_string(),
                branch_template: "{user}/{name}".to_string(),
                username: "testuser".to_string(),
            },
            repos: vec![],
        };

        let inputs = make_new_inputs("test", ForestMode::Feature);
        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no repos configured"));
    }

    #[test]
    fn plan_source_repo_missing_errors() {
        let env = TestEnv::new();
        let config = ResolvedConfig {
            general: GeneralConfig {
                worktree_base: env.worktree_base(),
                base_branch: "main".to_string(),
                branch_template: "{user}/{name}".to_string(),
                username: "testuser".to_string(),
            },
            repos: vec![crate::config::ResolvedRepo {
                path: PathBuf::from("/nonexistent/repo"),
                name: "missing".to_string(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            }],
        };

        let inputs = make_new_inputs("test", ForestMode::Feature);
        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("source repo not found"));
    }

    #[test]
    fn repo_branch_override_unknown_repo_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.repo_branches = vec![("nonexistent".to_string(), "branch".to_string())];

        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown repo"), "error: {}", err);
        assert!(err.contains("foo-api"), "should list known repos: {}", err);
    }

    #[test]
    fn duplicate_repo_branch_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.repo_branches = vec![
            ("foo-api".to_string(), "branch-a".to_string()),
            ("foo-api".to_string(), "branch-b".to_string()),
        ];

        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("duplicate repo-branch"));
    }

    #[test]
    fn plan_ambiguous_branch_refs_prefix_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.branch_override = Some("refs/heads/my-branch".to_string());

        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("looks like a ref path"));
    }

    #[test]
    fn plan_ambiguous_branch_remote_prefix_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.branch_override = Some("origin/my-branch".to_string());

        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("looks like a remote ref"));
    }

    #[test]
    fn plan_base_branch_ref_missing_errors() {
        let env = TestEnv::new();
        // create_repo_with_remote sets up origin/main. Set base_branch to "dev" (doesn't exist).
        env.create_repo_with_remote("foo-api");
        let mut config = make_config_with_repos(&env, &["foo-api"]);
        config.repos[0].base_branch = "dev".to_string();

        let inputs = make_new_inputs("test", ForestMode::Feature);
        let result = plan_forest(&inputs, &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("origin/dev not found"), "error: {}", err);
    }

    // --- Branch resolution ---

    #[test]
    fn plan_resolves_existing_local_branch() {
        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("foo-api");
        // Create a local branch
        crate::git::git(&repo, &["branch", "testuser/my-feature"]).unwrap();

        let config = make_config_with_repos(&env, &["foo-api"]);
        let inputs = make_new_inputs("my-feature", ForestMode::Feature);

        let plan = plan_forest(&inputs, &config).unwrap();
        assert_eq!(plan.repo_plans.len(), 1);
        assert!(matches!(
            plan.repo_plans[0].checkout,
            CheckoutKind::ExistingLocal
        ));
        assert!(!branch_created(&plan.repo_plans[0].checkout));
    }

    #[test]
    fn plan_resolves_remote_branch() {
        let env = TestEnv::new();
        let repo = env.create_repo_with_remote("foo-api");
        // Push a branch to origin but don't keep it locally
        crate::git::git(&repo, &["branch", "testuser/my-feature"]).unwrap();
        crate::git::git(&repo, &["push", "origin", "testuser/my-feature"]).unwrap();
        crate::git::git(&repo, &["branch", "-D", "testuser/my-feature"]).unwrap();
        crate::git::git(&repo, &["fetch", "origin"]).unwrap();

        let config = make_config_with_repos(&env, &["foo-api"]);
        let inputs = make_new_inputs("my-feature", ForestMode::Feature);

        let plan = plan_forest(&inputs, &config).unwrap();
        assert_eq!(plan.repo_plans.len(), 1);
        assert!(matches!(
            plan.repo_plans[0].checkout,
            CheckoutKind::TrackRemote
        ));
        assert!(!branch_created(&plan.repo_plans[0].checkout));
    }

    #[test]
    fn plan_resolves_new_branch() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("brand-new-feature", ForestMode::Feature);

        let plan = plan_forest(&inputs, &config).unwrap();
        assert_eq!(plan.repo_plans.len(), 1);
        assert!(matches!(
            plan.repo_plans[0].checkout,
            CheckoutKind::NewBranch
        ));
        assert!(branch_created(&plan.repo_plans[0].checkout));
    }

    // --- Full plan shape ---

    #[test]
    fn plan_feature_mode_all_repos() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let config = make_config_with_repos(&env, &["foo-api", "foo-web"]);

        let inputs = make_new_inputs("java-84/refactor-auth", ForestMode::Feature);
        let plan = plan_forest(&inputs, &config).unwrap();

        assert_eq!(plan.forest_name, "java-84/refactor-auth");
        assert_eq!(plan.mode, ForestMode::Feature);
        assert_eq!(plan.repo_plans.len(), 2);
        assert_eq!(plan.repo_plans[0].branch, "testuser/java-84/refactor-auth");
        assert_eq!(plan.repo_plans[1].branch, "testuser/java-84/refactor-auth");
        assert!(matches!(
            plan.repo_plans[0].checkout,
            CheckoutKind::NewBranch
        ));
    }

    #[test]
    fn plan_review_mode_with_exception() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let config = make_config_with_repos(&env, &["foo-api", "foo-web"]);

        let mut inputs = make_new_inputs("review-sues-dialog", ForestMode::Review);
        inputs.repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];

        let plan = plan_forest(&inputs, &config).unwrap();

        assert_eq!(plan.repo_plans.len(), 2);
        // foo-api gets the default review branch
        assert_eq!(plan.repo_plans[0].branch, "forest/review-sues-dialog");
        // foo-web gets the exception branch
        assert_eq!(plan.repo_plans[1].branch, "sue/fix-dialog");
    }

    // --- execute_plan ---

    #[test]
    fn execute_creates_forest_dir_and_worktrees() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let config = make_config_with_repos(&env, &["foo-api", "foo-web"]);

        let inputs = make_new_inputs("exec-test", ForestMode::Feature);
        let plan = plan_forest(&inputs, &config).unwrap();
        let result = execute_plan(&plan).unwrap();

        assert_eq!(result.repos.len(), 2);
        assert!(!result.dry_run);

        // Verify directories exist
        assert!(plan.forest_dir.exists());
        assert!(plan.forest_dir.join("foo-api").exists());
        assert!(plan.forest_dir.join("foo-web").exists());

        // Verify meta file exists
        assert!(plan.forest_dir.join(META_FILENAME).exists());
    }

    #[test]
    fn execute_meta_matches_plan() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("meta-test", ForestMode::Feature);
        let plan = plan_forest(&inputs, &config).unwrap();
        execute_plan(&plan).unwrap();

        let meta_path = plan.forest_dir.join(META_FILENAME);
        let meta = ForestMeta::read(&meta_path).unwrap();

        assert_eq!(meta.name, "meta-test");
        assert_eq!(meta.mode, ForestMode::Feature);
        assert_eq!(meta.repos.len(), 1);
        assert_eq!(meta.repos[0].name, "foo-api");
        assert_eq!(meta.repos[0].branch, "testuser/meta-test");
        assert_eq!(meta.repos[0].base_branch, "main");
        assert!(meta.repos[0].branch_created);
    }

    // --- cmd_new ---

    #[test]
    fn cmd_new_feature_mode_creates_forest() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let config = make_config_with_repos(&env, &["foo-api", "foo-web"]);

        let inputs = make_new_inputs("e2e-feature", ForestMode::Feature);
        let result = cmd_new(inputs, &config).unwrap();

        assert_eq!(result.forest_name, "e2e-feature");
        assert_eq!(result.mode, ForestMode::Feature);
        assert!(!result.dry_run);
        assert_eq!(result.repos.len(), 2);
        assert_eq!(result.repos[0].branch, "testuser/e2e-feature");
        assert!(result.repos[0].branch_created);
        assert!(result.forest_dir.exists());
    }

    #[test]
    fn cmd_new_dry_run_does_not_create() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("dry-run-test", ForestMode::Feature);
        inputs.dry_run = true;

        let result = cmd_new(inputs, &config).unwrap();

        assert!(result.dry_run);
        assert_eq!(result.repos.len(), 1);
        // Forest directory should NOT exist
        assert!(!result.forest_dir.exists());
    }

    #[test]
    fn cmd_new_review_mode_with_repo_branch() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let config = make_config_with_repos(&env, &["foo-api", "foo-web"]);

        let mut inputs = make_new_inputs("review-test", ForestMode::Review);
        inputs.repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];

        let result = cmd_new(inputs, &config).unwrap();

        assert_eq!(result.repos.len(), 2);
        assert_eq!(result.repos[0].branch, "forest/review-test");
        assert_eq!(result.repos[1].branch, "sue/fix-dialog");
    }

    #[test]
    fn cmd_new_duplicate_forest_name_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        // First create succeeds
        let inputs = make_new_inputs("dup-test", ForestMode::Feature);
        cmd_new(inputs, &config).unwrap();

        // Second create should fail
        let inputs2 = make_new_inputs("dup-test", ForestMode::Feature);
        let result = cmd_new(inputs2, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn ls_shows_new_forest() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let config = make_config_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("ls-test", ForestMode::Feature);
        cmd_new(inputs, &config).unwrap();

        let ls_result = cmd_ls(&config.general.worktree_base).unwrap();
        assert_eq!(ls_result.forests.len(), 1);
        assert_eq!(ls_result.forests[0].name, "ls-test");
        assert_eq!(ls_result.forests[0].mode, ForestMode::Feature);
    }
}
