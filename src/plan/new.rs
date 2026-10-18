//! Planning a new forest: its inputs, plan and result types, the pure decisions over what
//! `commands::new` observes, and the human renderer.

use anyhow::{bail, ensure, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::meta::ForestMode;
use crate::paths::{
    forest_dir, AbsolutePath, BranchName, DisposableRootEntry, ForestName, RepoName,
};

/// One repo of a resolved template.
#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    pub path: AbsolutePath,
    pub name: RepoName,
    pub base_branch: String,
    pub remote: String,
}

/// A template resolved from the config: where its forests go and which repos they hold.
#[derive(Debug, Clone)]
pub struct ResolvedTemplate {
    pub worktree_base: AbsolutePath,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub disposable_root_entries: Vec<DisposableRootEntry>,
    pub repos: Vec<ResolvedRepo>,
}

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
    pub disposable_root_entries: Vec<DisposableRootEntry>,
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
    pub disposable_root_entries: Vec<DisposableRootEntry>,
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

pub(crate) fn compute_target_branch(
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

pub(crate) fn branch_created(checkout: &CheckoutKind) -> bool {
    match checkout {
        CheckoutKind::ExistingLocal => false,
        CheckoutKind::TrackRemote => false,
        CheckoutKind::NewBranch => true,
    }
}

pub(crate) fn plan_to_result(plan: &ForestPlan, dry_run: bool) -> NewResult {
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
        disposable_root_entries: plan.disposable_root_entries.clone(),
        repos,
    }
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
    if !result.disposable_root_entries.is_empty() {
        lines.push(format!(
            "  Disposable root entries: {}",
            result
                .disposable_root_entries
                .iter()
                .map(DisposableRootEntry::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
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

/// What planning a new forest reads from the world, after the inputs are valid: the forest
/// directory, the forests already in the worktree base, and each template repo's source and
/// refs. Every fallible read keeps its error, which `decide_forest` raises at the point where
/// planning reaches that read.
pub struct NewObservation {
    /// Whether the forest directory already exists.
    pub forest_dir_exists: bool,
    /// The existing forest found under the requested name, or the error looking for it.
    pub collision: Result<Option<ForestCollision>>,
    /// One entry per template repo, in template order.
    pub repos: Vec<RepoObservation>,
}

/// An existing forest that the requested name resolves to.
pub struct ForestCollision {
    pub dir: PathBuf,
    pub name: ForestName,
}

/// What planning reads about one template repo.
pub struct RepoObservation {
    /// Whether the source repo's path is a directory.
    pub source_is_dir: bool,
    /// The target branch's refs, or `None` when the target branch name is invalid: planning
    /// stops at that name before it reads any ref.
    pub refs: Option<RefObservation>,
}

/// Which of the refs that choose a checkout exist in a source repo, each with its git error.
pub struct RefObservation {
    /// `refs/heads/<branch>`.
    pub local_branch: Result<bool>,
    /// `refs/remotes/<remote>/<branch>`.
    pub remote_branch: Result<bool>,
    /// `refs/remotes/<remote>/<base_branch>`.
    pub remote_base: Result<bool>,
}

/// The target branch name of every template repo, as planning computes it, in template order.
/// The observe step uses it to know which refs to read.
pub(crate) fn target_branches(
    inputs: &NewInputs,
    tmpl: &ResolvedTemplate,
    forest_name: &ForestName,
) -> Vec<Result<BranchName>> {
    tmpl.repos
        .iter()
        .map(|repo| {
            let branch = compute_target_branch(
                repo.name.as_str(),
                forest_name.as_str(),
                &inputs.mode,
                &tmpl.feature_branch_template,
                &inputs.branch_override,
                &inputs.repo_branches,
            );
            BranchName::new(branch, &repo.remote)
        })
        .collect()
}

/// The checks that need nothing but the inputs, in planning's order: the forest name, the
/// template's repos, and the `--repo-branch` keys. Returns the valid forest name.
pub fn validate_new_inputs(inputs: &NewInputs, tmpl: &ResolvedTemplate) -> Result<ForestName> {
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
            ensure!(
                seen.insert(repo_name.as_str()),
                "duplicate repo-branch for: {}\n  hint: specify each repo at most once",
                repo_name
            );
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

    Ok(forest_name)
}

/// The rest of planning, over the observation: the forest directory and name collision, each
/// source repo, the branch overrides, then per repo in template order its target branch, the
/// base-branch guard and the checkout chosen from its refs. A captured read error is raised at
/// the point where planning reaches that read.
pub fn decide_forest(
    inputs: &NewInputs,
    tmpl: &ResolvedTemplate,
    forest_name: ForestName,
    observation: NewObservation,
) -> Result<ForestPlan> {
    debug_assert_eq!(
        observation.repos.len(),
        tmpl.repos.len(),
        "the observation has one entry per template repo"
    );

    // Compute forest directory
    let fdir = forest_dir(&tmpl.worktree_base, &forest_name);

    // Check for directory/name collision
    if observation.forest_dir_exists {
        bail!(
            "forest directory already exists: {}\n  hint: choose a different name, or remove the existing forest with `git forest rm`",
            fdir.display()
        );
    }
    // Also check for name collision via meta scan
    if let Some(existing) = observation.collision? {
        bail!(
            "forest name {:?} collides with existing forest {:?} at {}\n  hint: choose a different name",
            forest_name.as_str(),
            existing.name.as_str(),
            existing.dir.display()
        );
    }

    // Validate source repos exist and branch names
    for (repo, observed) in tmpl.repos.iter().zip(&observation.repos) {
        ensure!(
            observed.source_is_dir,
            "source repo not found: {}\n  hint: check that the path exists, or update config with `git forest init --force`",
            repo.path.display()
        );
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
    for (repo, observed) in tmpl.repos.iter().zip(observation.repos) {
        let branch_str = compute_target_branch(
            repo.name.as_str(),
            forest_name.as_str(),
            &inputs.mode,
            &tmpl.feature_branch_template,
            &inputs.branch_override,
            &inputs.repo_branches,
        );

        let branch = BranchName::new(branch_str, &repo.remote)?;

        // Guard: computed branch must not collide with the repo's base branch
        if branch.as_str() == repo.base_branch {
            bail!(
                "computed branch {:?} for repo {} matches its base branch\n  \
                 hint: this would shadow the base branch and block work outside the forest; \
                 choose a different forest name, or use --branch / --repo-branch to override",
                branch.as_str(),
                repo.name,
            );
        }

        // Branch resolution
        let refs = match observed.refs {
            Some(refs) => refs,
            None => unreachable!("the refs of every repo with a valid target branch are observed"),
        };
        let checkout = if refs.local_branch? {
            CheckoutKind::ExistingLocal
        } else if refs.remote_branch? {
            CheckoutKind::TrackRemote
        } else {
            // Verify base branch exists on remote
            if !refs.remote_base? {
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
        disposable_root_entries: tmpl.disposable_root_entries.clone(),
        repo_plans,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn absolute(path: &str) -> AbsolutePath {
        AbsolutePath::new(PathBuf::from(path)).unwrap()
    }

    /// A template whose repos are `/src/<name>` with remote `origin` and base branch `main`.
    fn template(repo_names: &[&str]) -> ResolvedTemplate {
        ResolvedTemplate {
            worktree_base: absolute("/forests"),
            base_branch: "main".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos: repo_names
                .iter()
                .map(|name| ResolvedRepo {
                    path: absolute(&format!("/src/{name}")),
                    name: RepoName::new(name.to_string()).unwrap(),
                    base_branch: "main".to_string(),
                    remote: "origin".to_string(),
                })
                .collect(),
        }
    }

    fn inputs(name: &str) -> NewInputs {
        NewInputs {
            name: name.to_string(),
            mode: ForestMode::Feature,
            branch_override: None,
            repo_branches: vec![],
            no_fetch: true,
            dry_run: false,
        }
    }

    fn repo(source_is_dir: bool, local: bool, remote: bool, base: bool) -> RepoObservation {
        RepoObservation {
            source_is_dir,
            refs: Some(RefObservation {
                local_branch: Ok(local),
                remote_branch: Ok(remote),
                remote_base: Ok(base),
            }),
        }
    }

    fn observation(repos: Vec<RepoObservation>) -> NewObservation {
        NewObservation {
            forest_dir_exists: false,
            collision: Ok(None),
            repos,
        }
    }

    fn decide(
        inputs: &NewInputs,
        tmpl: &ResolvedTemplate,
        observation: NewObservation,
    ) -> Result<ForestPlan> {
        let forest_name = validate_new_inputs(inputs, tmpl)?;
        decide_forest(inputs, tmpl, forest_name, observation)
    }

    #[test]
    fn decide_new_picks_checkout_kind_from_observed_refs() {
        let tmpl = template(&["local", "tracked", "fresh"]);
        let plan = decide(
            &inputs("feat"),
            &tmpl,
            observation(vec![
                repo(true, true, false, true),
                repo(true, false, true, true),
                repo(true, false, false, true),
            ]),
        )
        .unwrap();

        let kinds: Vec<&CheckoutKind> = plan.repo_plans.iter().map(|rp| &rp.checkout).collect();
        assert!(matches!(
            kinds.as_slice(),
            [
                CheckoutKind::ExistingLocal,
                CheckoutKind::TrackRemote,
                CheckoutKind::NewBranch
            ]
        ));
        assert_eq!(*plan.forest_dir, *PathBuf::from("/forests/feat"));
        assert_eq!(
            *plan.repo_plans[2].dest,
            *PathBuf::from("/forests/feat/fresh")
        );
        assert_eq!(plan.repo_plans[0].branch.as_str(), "testuser/feat");
    }

    #[test]
    fn decide_new_refuses_an_existing_forest_dir_or_name_collision() {
        let tmpl = template(&["foo-api"]);
        let mut existing_dir = observation(vec![repo(true, false, false, true)]);
        existing_dir.forest_dir_exists = true;
        assert_eq!(
            decide(&inputs("feat"), &tmpl, existing_dir)
                .unwrap_err()
                .to_string(),
            "forest directory already exists: /forests/feat\n  hint: choose a different name, or remove the existing forest with `git forest rm`"
        );

        let mut collision = observation(vec![repo(true, false, false, true)]);
        collision.collision = Ok(Some(ForestCollision {
            dir: PathBuf::from("/forests/elsewhere"),
            name: ForestName::new("feat".to_string()).unwrap(),
        }));
        assert_eq!(
            decide(&inputs("feat"), &tmpl, collision)
                .unwrap_err()
                .to_string(),
            "forest name \"feat\" collides with existing forest \"feat\" at /forests/elsewhere\n  hint: choose a different name"
        );
    }

    #[test]
    fn decide_new_refuses_a_missing_source_or_remote_base() {
        let tmpl = template(&["foo-api"]);
        assert_eq!(
            decide(
                &inputs("feat"),
                &tmpl,
                observation(vec![repo(false, false, false, true)])
            )
            .unwrap_err()
            .to_string(),
            "source repo not found: /src/foo-api\n  hint: check that the path exists, or update config with `git forest init --force`"
        );
        assert_eq!(
            decide(
                &inputs("feat"),
                &tmpl,
                observation(vec![repo(true, false, false, false)])
            )
            .unwrap_err()
            .to_string(),
            "origin/main not found in foo-api\n  hint: check that base_branch \"main\" exists on remote \"origin\", or run `git fetch origin` in /src/foo-api"
        );
    }

    #[test]
    fn decide_new_refuses_a_branch_that_shadows_its_base() {
        let tmpl = template(&["foo-api"]);
        let mut shadowing = inputs("feat");
        shadowing.repo_branches = vec![("foo-api".to_string(), "main".to_string())];
        assert_eq!(
            decide(
                &shadowing,
                &tmpl,
                observation(vec![repo(true, true, true, true)])
            )
            .unwrap_err()
            .to_string(),
            "computed branch \"main\" for repo foo-api matches its base branch\n  hint: this would shadow the base branch and block work outside the forest; choose a different forest name, or use --branch / --repo-branch to override"
        );
    }

    #[test]
    fn decide_new_raises_problems_in_todays_order() {
        // A git error on repo 1's local ref comes before repo 2's base-branch shadowing.
        let tmpl = template(&["foo-api", "foo-web"]);
        let mut two_problems = inputs("feat");
        two_problems.repo_branches = vec![("foo-web".to_string(), "main".to_string())];
        let git_error = "git show-ref --verify refs/heads/testuser/feat failed in /src/foo-api (exit code: 128)\nstderr: fatal: not a git repository";
        let failing_ref = RepoObservation {
            source_is_dir: true,
            refs: Some(RefObservation {
                local_branch: Err(anyhow!(git_error)),
                remote_branch: Ok(false),
                remote_base: Ok(true),
            }),
        };
        assert_eq!(
            decide(
                &two_problems,
                &tmpl,
                observation(vec![failing_ref, repo(true, false, false, true)])
            )
            .unwrap_err()
            .to_string(),
            git_error
        );

        // An existing forest directory comes before a missing source.
        let mut dir_and_source = observation(vec![
            repo(true, false, false, true),
            repo(false, false, false, true),
        ]);
        dir_and_source.forest_dir_exists = true;
        assert_eq!(
            decide(&inputs("feat"), &tmpl, dir_and_source)
                .unwrap_err()
                .to_string(),
            "forest directory already exists: /forests/feat\n  hint: choose a different name, or remove the existing forest with `git forest rm`"
        );
    }
}
