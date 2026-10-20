//! Configuring a template: the input and result types, the input checks, the pure decision over
//! what the forest ops crate's (`git-forest-ops`) `commands::init` observes of each `--repo` path,
//! the merge of the new template into the config, and the human renderer. The merge is decided
//! here; only reading and writing the config file stay in the forest ops crate.

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::config::ResolvedConfig;
use crate::meta::META_FILENAME;
use crate::paths::{validate_disposable_root_entries, AbsolutePath, DisposableRootEntry, RepoName};
use crate::plan::new::{ResolvedRepo, ResolvedTemplate};

pub struct InitInputs {
    pub template_name: String,
    pub worktree_base: String,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub disposable_root_entries: Vec<DisposableRootEntry>,
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
    pub template_name: String,
    pub worktree_base: AbsolutePath,
    pub disposable_root_entries: Vec<DisposableRootEntry>,
    pub repos: Vec<InitRepoSummary>,
}

#[derive(Debug, Serialize)]
pub struct InitRepoSummary {
    pub name: String,
    pub path: AbsolutePath,
    pub base_branch: String,
}

/// Checks what needs no observation, in order: at least one repo, a feature branch template
/// containing `{name}`, and a template name that is neither empty nor padded.
pub fn validate_init_inputs(inputs: &InitInputs) -> Result<()> {
    if inputs.repos.is_empty() {
        bail!("at least one --repo is required\n  hint: git forest init --feature-branch-template \"yourname/{{name}}\" --repo <path>");
    }

    if !inputs.feature_branch_template.contains("{name}") {
        bail!("--feature-branch-template must contain {{name}}\n  hint: use a template like \"yourname/{{name}}\"");
    }

    if inputs.template_name.trim().is_empty() {
        bail!("template name must not be empty");
    }
    if inputs.template_name != inputs.template_name.trim() {
        bail!("template name must not have leading/trailing whitespace");
    }

    Ok(())
}

/// What `init` reads once the inputs are valid: the expanded worktree base and what each
/// `--repo` path is, one entry per input in input order. Each read keeps its error for
/// `decide_init`.
#[derive(Debug)]
pub struct InitObservation {
    pub worktree_base: Result<AbsolutePath>,
    pub repos: Vec<RepoPathObservation>,
}

/// What one `--repo` path is.
#[derive(Debug)]
pub enum RepoPathObservation {
    /// The path did not expand to an absolute path.
    Unexpanded(anyhow::Error),
    /// Nothing is at the expanded path.
    Missing(AbsolutePath),
    /// Something is at the path, and git finds no repository there.
    NotRepository(AbsolutePath),
    /// git could not be started at the path.
    CheckFailed { path: AbsolutePath, error: String },
    /// The path is inside a git repository.
    Repository(AbsolutePath),
}

/// Resolves the template to add, over the observation, raising the first problem where the
/// checks reach it: the worktree base, then per repo in input order its path, its name and a
/// duplicate name, then the disposable root entries.
pub fn decide_init(inputs: &InitInputs, observation: InitObservation) -> Result<ResolvedTemplate> {
    debug_assert_eq!(
        observation.repos.len(),
        inputs.repos.len(),
        "the observation has one entry per --repo input"
    );
    let worktree_base = observation.worktree_base?;

    let mut resolved_repos = Vec::new();
    let mut names = HashSet::new();

    for (repo_input, repo_path) in inputs.repos.iter().zip(observation.repos) {
        let path = match repo_path {
            RepoPathObservation::Unexpanded(error) => return Err(error),
            RepoPathObservation::Missing(path) => bail!(
                "repo path does not exist: {}\n  hint: provide an absolute path to a git repository",
                path.display()
            ),
            RepoPathObservation::NotRepository(path) => bail!(
                "not a git repository: {}\n  hint: provide a path to a git repository",
                path.display()
            ),
            RepoPathObservation::CheckFailed { path, error } => {
                bail!("failed to check git repo at {}: {}", path.display(), error)
            }
            RepoPathObservation::Repository(path) => path,
        };

        let name_str = repo_input.name.clone().unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        let name = RepoName::new(name_str)
            .with_context(|| format!("repo has empty name (path: {})", path.display()))?;

        ensure!(
            names.insert(name.to_string()),
            "duplicate repo name: {}\n  hint: use --repo /path:custom-name to disambiguate",
            name
        );

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

    validate_disposable_root_entries(
        &inputs.disposable_root_entries,
        std::iter::once(META_FILENAME).chain(resolved_repos.iter().map(|repo| repo.name.as_str())),
    )?;

    Ok(ResolvedTemplate {
        worktree_base,
        base_branch: inputs.base_branch.clone(),
        feature_branch_template: inputs.feature_branch_template.clone(),
        disposable_root_entries: inputs.disposable_root_entries.clone(),
        repos: resolved_repos,
    })
}

/// Adds `template` under `template_name` to the `existing` config, or starts a new config whose
/// default template it is. Replacing a template of the same name requires `force`.
pub fn merge_init(
    existing: Option<ResolvedConfig>,
    template_name: &str,
    template: ResolvedTemplate,
    force: bool,
) -> Result<ResolvedConfig> {
    let mut config = existing.unwrap_or_else(|| ResolvedConfig {
        default_template: template_name.to_string(),
        templates: BTreeMap::new(),
        version_check: None,
    });

    // Only require --force when overwriting an existing template
    if config.templates.contains_key(template_name) && !force {
        bail!(
            "template {:?} already exists in config\n  hint: use --force to overwrite, or choose a different name",
            template_name
        );
    }

    config.templates.insert(template_name.to_string(), template);
    Ok(config)
}

pub fn format_init_human(result: &InitResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Config written to {}",
        result.config_path.display()
    ));
    lines.push(format!("Template: {}", result.template_name));
    lines.push(format!("Worktree base: {}", result.worktree_base.display()));
    if !result.disposable_root_entries.is_empty() {
        lines.push(format!(
            "Disposable root entries: {}",
            result
                .disposable_root_entries
                .iter()
                .map(DisposableRootEntry::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
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
    use anyhow::anyhow;

    fn abs(path: &str) -> AbsolutePath {
        AbsolutePath::new(PathBuf::from(path)).unwrap()
    }

    fn repo_input(path: &str) -> RepoInput {
        RepoInput {
            path: path.to_string(),
            name: None,
            base_branch: None,
        }
    }

    fn make_inputs(repos: Vec<RepoInput>) -> InitInputs {
        InitInputs {
            template_name: "default".to_string(),
            worktree_base: "/wt".to_string(),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos,
        }
    }

    fn observed(repos: Vec<RepoPathObservation>) -> InitObservation {
        InitObservation {
            worktree_base: Ok(abs("/wt")),
            repos,
        }
    }

    fn error_text<T: std::fmt::Debug>(result: Result<T>) -> String {
        format!("{:#}", result.unwrap_err())
    }

    fn entry(name: &str) -> DisposableRootEntry {
        DisposableRootEntry::new(name.to_string()).unwrap()
    }

    #[test]
    fn validate_init_inputs_checks_the_inputs_in_order() {
        let mut no_repos = make_inputs(vec![]);
        no_repos.feature_branch_template = "user/feature".to_string();
        no_repos.template_name = String::new();
        assert_eq!(
            error_text(validate_init_inputs(&no_repos)),
            "at least one --repo is required\n  hint: git forest init --feature-branch-template \"yourname/{name}\" --repo <path>"
        );

        let mut no_placeholder = make_inputs(vec![repo_input("/repos/foo-api")]);
        no_placeholder.feature_branch_template = "user/feature".to_string();
        no_placeholder.template_name = String::new();
        assert_eq!(
            error_text(validate_init_inputs(&no_placeholder)),
            "--feature-branch-template must contain {name}\n  hint: use a template like \"yourname/{name}\""
        );

        let mut empty_name = make_inputs(vec![repo_input("/repos/foo-api")]);
        empty_name.template_name = String::new();
        assert_eq!(
            error_text(validate_init_inputs(&empty_name)),
            "template name must not be empty"
        );

        let mut padded_name = make_inputs(vec![repo_input("/repos/foo-api")]);
        padded_name.template_name = " spaces ".to_string();
        assert_eq!(
            error_text(validate_init_inputs(&padded_name)),
            "template name must not have leading/trailing whitespace"
        );

        assert!(validate_init_inputs(&make_inputs(vec![repo_input("/repos/foo-api")])).is_ok());
    }

    #[test]
    fn decide_init_resolves_each_repo_in_input_order() {
        let mut inputs = make_inputs(vec![
            repo_input("/repos/foo-api"),
            RepoInput {
                path: "/repos/foo-web".to_string(),
                name: Some("web".to_string()),
                base_branch: None,
            },
            RepoInput {
                path: "/repos/foo-db".to_string(),
                name: None,
                base_branch: Some("release".to_string()),
            },
        ]);
        inputs.disposable_root_entries = vec![entry(".idea")];
        let observation = observed(vec![
            RepoPathObservation::Repository(abs("/repos/foo-api")),
            RepoPathObservation::Repository(abs("/repos/foo-web")),
            RepoPathObservation::Repository(abs("/repos/foo-db")),
        ]);

        let template = decide_init(&inputs, observation).unwrap();

        assert_eq!(template.worktree_base, abs("/wt"));
        assert_eq!(template.base_branch, "dev");
        assert_eq!(template.feature_branch_template, "testuser/{name}");
        assert_eq!(template.disposable_root_entries, vec![entry(".idea")]);
        let repos: Vec<(&str, &AbsolutePath, &str, &str)> = template
            .repos
            .iter()
            .map(|repo| {
                (
                    repo.name.as_str(),
                    &repo.path,
                    repo.base_branch.as_str(),
                    repo.remote.as_str(),
                )
            })
            .collect();
        assert_eq!(
            repos,
            vec![
                ("foo-api", &abs("/repos/foo-api"), "dev", "origin"),
                ("web", &abs("/repos/foo-web"), "dev", "origin"),
                ("foo-db", &abs("/repos/foo-db"), "release", "origin"),
            ]
        );
    }

    #[test]
    fn decide_init_refuses_paths_that_are_not_repositories() {
        let one_repo = make_inputs(vec![repo_input("/repos/x")]);
        assert_eq!(
            error_text(decide_init(
                &one_repo,
                observed(vec![RepoPathObservation::Missing(abs("/repos/x"))])
            )),
            "repo path does not exist: /repos/x\n  hint: provide an absolute path to a git repository"
        );
        assert_eq!(
            error_text(decide_init(
                &one_repo,
                observed(vec![RepoPathObservation::NotRepository(abs("/repos/x"))])
            )),
            "not a git repository: /repos/x\n  hint: provide a path to a git repository"
        );
        assert_eq!(
            error_text(decide_init(
                &one_repo,
                observed(vec![RepoPathObservation::CheckFailed {
                    path: abs("/repos/x"),
                    error: "spawn failed".to_string(),
                }])
            )),
            "failed to check git repo at /repos/x: spawn failed"
        );
        assert_eq!(
            error_text(decide_init(
                &make_inputs(vec![repo_input("x")]),
                observed(vec![RepoPathObservation::Unexpanded(anyhow!(
                    "path is not absolute: x"
                ))])
            )),
            "path is not absolute: x"
        );
    }

    #[test]
    fn decide_init_refuses_duplicate_and_empty_names() {
        let duplicates = make_inputs(vec![
            repo_input("/repos/a/foo-api"),
            repo_input("/repos/b/foo-api"),
        ]);
        assert_eq!(
            error_text(decide_init(
                &duplicates,
                observed(vec![
                    RepoPathObservation::Repository(abs("/repos/a/foo-api")),
                    RepoPathObservation::Repository(abs("/repos/b/foo-api")),
                ])
            )),
            "duplicate repo name: foo-api\n  hint: use --repo /path:custom-name to disambiguate"
        );

        assert_eq!(
            error_text(decide_init(
                &make_inputs(vec![repo_input("/")]),
                observed(vec![RepoPathObservation::Repository(abs("/"))])
            )),
            "repo has empty name (path: /): repo name must not be empty"
        );
    }

    #[test]
    fn decide_init_raises_the_first_problem_in_todays_order() {
        let one_repo = make_inputs(vec![repo_input("/repos/a")]);
        assert_eq!(
            error_text(decide_init(
                &one_repo,
                InitObservation {
                    worktree_base: Err(anyhow!(
                        "cannot expand ~/: HOME environment variable is not set"
                    )),
                    repos: vec![RepoPathObservation::Missing(abs("/repos/a"))],
                }
            )),
            "cannot expand ~/: HOME environment variable is not set"
        );

        let two_repos = make_inputs(vec![repo_input("/repos/a"), repo_input("/repos/b")]);
        assert_eq!(
            error_text(decide_init(
                &two_repos,
                observed(vec![
                    RepoPathObservation::NotRepository(abs("/repos/a")),
                    RepoPathObservation::Missing(abs("/repos/b")),
                ])
            )),
            "not a git repository: /repos/a\n  hint: provide a path to a git repository"
        );

        let three_repos = make_inputs(vec![
            repo_input("/repos/a/foo-api"),
            repo_input("/repos/b/foo-api"),
            repo_input("/repos/c"),
        ]);
        assert_eq!(
            error_text(decide_init(
                &three_repos,
                observed(vec![
                    RepoPathObservation::Repository(abs("/repos/a/foo-api")),
                    RepoPathObservation::Repository(abs("/repos/b/foo-api")),
                    RepoPathObservation::Missing(abs("/repos/c")),
                ])
            )),
            "duplicate repo name: foo-api\n  hint: use --repo /path:custom-name to disambiguate"
        );

        let mut reserved_entry =
            make_inputs(vec![repo_input("/repos/foo-api"), repo_input("/repos/b")]);
        reserved_entry.disposable_root_entries = vec![entry("foo-api")];
        assert_eq!(
            error_text(decide_init(
                &reserved_entry,
                observed(vec![
                    RepoPathObservation::Repository(abs("/repos/foo-api")),
                    RepoPathObservation::Missing(abs("/repos/b")),
                ])
            )),
            "repo path does not exist: /repos/b\n  hint: provide an absolute path to a git repository"
        );
    }

    #[test]
    fn decide_init_checks_disposable_root_entries_against_repo_names() {
        let mut repo_name_entry = make_inputs(vec![repo_input("/repos/foo-api")]);
        repo_name_entry.disposable_root_entries = vec![entry("foo-api")];
        let error = error_text(decide_init(
            &repo_name_entry,
            observed(vec![RepoPathObservation::Repository(abs("/repos/foo-api"))]),
        ));
        assert!(
            error.contains("disposable root entry foo-api is reserved"),
            "{error}"
        );

        let mut metadata_entry = make_inputs(vec![repo_input("/repos/foo-api")]);
        metadata_entry.disposable_root_entries = vec![entry(META_FILENAME)];
        let error = error_text(decide_init(
            &metadata_entry,
            observed(vec![RepoPathObservation::Repository(abs("/repos/foo-api"))]),
        ));
        assert!(
            error.contains("disposable root entry .forest-meta.toml is reserved"),
            "{error}"
        );
    }

    fn template_at(worktree_base: &str) -> ResolvedTemplate {
        ResolvedTemplate {
            worktree_base: abs(worktree_base),
            base_branch: "dev".to_string(),
            feature_branch_template: "testuser/{name}".to_string(),
            disposable_root_entries: vec![],
            repos: vec![],
        }
    }

    #[test]
    fn merge_init_starts_a_config_with_the_template_as_default() {
        let config = merge_init(None, "first", template_at("/wt/first"), false).unwrap();

        assert_eq!(config.default_template, "first");
        assert_eq!(config.templates.keys().collect::<Vec<_>>(), ["first"]);
        assert_eq!(
            *config.templates["first"].worktree_base,
            *PathBuf::from("/wt/first")
        );
        assert!(config.version_check.is_none());
    }

    #[test]
    fn merge_init_adds_templates_and_replaces_one_only_with_force() {
        let existing = || ResolvedConfig {
            default_template: "a".to_string(),
            templates: BTreeMap::from([
                ("a".to_string(), template_at("/wt/a")),
                ("b".to_string(), template_at("/wt/b")),
            ]),
            version_check: None,
        };

        let added = merge_init(Some(existing()), "c", template_at("/wt/c"), false).unwrap();
        assert_eq!(added.default_template, "a");
        assert_eq!(added.templates.keys().collect::<Vec<_>>(), ["a", "b", "c"]);

        let refused = merge_init(Some(existing()), "a", template_at("/wt/new-a"), false);
        assert_eq!(
            error_text(refused),
            "template \"a\" already exists in config\n  hint: use --force to overwrite, or choose a different name"
        );

        let replaced = merge_init(Some(existing()), "a", template_at("/wt/new-a"), true).unwrap();
        assert_eq!(replaced.default_template, "a");
        assert_eq!(
            *replaced.templates["a"].worktree_base,
            *PathBuf::from("/wt/new-a")
        );
        assert_eq!(
            *replaced.templates["b"].worktree_base,
            *PathBuf::from("/wt/b")
        );
    }
}
