use anyhow::{bail, Result};
use chrono::Utc;
use serde::Serialize;
use std::collections::HashSet;

use crate::config::ResolvedTemplate;
use crate::git::ref_exists;
use crate::meta::{ForestMeta, ForestMode, RepoMeta, META_FILENAME};
use crate::paths::{forest_dir, AbsolutePath, BranchName, ForestName, RepoName};

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
    pub forest_name: ForestName,
    pub forest_dir: AbsolutePath,
    pub mode: ForestMode,
    pub repo_plans: Vec<RepoPlan>,
}

#[derive(Debug)]
pub struct RepoPlan {
    pub name: RepoName,
    pub source: AbsolutePath,
    pub dest: AbsolutePath,
    pub branch: BranchName,
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
    pub forest_name: ForestName,
    pub forest_dir: AbsolutePath,
    pub mode: ForestMode,
    pub dry_run: bool,
    pub repos: Vec<NewRepoResult>,
}

#[derive(Debug, Serialize)]
pub struct NewRepoResult {
    pub name: RepoName,
    pub branch: String,
    pub base_branch: String,
    pub branch_created: bool,
    pub checkout_kind: CheckoutKind,
    pub worktree_path: AbsolutePath,
}

fn compute_target_branch(
    repo_name: &str,
    forest_name: &str,
    mode: &ForestMode,
    feature_branch_template: &str,
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
        ForestMode::Feature => feature_branch_template.replace("{name}", forest_name),
        ForestMode::Review => format!("forest/{}", forest_name),
    }
}

pub fn plan_forest(inputs: &NewInputs, tmpl: &ResolvedTemplate) -> Result<ForestPlan> {
    // Validate forest name
    let forest_name = ForestName::new(inputs.name.clone())?;

    // Validate template has repos
    if tmpl.repos.is_empty() {
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

    // Validate --repo-branch keys: all match template repos
    {
        let known_names: HashSet<&str> = tmpl.repos.iter().map(|r| r.name.as_str()).collect();
        for (repo_name, _) in &inputs.repo_branches {
            if !known_names.contains(repo_name.as_str()) {
                let known: Vec<&str> = tmpl.repos.iter().map(|r| r.name.as_str()).collect();
                bail!(
                    "unknown repo: {}\n  hint: known repos: {}",
                    repo_name,
                    known.join(", ")
                );
            }
        }
    }

    // Compute forest directory
    let fdir = forest_dir(&tmpl.worktree_base, &forest_name);

    // Create worktree_base if it doesn't exist (match discover_forests leniency)
    if !tmpl.worktree_base.exists() {
        std::fs::create_dir_all(&tmpl.worktree_base)?;
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
        crate::forest::find_forest(&tmpl.worktree_base, forest_name.as_str())?
    {
        bail!(
            "forest name {:?} collides with existing forest {:?} at {}\n  hint: choose a different name",
            forest_name.as_str(),
            existing_meta.name.as_str(),
            existing_dir.display()
        );
    }

    // Validate source repos exist and branch names
    for repo in &tmpl.repos {
        if !repo.path.is_dir() {
            bail!(
                "source repo not found: {}\n  hint: check that the path exists, or update config with `git forest init --force`",
                repo.path.display()
            );
        }
    }

    // Validate all branch names (global override, per-repo overrides)
    if let Some(ref branch) = inputs.branch_override {
        // Validate against all remotes — use first repo's remote as representative
        if let Some(repo) = tmpl.repos.first() {
            BranchName::new(branch.clone(), &repo.remote)?;
        }
    }
    for (repo_name, branch) in &inputs.repo_branches {
        let remote = tmpl
            .repos
            .iter()
            .find(|r| r.name.as_str() == repo_name.as_str())
            .map(|r| r.remote.as_str())
            .unwrap_or("origin");
        BranchName::new(branch.clone(), remote)?;
    }

    // Build repo plans
    let mut repo_plans = Vec::new();
    for repo in &tmpl.repos {
        let branch_str = compute_target_branch(
            repo.name.as_str(),
            forest_name.as_str(),
            &inputs.mode,
            &tmpl.feature_branch_template,
            &inputs.branch_override,
            &inputs.repo_branches,
        );

        let branch = BranchName::new(branch_str, &repo.remote)?;

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

        let dest = fdir.join(repo.name.as_str());

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
        forest_name,
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
            branch: rp.branch.to_string(),
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
    // SAFETY: create_dir (not create_dir_all) is intentional. It fails atomically
    // if the directory already exists, preventing a TOCTOU race between plan_forest
    // (which checks for collisions) and execution. Do not change to create_dir_all.
    std::fs::create_dir(&plan.forest_dir)?;

    // Write initial meta with empty repos
    let mut meta = ForestMeta {
        name: plan.forest_name.clone(),
        created_at: Utc::now(),
        mode: plan.mode.clone(),
        repos: vec![],
    };
    // RepoMeta.branch is String, not BranchName, so we use to_string() below
    let meta_path = plan.forest_dir.join(META_FILENAME);
    meta.write(&meta_path)?;

    // Create worktrees incrementally, tracking successes for rollback
    let mut created_worktrees: Vec<(&AbsolutePath, &AbsolutePath)> = Vec::new();

    for repo_plan in &plan.repo_plans {
        let dest_str = repo_plan.dest.to_string_lossy();

        let branch_str = repo_plan.branch.as_str();
        let result = match &repo_plan.checkout {
            CheckoutKind::ExistingLocal => crate::git::git(
                &repo_plan.source,
                &["worktree", "add", &dest_str, branch_str],
            ),
            CheckoutKind::TrackRemote => {
                let start = format!("{}/{}", repo_plan.remote, branch_str);
                crate::git::git(
                    &repo_plan.source,
                    &["worktree", "add", &dest_str, "-b", branch_str, &start],
                )
            }
            CheckoutKind::NewBranch => {
                let start = format!("{}/{}", repo_plan.remote, repo_plan.base_branch);
                // Canonical arg order (options before <path> <commit-ish>) for
                // compatibility across git versions.
                crate::git::git(
                    &repo_plan.source,
                    &[
                        "worktree",
                        "add",
                        "-b",
                        branch_str,
                        "--no-track",
                        &dest_str,
                        &start,
                    ],
                )
            }
        };

        match result {
            Ok(_) => {
                created_worktrees.push((&repo_plan.source, &repo_plan.dest));

                // Update meta incrementally
                meta.repos.push(RepoMeta {
                    name: repo_plan.name.clone(),
                    source: repo_plan.source.clone(),
                    branch: repo_plan.branch.to_string(),
                    base_branch: repo_plan.base_branch.clone(),
                    branch_created: branch_created(&repo_plan.checkout),
                });
                meta.write(&meta_path)?;
            }
            Err(e) => {
                // Rollback: remove successfully-created worktrees
                for (source, dest) in &created_worktrees {
                    assert!(
                        dest.starts_with(&plan.forest_dir),
                        "rollback target {:?} is not inside forest dir {:?}",
                        dest,
                        plan.forest_dir
                    );
                    let d = dest.to_string_lossy();
                    let _ = crate::git::git(source, &["worktree", "remove", "--force", &d]);
                }
                let _ = std::fs::remove_dir_all(&plan.forest_dir);
                return Err(e);
            }
        }
    }

    Ok(plan_to_result(plan, false))
}

pub fn cmd_new(inputs: NewInputs, tmpl: &ResolvedTemplate) -> Result<NewResult> {
    // Fetch unless --no-fetch
    if !inputs.no_fetch {
        for repo in &tmpl.repos {
            if repo.path.is_dir() {
                crate::git::git(&repo.path, &["fetch", &repo.remote])?;
            }
        }
    }

    let plan = plan_forest(&inputs, tmpl)?;

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
        result.forest_name.as_str(),
        result.mode
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cmd_ls;
    use crate::config::{ResolvedRepo, ResolvedTemplate};
    use crate::paths::AbsolutePath;
    use crate::testutil::TestEnv;
    use std::path::PathBuf;

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

    fn make_template_with_repos(env: &TestEnv, repo_names: &[&str]) -> ResolvedTemplate {
        env.default_template(repo_names)
    }

    // --- Branch computation ---

    #[test]
    fn feature_mode_uses_feature_branch_template() {
        let branch = compute_target_branch(
            "foo-api",
            "java-84/refactor-auth",
            &ForestMode::Feature,
            "dliv/{name}",
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
            "dliv/{name}",
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
            "dliv/{name}",
            &override_branch,
            &[],
        );
        let b2 = compute_target_branch(
            "foo-web",
            "test",
            &ForestMode::Feature,
            "dliv/{name}",
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
            "dliv/{name}",
            &None,
            &repo_branches,
        );
        let b2 = compute_target_branch(
            "foo-web",
            "review-pr",
            &ForestMode::Review,
            "dliv/{name}",
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);
        let mut inputs = make_new_inputs("", ForestMode::Feature);
        inputs.no_fetch = true;

        let result = plan_forest(&inputs, &tmpl);
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        for name in &[".", ".."] {
            let inputs = make_new_inputs(name, ForestMode::Feature);
            let result = plan_forest(&inputs, &tmpl);
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        // Pre-create the forest directory
        let fname = ForestName::new("my-feature".to_string()).unwrap();
        let fdir = forest_dir(&tmpl.worktree_base, &fname);
        std::fs::create_dir_all(&fdir).unwrap();

        let inputs = make_new_inputs("my-feature", ForestMode::Feature);
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn plan_empty_config_repos_errors() {
        let env = TestEnv::new();
        let tmpl = ResolvedTemplate {
            worktree_base: env.worktree_base(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![],
        };

        let inputs = make_new_inputs("test", ForestMode::Feature);
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no repos configured"));
    }

    #[test]
    fn plan_source_repo_missing_errors() {
        let env = TestEnv::new();
        let tmpl = ResolvedTemplate {
            worktree_base: env.worktree_base(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![crate::config::ResolvedRepo {
                path: AbsolutePath::new(PathBuf::from("/nonexistent/repo")).unwrap(),
                name: RepoName::new("missing".to_string()).unwrap(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            }],
        };

        let inputs = make_new_inputs("test", ForestMode::Feature);
        let result = plan_forest(&inputs, &tmpl);
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.repo_branches = vec![("nonexistent".to_string(), "branch".to_string())];

        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown repo"), "error: {}", err);
        assert!(err.contains("foo-api"), "should list known repos: {}", err);
    }

    #[test]
    fn duplicate_repo_branch_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.repo_branches = vec![
            ("foo-api".to_string(), "branch-a".to_string()),
            ("foo-api".to_string(), "branch-b".to_string()),
        ];

        let result = plan_forest(&inputs, &tmpl);
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.branch_override = Some("refs/heads/my-branch".to_string());

        let result = plan_forest(&inputs, &tmpl);
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("test", ForestMode::Feature);
        inputs.branch_override = Some("origin/my-branch".to_string());

        let result = plan_forest(&inputs, &tmpl);
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
        let mut tmpl = make_template_with_repos(&env, &["foo-api"]);
        tmpl.repos[0].base_branch = "dev".to_string();

        let inputs = make_new_inputs("test", ForestMode::Feature);
        let result = plan_forest(&inputs, &tmpl);
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

        let tmpl = make_template_with_repos(&env, &["foo-api"]);
        let inputs = make_new_inputs("my-feature", ForestMode::Feature);

        let plan = plan_forest(&inputs, &tmpl).unwrap();
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

        let tmpl = make_template_with_repos(&env, &["foo-api"]);
        let inputs = make_new_inputs("my-feature", ForestMode::Feature);

        let plan = plan_forest(&inputs, &tmpl).unwrap();
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("brand-new-feature", ForestMode::Feature);

        let plan = plan_forest(&inputs, &tmpl).unwrap();
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
        let tmpl = make_template_with_repos(&env, &["foo-api", "foo-web"]);

        let inputs = make_new_inputs("java-84/refactor-auth", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();

        assert_eq!(plan.forest_name.as_str(), "java-84/refactor-auth");
        assert_eq!(plan.mode, ForestMode::Feature);
        assert_eq!(plan.repo_plans.len(), 2);
        assert_eq!(
            plan.repo_plans[0].branch.as_str(),
            "testuser/java-84/refactor-auth"
        );
        assert_eq!(
            plan.repo_plans[1].branch.as_str(),
            "testuser/java-84/refactor-auth"
        );
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
        let tmpl = make_template_with_repos(&env, &["foo-api", "foo-web"]);

        let mut inputs = make_new_inputs("review-sues-dialog", ForestMode::Review);
        inputs.repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];

        let plan = plan_forest(&inputs, &tmpl).unwrap();

        assert_eq!(plan.repo_plans.len(), 2);
        // foo-api gets the default review branch
        assert_eq!(
            plan.repo_plans[0].branch.as_str(),
            "forest/review-sues-dialog"
        );
        // foo-web gets the exception branch
        assert_eq!(plan.repo_plans[1].branch.as_str(), "sue/fix-dialog");
    }

    // --- execute_plan ---

    #[test]
    fn execute_creates_forest_dir_and_worktrees() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        env.create_repo_with_remote("foo-web");
        let tmpl = make_template_with_repos(&env, &["foo-api", "foo-web"]);

        let inputs = make_new_inputs("exec-test", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();
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
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("meta-test", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();
        execute_plan(&plan).unwrap();

        let meta_path = plan.forest_dir.join(META_FILENAME);
        let meta = ForestMeta::read(&meta_path).unwrap();

        assert_eq!(meta.name.as_str(), "meta-test");
        assert_eq!(meta.mode, ForestMode::Feature);
        assert_eq!(meta.repos.len(), 1);
        assert_eq!(meta.repos[0].name.as_str(), "foo-api");
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
        let tmpl = make_template_with_repos(&env, &["foo-api", "foo-web"]);

        let inputs = make_new_inputs("e2e-feature", ForestMode::Feature);
        let result = cmd_new(inputs, &tmpl).unwrap();

        assert_eq!(result.forest_name.as_str(), "e2e-feature");
        assert_eq!(result.mode, ForestMode::Feature);
        assert!(!result.dry_run);
        assert_eq!(result.repos.len(), 2);
        assert_eq!(result.repos[0].branch, "testuser/e2e-feature");
        assert!(result.repos[0].branch_created);
        assert!(result.forest_dir.exists());
    }

    #[test]
    fn feature_branch_has_no_upstream_tracking() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("no-track-test", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let worktree = env.worktree_base().join("no-track-test").join("foo-api");
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "@{u}"])
            .current_dir(worktree.as_ref())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "feature branch should have no upstream, but got: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn review_branch_tracks_remote() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        // Push a branch to the remote so TrackRemote can find it
        let repo_path = env.repo_path("foo-api");
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(repo_path.as_ref())
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {:?} failed", args);
        };
        run(&["checkout", "-b", "sue/fix-dialog"]);
        run(&["commit", "--allow-empty", "-m", "feature"]);
        run(&["push", "origin", "sue/fix-dialog"]);
        run(&["checkout", "main"]);
        run(&["branch", "-D", "sue/fix-dialog"]);
        run(&["fetch", "origin"]);

        let mut inputs = make_new_inputs("track-test", ForestMode::Review);
        inputs.repo_branches = vec![("foo-api".to_string(), "sue/fix-dialog".to_string())];
        cmd_new(inputs, &tmpl).unwrap();

        let worktree = env.worktree_base().join("track-test").join("foo-api");
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "@{u}"])
            .current_dir(worktree.as_ref())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "review branch should track remote upstream"
        );
        let upstream = String::from_utf8_lossy(&output.stdout);
        assert!(
            upstream.trim() == "origin/sue/fix-dialog",
            "expected origin/sue/fix-dialog, got: {}",
            upstream.trim()
        );
    }

    #[test]
    fn cmd_new_dry_run_does_not_create() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("dry-run-test", ForestMode::Feature);
        inputs.dry_run = true;

        let result = cmd_new(inputs, &tmpl).unwrap();

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
        let tmpl = make_template_with_repos(&env, &["foo-api", "foo-web"]);

        let mut inputs = make_new_inputs("review-test", ForestMode::Review);
        inputs.repo_branches = vec![("foo-web".to_string(), "sue/fix-dialog".to_string())];

        let result = cmd_new(inputs, &tmpl).unwrap();

        assert_eq!(result.repos.len(), 2);
        assert_eq!(result.repos[0].branch, "forest/review-test");
        assert_eq!(result.repos[1].branch, "sue/fix-dialog");
    }

    #[test]
    fn cmd_new_duplicate_forest_name_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        // First create succeeds
        let inputs = make_new_inputs("dup-test", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        // Second create should fail
        let inputs2 = make_new_inputs("dup-test", ForestMode::Feature);
        let result = cmd_new(inputs2, &tmpl);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn new_with_explicit_template() {
        let env = TestEnv::new();
        env.create_repo_with_remote("alpha-api");
        env.create_repo_with_remote("beta-api");

        // Template "alpha" has alpha-api, template "beta" has beta-api
        let tmpl_alpha = env.default_template(&["alpha-api"]);

        let tmpl_beta = ResolvedTemplate {
            worktree_base: env.worktree_base(),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            repos: vec![ResolvedRepo {
                path: env.repo_path("beta-api"),
                name: RepoName::new("beta-api".to_string()).unwrap(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            }],
        };

        // Using tmpl_alpha creates worktrees only for alpha-api
        let inputs = make_new_inputs("alpha-feature", ForestMode::Feature);
        let result = cmd_new(inputs, &tmpl_alpha).unwrap();
        assert_eq!(result.repos.len(), 1);
        assert_eq!(result.repos[0].name.as_str(), "alpha-api");

        // Using tmpl_beta creates worktrees only for beta-api
        let inputs = make_new_inputs("beta-feature", ForestMode::Feature);
        let result = cmd_new(inputs, &tmpl_beta).unwrap();
        assert_eq!(result.repos.len(), 1);
        assert_eq!(result.repos[0].name.as_str(), "beta-api");
    }

    #[test]
    fn ls_shows_new_forest() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("ls-test", ForestMode::Feature);
        cmd_new(inputs, &tmpl).unwrap();

        let ls_result = cmd_ls(&[tmpl.worktree_base.as_ref()]).unwrap();
        assert_eq!(ls_result.forests.len(), 1);
        assert_eq!(ls_result.forests[0].name.as_str(), "ls-test");
        assert_eq!(ls_result.forests[0].mode, ForestMode::Feature);
    }

    #[test]
    fn plan_forest_name_all_slashes_works() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        // "////" sanitizes to "----" which is valid
        let inputs = make_new_inputs("////", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();
        assert_eq!(plan.forest_name.as_str(), "////");
        assert!(plan.forest_dir.to_string_lossy().contains("----"));
    }

    #[test]
    fn plan_forest_name_with_spaces_works() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("my feature", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();
        assert_eq!(plan.forest_name.as_str(), "my feature");
    }

    // --- Bug reproduction tests ---

    /// When execute_plan fails partway through (e.g., second repo's worktree
    /// add fails), the forest directory should be cleaned up so a retry with
    /// the same name doesn't hit "forest directory already exists".
    #[test]
    fn execute_plan_cleans_up_forest_dir_on_failure() {
        let env = TestEnv::new();
        env.create_repo_with_remote("good-repo");
        env.create_repo_with_remote("bad-repo");
        let tmpl = make_template_with_repos(&env, &["good-repo", "bad-repo"]);

        let inputs = make_new_inputs("cleanup-test", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();

        // Sabotage: remove the second repo's source so git worktree add fails.
        // (Don't pre-create the forest dir — execute_plan needs to create it.)
        let bad_source = &plan.repo_plans[1].source;
        std::fs::remove_dir_all(&**bad_source).unwrap();

        // execute_plan should fail on the second repo
        let result = execute_plan(&plan);
        assert!(result.is_err(), "execute_plan should fail");

        // The forest directory should have been cleaned up
        assert!(
            !plan.forest_dir.exists(),
            "forest directory should be cleaned up after failed execute_plan, but exists at: {}",
            plan.forest_dir.display()
        );

        // The first repo's worktree was successfully created before the failure.
        // Its git worktree registration should also be cleaned up, otherwise
        // a retry would hit "missing but already registered worktree".
        let good_source = env.repo_path("good-repo");
        let wt_list = crate::git::git(&good_source, &["worktree", "list"]).unwrap();
        assert!(
            !wt_list.contains("cleanup-test"),
            "first repo's worktree registration should be cleaned up after failed execute_plan, but got: {}",
            wt_list
        );
    }
}
