use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};

use crate::config::ResolvedTemplate;
use crate::forest::write_meta;
use crate::git::ref_exists;
use git_forest_core::meta::{ForestMeta, RepoMeta, META_FILENAME};
use git_forest_core::paths::{forest_dir, AbsolutePath, ForestName};
pub(crate) use git_forest_core::plan::new::{branch_created, plan_to_result};
use git_forest_core::plan::new::{
    decide_forest, target_branches, validate_new_inputs, ForestCollision, NewObservation,
    RefObservation, RepoObservation,
};
pub use git_forest_core::plan::new::{
    format_new_human, CheckoutKind, ForestPlan, NewInputs, NewResult, RepoPlan,
};

/// Reads what planning a new forest needs once the inputs are valid: whether the forest
/// directory exists, the forest the name resolves to, and each template repo's source and the
/// refs of its target branch. Read-only; every read error is kept for `decide_forest`.
pub fn observe_new(
    inputs: &NewInputs,
    tmpl: &ResolvedTemplate,
    forest_name: &ForestName,
) -> NewObservation {
    let fdir = forest_dir(&tmpl.worktree_base, forest_name);
    let collision =
        crate::forest::find_forest(&tmpl.worktree_base, forest_name.as_str()).map(|found| {
            found.map(|(dir, meta)| ForestCollision {
                dir,
                name: meta.name,
            })
        });
    let repos = tmpl
        .repos
        .iter()
        .zip(target_branches(inputs, tmpl, forest_name))
        .map(|(repo, branch)| RepoObservation {
            source_is_dir: repo.path.is_dir(),
            refs: branch.ok().map(|branch| RefObservation {
                local_branch: ref_exists(&repo.path, &format!("refs/heads/{}", branch)),
                remote_branch: ref_exists(
                    &repo.path,
                    &format!("refs/remotes/{}/{}", repo.remote, branch),
                ),
                remote_base: ref_exists(
                    &repo.path,
                    &format!("refs/remotes/{}/{}", repo.remote, repo.base_branch),
                ),
            }),
        })
        .collect();
    NewObservation {
        forest_dir_exists: fdir.exists(),
        collision,
        repos,
    }
}

/// Plans a new forest: validates the inputs, observes the world, then decides.
pub fn plan_forest(inputs: &NewInputs, tmpl: &ResolvedTemplate) -> Result<ForestPlan> {
    let forest_name = validate_new_inputs(inputs, tmpl)?;
    let observation = observe_new(inputs, tmpl, &forest_name);
    decide_forest(inputs, tmpl, forest_name, observation)
}

/// A worktree this run added, and the branch its add created, if any.
struct CreatedWorktree<'a> {
    source: &'a AbsolutePath,
    dest: &'a AbsolutePath,
    branch: Option<CreatedBranch<'a>>,
}

/// A branch this run's `git worktree add -b` created, with the commit it was created at.
struct CreatedBranch<'a> {
    name: &'a str,
    commit: String,
}

/// The commit `start` names in `source` now.
fn resolve_commit(source: &AbsolutePath, start: &str) -> Result<String> {
    crate::git::git(
        source,
        &["rev-parse", "--verify", &format!("{}^{{commit}}", start)],
    )
}

/// Adds one repo's worktree. A branch the add creates starts at the commit its start point
/// names when execution resolves it, and is recorded with that commit only after the add
/// succeeds, so rollback never touches a branch another operation created.
fn add_worktree<'a>(repo_plan: &'a RepoPlan, created: &mut Vec<CreatedWorktree<'a>>) -> Result<()> {
    let source = &repo_plan.source;
    let dest_str = repo_plan.dest.to_string_lossy();
    let branch = repo_plan.branch.as_str();
    // Canonical arg order (options before <path> <commit-ish>) for compatibility across git
    // versions.
    match &repo_plan.checkout {
        CheckoutKind::ExistingLocal => {
            crate::git::git(source, &["worktree", "add", &dest_str, branch])?;
            created.push(CreatedWorktree {
                source,
                dest: &repo_plan.dest,
                branch: None,
            });
        }
        CheckoutKind::TrackRemote => {
            let upstream = format!("{}/{}", repo_plan.remote, branch);
            let commit = resolve_commit(source, &upstream)?;
            crate::git::git(
                source,
                &["worktree", "add", "-b", branch, &dest_str, &commit],
            )?;
            created.push(CreatedWorktree {
                source,
                dest: &repo_plan.dest,
                branch: Some(CreatedBranch {
                    name: branch,
                    commit,
                }),
            });
            let set_upstream = format!("--set-upstream-to={}", upstream);
            crate::git::git(source, &["branch", &set_upstream, branch])?;
        }
        CheckoutKind::NewBranch => {
            let start = format!("{}/{}", repo_plan.remote, repo_plan.base_branch);
            let commit = resolve_commit(source, &start)?;
            crate::git::git(
                source,
                &[
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    "--no-track",
                    &dest_str,
                    &commit,
                ],
            )?;
            created.push(CreatedWorktree {
                source,
                dest: &repo_plan.dest,
                branch: Some(CreatedBranch {
                    name: branch,
                    commit,
                }),
            });
        }
    }
    Ok(())
}

/// Undoes a failed run: removes the worktrees it added, then deletes each branch it created
/// whose worktree was removed, only if the branch still points at the commit it was created
/// at (`git update-ref -d <ref> <commit>` compares and deletes atomically), then removes the
/// forest directory. Rollback errors are ignored; the caller returns the original error.
fn roll_back(plan: &ForestPlan, created: &[CreatedWorktree]) {
    let mut removed = Vec::new();
    for worktree in created {
        assert!(
            worktree.dest.starts_with(&plan.forest_dir),
            "rollback target {:?} is not inside forest dir {:?}",
            worktree.dest,
            plan.forest_dir
        );
        let dest = worktree.dest.to_string_lossy();
        if crate::git::git(worktree.source, &["worktree", "remove", "--force", &dest]).is_ok() {
            removed.push(worktree);
        }
    }
    for worktree in removed {
        if let Some(branch) = &worktree.branch {
            let refname = format!("refs/heads/{}", branch.name);
            let _ = crate::git::git(
                worktree.source,
                &["update-ref", "-d", &refname, &branch.commit],
            );
        }
    }
    let _ = std::fs::remove_dir_all(&plan.forest_dir);
}

pub fn execute_plan(plan: &ForestPlan, now: DateTime<Utc>) -> Result<NewResult> {
    // The worktree base is created here, not while planning, so a dry-run writes nothing.
    // A missing base plans like an empty one (match discover_forests leniency).
    if let Some(base) = plan.forest_dir.parent() {
        std::fs::create_dir_all(base)?;
    }

    // SAFETY: create_dir (not create_dir_all) is intentional. It fails atomically
    // if the directory already exists, preventing a TOCTOU race between plan_forest
    // (which checks for collisions) and execution. Do not change to create_dir_all.
    std::fs::create_dir(&plan.forest_dir)?;

    // Write initial meta with empty repos
    let mut meta = ForestMeta {
        name: plan.forest_name.clone(),
        created_at: now,
        mode: plan.mode.clone(),
        disposable_root_entries: plan.disposable_root_entries.clone(),
        repos: vec![],
    };
    // RepoMeta.branch is String, not BranchName, so we use to_string() below
    let meta_path = plan.forest_dir.join(META_FILENAME);
    write_meta(&meta, &meta_path)?;

    // Create worktrees incrementally, recording what this run created for rollback
    let mut created: Vec<CreatedWorktree> = Vec::new();

    for repo_plan in &plan.repo_plans {
        if let Err(e) = add_worktree(repo_plan, &mut created) {
            roll_back(plan, &created);
            return Err(e);
        }

        // Update meta incrementally
        meta.repos.push(RepoMeta {
            name: repo_plan.name.clone(),
            source: repo_plan.source.clone(),
            branch: repo_plan.branch.to_string(),
            base_branch: repo_plan.base_branch.clone(),
            remote: Some(repo_plan.remote.clone()),
            branch_created: branch_created(&repo_plan.checkout),
        });
        write_meta(&meta, &meta_path)?;
    }

    Ok(plan_to_result(plan, false))
}

/// `cmd_new_at` with the current time, for tests.
#[cfg(test)]
pub fn cmd_new(inputs: NewInputs, tmpl: &ResolvedTemplate) -> Result<NewResult> {
    cmd_new_at(inputs, tmpl, Utc::now())
}

/// Creates the forest `inputs` names from `tmpl`, recording `now` as its creation time.
pub fn cmd_new_at(
    inputs: NewInputs,
    tmpl: &ResolvedTemplate,
    now: DateTime<Utc>,
) -> Result<NewResult> {
    // Fetch unless --no-fetch
    if !inputs.no_fetch {
        for repo in &tmpl.repos {
            if repo.path.is_dir() {
                crate::git::git(&repo.path, &["fetch", &repo.remote]).map_err(|error| {
                    anyhow!(
                        "{:#}\n  hint: check that the remote is reachable, or pass `--no-fetch` to plan from the refs already fetched",
                        error
                    )
                })?;
            }
        }
    }

    let plan = plan_forest(&inputs, tmpl)?;

    if inputs.dry_run {
        return Ok(plan_to_result(&plan, true));
    }

    execute_plan(&plan, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cmd_ls;
    use crate::config::ResolvedTemplate;
    use crate::forest::read_meta;
    use crate::testutil::TestEnv;
    use git_forest_core::meta::ForestMode;
    use git_forest_core::paths::{AbsolutePath, DisposableRootEntry, RepoName};
    use git_forest_core::plan::new::compute_target_branch;
    use git_forest_core::plan::new::ResolvedRepo;
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
            disposable_root_entries: vec![],
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
            disposable_root_entries: vec![],
            repos: vec![ResolvedRepo {
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

    // --- Base-branch collision guard ---

    #[test]
    fn plan_branch_matches_base_branch_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        // Template uses "{name}" so forest name "main" → branch "main" = base_branch
        let mut tmpl = make_template_with_repos(&env, &["foo-api"]);
        tmpl.feature_branch_template = "{name}".to_string();

        let inputs = make_new_inputs("main", ForestMode::Feature);
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("matches its base branch"), "error: {}", err);
    }

    #[test]
    fn plan_branch_with_prefix_does_not_collide() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        // Template uses "dliv/{name}" so forest name "main" → branch "dliv/main" ≠ "main"
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let inputs = make_new_inputs("main", ForestMode::Feature);
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_ok(), "unexpected error: {:?}", result.err());
    }

    #[test]
    fn plan_branch_override_matches_base_branch_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("something", ForestMode::Feature);
        inputs.branch_override = Some("main".to_string());
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("matches its base branch"), "error: {}", err);
    }

    #[test]
    fn plan_repo_branch_override_matches_base_branch_errors() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        let mut inputs = make_new_inputs("something", ForestMode::Feature);
        inputs.repo_branches = vec![("foo-api".to_string(), "main".to_string())];
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("matches its base branch"), "error: {}", err);
    }

    #[test]
    fn plan_review_mode_does_not_collide_with_typical_base() {
        let env = TestEnv::new();
        env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);

        // Review mode → branch "forest/main", base is "main" → no collision
        let inputs = make_new_inputs("main", ForestMode::Review);
        let result = plan_forest(&inputs, &tmpl);
        assert!(result.is_ok(), "unexpected error: {:?}", result.err());
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
        let result = execute_plan(&plan, chrono::Utc::now()).unwrap();

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
        let mut tmpl = make_template_with_repos(&env, &["foo-api"]);
        tmpl.disposable_root_entries = vec![DisposableRootEntry::new(".idea".to_string()).unwrap()];

        let inputs = make_new_inputs("meta-test", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();
        let result = execute_plan(&plan, chrono::Utc::now()).unwrap();

        let meta_path = plan.forest_dir.join(META_FILENAME);
        let meta = read_meta(&meta_path).unwrap();

        assert_eq!(meta.name.as_str(), "meta-test");
        assert_eq!(meta.mode, ForestMode::Feature);
        assert_eq!(meta.disposable_root_entries, tmpl.disposable_root_entries);
        assert_eq!(result.disposable_root_entries, tmpl.disposable_root_entries);
        assert_eq!(meta.repos.len(), 1);
        assert_eq!(meta.repos[0].name.as_str(), "foo-api");
        assert_eq!(meta.repos[0].branch, "testuser/meta-test");
        assert_eq!(meta.repos[0].base_branch, "main");
        assert_eq!(meta.repos[0].remote.as_deref(), Some("origin"));
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
            disposable_root_entries: vec![],
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

        let ls_result = cmd_ls(&[tmpl.worktree_base.as_ref()], chrono::Utc::now()).unwrap();
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
        let result = execute_plan(&plan, chrono::Utc::now());
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

    #[test]
    fn cmd_new_fetch_failure_hints_at_no_fetch() {
        let env = TestEnv::new();
        let source = env.create_repo_with_remote("foo-api");
        let missing = env.root().join("missing-remote.git");
        crate::git::git(
            &source,
            &["remote", "set-url", "origin", missing.to_str().unwrap()],
        )
        .unwrap();
        let tmpl = make_template_with_repos(&env, &["foo-api"]);
        let mut inputs = make_new_inputs("fetch-fails", ForestMode::Feature);
        inputs.no_fetch = false;

        let text = format!("{:#}", cmd_new(inputs, &tmpl).unwrap_err());
        assert!(text.starts_with("git fetch origin failed in"), "{text}");
        assert!(
            text.ends_with(
                "\n  hint: check that the remote is reachable, or pass `--no-fetch` to plan from the refs already fetched"
            ),
            "{text}"
        );
    }

    /// A failed `new` deletes the branches it created, so a retry does not find a leftover
    /// branch.
    #[test]
    fn execute_plan_rollback_deletes_branches_it_created() {
        let env = TestEnv::new();
        env.create_repo_with_remote("good-repo");
        env.create_repo_with_remote("bad-repo");
        let tmpl = make_template_with_repos(&env, &["good-repo", "bad-repo"]);

        let inputs = make_new_inputs("rollback-branches", ForestMode::Feature);
        let plan = plan_forest(&inputs, &tmpl).unwrap();
        let branch = plan.repo_plans[0].branch.to_string();

        // Sabotage: remove the second repo's source so git worktree add fails.
        let bad_source = &plan.repo_plans[1].source;
        std::fs::remove_dir_all(&**bad_source).unwrap();

        let result = execute_plan(&plan, chrono::Utc::now());
        assert!(result.is_err(), "execute_plan should fail");

        let good_source = env.repo_path("good-repo");
        assert!(
            !ref_exists(&good_source, &format!("refs/heads/{}", branch)).unwrap(),
            "the branch {branch} that the failed run created should be deleted"
        );
    }

    // --- Rollback preserves branches this run did not create or that moved ---

    /// Creates `branch` in `source` at a new commit that no other ref contains, as another
    /// operation would, and returns the commit.
    fn create_branch_with_unique_commit(
        source: &std::path::Path,
        branch: &str,
        subject: &str,
    ) -> String {
        let commit = crate::git::git(
            source,
            &[
                "-c",
                "user.name=Other",
                "-c",
                "user.email=other@example.invalid",
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "HEAD",
                "-m",
                subject,
            ],
        )
        .unwrap();
        crate::git::git(source, &["branch", branch, &commit]).unwrap();
        commit
    }

    /// Fails unless `branch` exists in `source` and contains `commit`.
    fn assert_branch_contains(source: &std::path::Path, branch: &str, commit: &str) {
        let refname = format!("refs/heads/{}", branch);
        assert!(
            ref_exists(source, &refname).unwrap(),
            "branch {branch} should survive the rollback"
        );
        assert!(
            crate::git::git(source, &["merge-base", "--is-ancestor", commit, &refname]).is_ok(),
            "branch {branch} should still contain {commit}"
        );
    }

    #[test]
    fn rollback_keeps_a_planned_new_branch_another_operation_created() {
        let env = TestEnv::new();
        let source = env.create_repo_with_remote("foo-api");
        let tmpl = make_template_with_repos(&env, &["foo-api"]);
        let plan = plan_forest(&make_new_inputs("race", ForestMode::Feature), &tmpl).unwrap();
        assert!(matches!(
            plan.repo_plans[0].checkout,
            CheckoutKind::NewBranch
        ));
        let branch = plan.repo_plans[0].branch.to_string();

        // After planning, another operation creates the planned branch with its own commit.
        let unique = create_branch_with_unique_commit(&source, &branch, "unique-concurrent-work");

        assert!(execute_plan(&plan, chrono::Utc::now()).is_err());
        assert_branch_contains(&source, &branch, &unique);
    }

    #[test]
    fn rollback_keeps_a_planned_tracking_branch_another_operation_created() {
        let env = TestEnv::new();
        let source = env.create_repo_with_remote("foo-api");
        crate::git::git(&source, &["branch", "testuser/race"]).unwrap();
        crate::git::git(&source, &["push", "origin", "testuser/race"]).unwrap();
        crate::git::git(&source, &["branch", "-D", "testuser/race"]).unwrap();
        crate::git::git(&source, &["fetch", "origin"]).unwrap();
        let tmpl = make_template_with_repos(&env, &["foo-api"]);
        let plan = plan_forest(&make_new_inputs("race", ForestMode::Feature), &tmpl).unwrap();
        assert!(matches!(
            plan.repo_plans[0].checkout,
            CheckoutKind::TrackRemote
        ));
        let branch = plan.repo_plans[0].branch.to_string();

        // After planning, another operation creates the local branch with its own commit.
        let unique = create_branch_with_unique_commit(&source, &branch, "unique-concurrent-work");

        assert!(execute_plan(&plan, chrono::Utc::now()).is_err());
        assert_branch_contains(&source, &branch, &unique);
    }

    #[test]
    fn rollback_keeps_a_created_branch_that_moved_during_the_run() {
        let env = TestEnv::new();
        let first = env.create_repo_with_remote("first-repo");
        env.create_repo_with_remote("second-repo");
        let tmpl = make_template_with_repos(&env, &["first-repo", "second-repo"]);
        let plan = plan_forest(&make_new_inputs("advanced", ForestMode::Feature), &tmpl).unwrap();
        let branch = plan.repo_plans[0].branch.to_string();

        // A post-checkout hook in the first source repo commits in its new worktree, so the
        // branch this run created moves past its start before the run fails.
        let hooks = env.root().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(
            &hook,
            "#!/bin/sh\ngit -c user.name=Hook -c user.email=hook@example.invalid commit --allow-empty -q -m unique-during-new\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        crate::git::git(
            &first,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        )
        .unwrap();

        // The second repo's add fails.
        std::fs::remove_dir_all(&*plan.repo_plans[1].source).unwrap();

        assert!(execute_plan(&plan, chrono::Utc::now()).is_err());
        let refname = format!("refs/heads/{}", branch);
        assert!(
            ref_exists(&first, &refname).unwrap(),
            "the moved branch {branch} should survive the rollback"
        );
        let subject = crate::git::git(&first, &["log", "-1", "--format=%s", &refname]).unwrap();
        assert_eq!(subject, "unique-during-new");
    }
}
