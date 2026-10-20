//! The config file's format: its schema, parsing and rendering, and `~` expansion with the home
//! directory passed in. Reading and writing the file stay in the forest ops crate
//! (`git-forest-ops`).
//!
//! Each section of the file is owned by its reader: the forest's `default_template` and
//! `[template.*]` here, the update check's `[version_check]` in the update-check edge, which
//! reads it itself. Every other top-level key and table is kept and written back, re-serialized
//! as parsed (keys in sorted order, comments and formatting dropped, as the rewrite does for the
//! forest's own keys).

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::meta::META_FILENAME;
use crate::paths::{validate_disposable_root_entries, AbsolutePath, DisposableRootEntry, RepoName};
use crate::plan::new::{ResolvedRepo, ResolvedTemplate};

/// Expands a leading `~` or `~/` with `home`, the home directory the composition root read and
/// passed in (`None` when `HOME` is unset), and requires the result to be absolute.
pub fn expand_tilde(path: &str, home: Option<&str>) -> Result<AbsolutePath> {
    let result = if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home {
            PathBuf::from(home).join(rest)
        } else {
            bail!("cannot expand ~/: HOME environment variable is not set");
        }
    } else if path == "~" {
        if let Some(home) = home {
            PathBuf::from(home)
        } else {
            bail!("cannot expand ~: HOME environment variable is not set");
        }
    } else {
        PathBuf::from(path)
    };

    AbsolutePath::new(result).ok_or_else(|| anyhow::anyhow!("path is not absolute: {}", path))
}

// --- Raw deserialization structs (TOML shape) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MultiTemplateConfig {
    default_template: String,
    template: BTreeMap<String, TemplateConfig>,
    /// Every top-level key and table the forest does not own, kept to be written back.
    #[serde(flatten)]
    rest: toml::Table,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemplateConfig {
    worktree_base: PathBuf,
    base_branch: String,
    feature_branch_template: String,
    #[serde(default)]
    disposable_root_entries: Vec<DisposableRootEntry>,
    repos: Vec<RepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepoConfig {
    path: PathBuf,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    base_branch: Option<String>,
    #[serde(default)]
    remote: Option<String>,
}

// --- Resolved types (post-parse) ---

/// The config file, parsed: the default template's name, every template resolved, and every
/// other top-level key and table as parsed, which the forest does not read and writes back.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub default_template: String,
    pub templates: BTreeMap<String, ResolvedTemplate>,
    /// The sections the forest does not own, the update check's `[version_check]` among them.
    pub rest: toml::Table,
}

impl ResolvedConfig {
    pub fn resolve_template(&self, name: Option<&str>) -> Result<&ResolvedTemplate> {
        if self.templates.is_empty() {
            bail!("no templates configured\n  hint: run `git forest init --repo <path> ...` to create one");
        }
        let key = name.unwrap_or(&self.default_template);
        self.templates.get(key).ok_or_else(|| {
            let available: Vec<&str> = self.templates.keys().map(|k| k.as_str()).collect();
            anyhow!(
                "template {:?} not found\n  hint: available templates: {}",
                key,
                available.join(", ")
            )
        })
    }

    pub fn all_worktree_bases(&self) -> Vec<&Path> {
        let mut bases: Vec<&Path> = self
            .templates
            .values()
            .map(|t| t.worktree_base.as_ref())
            .collect();
        bases.sort();
        bases.dedup();
        bases
    }
}

/// Parses the config file's `contents`; its `~` paths expand with `home`.
pub fn parse_config(contents: &str, home: Option<&str>) -> Result<ResolvedConfig> {
    let raw: MultiTemplateConfig =
        toml::from_str(contents).context("failed to parse config TOML")?;

    let mut templates = BTreeMap::new();

    for (tmpl_name, tmpl_config) in &raw.template {
        let worktree_base = expand_tilde(tmpl_config.worktree_base.to_str().unwrap_or(""), home)
            .with_context(|| format!("template {:?}: invalid worktree_base", tmpl_name))?;

        if tmpl_config.repos.is_empty() {
            bail!("template {:?}: must have at least one repo", tmpl_name);
        }

        if !tmpl_config.feature_branch_template.contains("{name}") {
            bail!(
                "template {:?}: feature_branch_template must contain {{name}}",
                tmpl_name
            );
        }

        let mut repos = Vec::new();
        let mut names = HashSet::new();

        for repo in &tmpl_config.repos {
            let path = expand_tilde(repo.path.to_str().unwrap_or(""), home)
                .with_context(|| format!("template {:?}: invalid repo path", tmpl_name))?;

            let name_str = repo.name.clone().unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });

            let name = RepoName::new(name_str).with_context(|| {
                format!(
                    "template {:?}: repo has empty name (path: {})",
                    tmpl_name,
                    path.display()
                )
            })?;

            if !names.insert(name.to_string()) {
                bail!("template {:?}: duplicate repo name: {}", tmpl_name, name);
            }

            let base_branch = repo
                .base_branch
                .clone()
                .unwrap_or_else(|| tmpl_config.base_branch.clone());

            let remote = repo.remote.clone().unwrap_or_else(|| "origin".to_string());

            repos.push(ResolvedRepo {
                path,
                name,
                base_branch,
                remote,
            });
        }

        validate_disposable_root_entries(
            &tmpl_config.disposable_root_entries,
            std::iter::once(META_FILENAME).chain(repos.iter().map(|repo| repo.name.as_str())),
        )
        .with_context(|| format!("template {:?}: invalid disposable root entries", tmpl_name))?;

        let resolved_tmpl = ResolvedTemplate {
            worktree_base,
            base_branch: tmpl_config.base_branch.clone(),
            feature_branch_template: tmpl_config.feature_branch_template.clone(),
            disposable_root_entries: tmpl_config.disposable_root_entries.clone(),
            repos,
        };

        // Collection-level invariant, not expressible as newtype
        debug_assert!(
            {
                let names: HashSet<&str> = resolved_tmpl
                    .repos
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect();
                names.len() == resolved_tmpl.repos.len()
            },
            "repo names must be unique"
        );

        templates.insert(tmpl_name.clone(), resolved_tmpl);
    }

    let resolved = ResolvedConfig {
        default_template: raw.default_template.clone(),
        templates,
        rest: raw.rest.clone(),
    };

    // Validate default_template references an existing key
    if !resolved.templates.contains_key(&resolved.default_template) {
        let available: Vec<&str> = resolved.templates.keys().map(|k| k.as_str()).collect();
        bail!(
            "default_template {:?} not found in config\n  hint: available templates: {}\n  hint: edit config to fix default_template, or run `git forest init --template {} ...`",
            resolved.default_template,
            available.join(", "),
            resolved.default_template
        );
    }

    Ok(resolved)
}

/// Renders `config` as the config file's text: every template in full, each repo with its name,
/// base branch and remote written out.
pub fn render_config(config: &ResolvedConfig) -> Result<String> {
    let raw = MultiTemplateConfig {
        default_template: config.default_template.clone(),
        rest: config.rest.clone(),
        template: config
            .templates
            .iter()
            .map(|(name, tmpl)| {
                (
                    name.clone(),
                    TemplateConfig {
                        worktree_base: tmpl.worktree_base.clone().into_inner(),
                        base_branch: tmpl.base_branch.clone(),
                        feature_branch_template: tmpl.feature_branch_template.clone(),
                        disposable_root_entries: tmpl.disposable_root_entries.clone(),
                        repos: tmpl
                            .repos
                            .iter()
                            .map(|r| RepoConfig {
                                path: r.path.clone().into_inner(),
                                name: Some(r.name.to_string()),
                                base_branch: Some(r.base_branch.clone()),
                                remote: Some(r.remote.clone()),
                            })
                            .collect(),
                    },
                )
            })
            .collect(),
    };

    toml::to_string_pretty(&raw).context("failed to serialize config")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed fake home directory, so that every expected path proves the passed value is used.
    const TEST_HOME: &str = "/home/tester";

    #[test]
    fn expand_tilde_replaces_home() {
        let result = expand_tilde("~/src/foo", Some(TEST_HOME)).unwrap();
        assert_eq!(*result, *PathBuf::from(TEST_HOME).join("src/foo"));
    }

    #[test]
    fn expand_tilde_bare_tilde() {
        assert_eq!(
            *expand_tilde("~", Some(TEST_HOME)).unwrap(),
            *PathBuf::from(TEST_HOME)
        );
    }

    #[test]
    fn expand_tilde_leaves_absolute_unchanged() {
        let result = expand_tilde("/usr/local/bin", Some(TEST_HOME)).unwrap();
        assert_eq!(*result, *PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn expand_tilde_relative_path_errors() {
        let result = expand_tilde("foo/bar", Some(TEST_HOME));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not absolute"));
    }

    #[test]
    fn expand_tilde_returns_absolute_path() {
        let result = expand_tilde("/some/absolute/path", Some(TEST_HOME)).unwrap();
        assert!(result.is_absolute());
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
default_template = "project_a"

[template.project_a]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"
disposable_root_entries = [".idea", ".claude"]

[[template.project_a.repos]]
path = "/tmp/src/foo-api"
name = "foo-api"
base_branch = "dev"
remote = "upstream"

[[template.project_a.repos]]
path = "/tmp/src/foo-web"
name = "foo-web"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(*tmpl.worktree_base, *PathBuf::from("/tmp/worktrees"));
        assert_eq!(tmpl.base_branch, "dev");
        assert_eq!(tmpl.feature_branch_template, "user/{name}");
        assert_eq!(
            tmpl.disposable_root_entries
                .iter()
                .map(DisposableRootEntry::as_str)
                .collect::<Vec<_>>(),
            vec![".idea", ".claude"]
        );
        assert_eq!(tmpl.repos.len(), 2);
        assert_eq!(tmpl.repos[0].name.as_str(), "foo-api");
        assert_eq!(tmpl.repos[0].remote, "upstream");
        assert_eq!(tmpl.repos[1].name.as_str(), "foo-web");
        assert_eq!(tmpl.repos[1].remote, "origin");
    }

    #[test]
    fn parse_minimal_config_defaults_applied() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name.as_str(), "foo-api");
        assert_eq!(tmpl.repos[0].base_branch, "dev");
        assert_eq!(tmpl.repos[0].remote, "origin");
        assert!(tmpl.disposable_root_entries.is_empty());
    }

    #[test]
    fn disposable_root_entries_reject_nested_reserved_and_duplicate_values() {
        for entries in [
            r#"[".claude/cache"]"#,
            r#"[".forest-meta.toml"]"#,
            r#"[".FOREST-META.TOML"]"#,
            r#"["foo-api"]"#,
            r#"["FOO-API"]"#,
            r#"[".idea", ".IDEA"]"#,
        ] {
            let toml = format!(
                r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{{name}}"
disposable_root_entries = {entries}

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#
            );

            assert!(
                parse_config(&toml, Some(TEST_HOME)).is_err(),
                "expected disposable root entries to be rejected: {entries}"
            );
        }
    }

    #[test]
    fn tilde_expansion_on_worktree_base() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "~/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(
            *tmpl.worktree_base,
            *PathBuf::from(TEST_HOME).join("worktrees")
        );
    }

    #[test]
    fn tilde_expansion_on_repo_path() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "~/src/foo-api"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(
            *tmpl.repos[0].path,
            *PathBuf::from(TEST_HOME).join("src/foo-api")
        );
    }

    #[test]
    fn name_derived_from_path_when_omitted() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/my-cool-repo"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name.as_str(), "my-cool-repo");
    }

    #[test]
    fn base_branch_inherited_from_general() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "develop"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"

[[template.default.repos]]
path = "/tmp/src/bar"
base_branch = "main"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].base_branch, "develop");
        assert_eq!(tmpl.repos[1].base_branch, "main");
    }

    #[test]
    fn remote_defaults_to_origin() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].remote, "origin");
    }

    #[test]
    fn duplicate_repo_names_error() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"
name = "foo"

[[template.default.repos]]
path = "/tmp/src/bar"
name = "foo"
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("duplicate repo name"));
    }

    #[test]
    fn missing_required_fields_error() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
    }

    #[test]
    fn branch_template_must_contain_name() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/feature"

[[template.default.repos]]
path = "/tmp/src/foo"
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("feature_branch_template"));
    }

    #[test]
    fn parse_multi_template_config() {
        let toml = r#"
default_template = "project_a"

[template.project_a]
worktree_base = "/tmp/worktrees/project_a"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.project_a.repos]]
path = "/tmp/src/project_a-java"

[[template.project_a.repos]]
path = "/tmp/src/project_a-web"
base_branch = "main"

[template.project_b]
worktree_base = "/tmp/worktrees/project_b"
base_branch = "main"
feature_branch_template = "user/{name}"

[[template.project_b.repos]]
path = "/tmp/src/project_b-api"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        assert_eq!(config.templates.len(), 2);

        let project_a = config.resolve_template(Some("project_a")).unwrap();
        assert_eq!(project_a.repos.len(), 2);
        assert_eq!(project_a.repos[0].base_branch, "dev");
        assert_eq!(project_a.repos[1].base_branch, "main");

        let project_b = config.resolve_template(Some("project_b")).unwrap();
        assert_eq!(project_b.repos.len(), 1);
        assert_eq!(project_b.base_branch, "main");
    }

    #[test]
    fn parse_multi_template_default_resolution() {
        let toml = r#"
default_template = "project_a"

[template.project_a]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.project_a.repos]]
path = "/tmp/src/foo"

[template.project_b]
worktree_base = "/tmp/worktrees/project_b"
base_branch = "main"
feature_branch_template = "user/{name}"

[[template.project_b.repos]]
path = "/tmp/src/bar"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.base_branch, "dev"); // project_a is the default
    }

    #[test]
    fn parse_multi_template_explicit_resolution() {
        let toml = r#"
default_template = "project_a"

[template.project_a]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.project_a.repos]]
path = "/tmp/src/foo"

[template.project_b]
worktree_base = "/tmp/worktrees/project_b"
base_branch = "main"
feature_branch_template = "user/{name}"

[[template.project_b.repos]]
path = "/tmp/src/bar"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let tmpl = config.resolve_template(Some("project_b")).unwrap();
        assert_eq!(tmpl.base_branch, "main");
    }

    #[test]
    fn parse_multi_template_unknown_template_errors() {
        let toml = r#"
default_template = "project_a"

[template.project_a]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.project_a.repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        let result = config.resolve_template(Some("nonexistent"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "error: {}", err);
        assert!(err.contains("project_a"), "should list available: {}", err);
    }

    #[test]
    fn parse_multi_template_empty_templates_errors() {
        let config = ResolvedConfig {
            default_template: "default".to_string(),
            templates: BTreeMap::new(),
            rest: toml::Table::new(),
        };
        let result = config.resolve_template(None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no templates configured"));
    }

    #[test]
    fn parse_multi_template_invalid_default_errors() {
        let toml = r#"
default_template = "nonexistent"

[template.project_a]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.project_a.repos]]
path = "/tmp/src/foo"
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("default_template"), "error: {}", err);
        assert!(err.contains("nonexistent"), "error: {}", err);
        assert!(err.contains("project_a"), "should list available: {}", err);
    }

    #[test]
    fn template_name_validation_empty() {
        // Empty template name in TOML is technically possible but weird.
        // The TOML `[template.""]` would parse but default_template validation catches it.
        let toml = r#"
default_template = ""

[template.""]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template."".repos]]
path = "/tmp/src/foo"
"#;
        // This should parse since "" is a valid TOML key and default_template matches
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_ok());
    }

    #[test]
    fn all_worktree_bases_deduplicates() {
        let mut templates = BTreeMap::new();
        templates.insert(
            "a".to_string(),
            ResolvedTemplate {
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                disposable_root_entries: vec![],
                repos: vec![],
            },
        );
        templates.insert(
            "b".to_string(),
            ResolvedTemplate {
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                disposable_root_entries: vec![],
                repos: vec![],
            },
        );
        let config = ResolvedConfig {
            default_template: "a".to_string(),
            templates,
            rest: toml::Table::new(),
        };
        let bases = config.all_worktree_bases();
        assert_eq!(bases.len(), 1);
    }

    #[test]
    fn all_worktree_bases_multiple() {
        let mut templates = BTreeMap::new();
        templates.insert(
            "a".to_string(),
            ResolvedTemplate {
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees/a")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                disposable_root_entries: vec![],
                repos: vec![],
            },
        );
        templates.insert(
            "b".to_string(),
            ResolvedTemplate {
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees/b")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                disposable_root_entries: vec![],
                repos: vec![],
            },
        );
        let config = ResolvedConfig {
            default_template: "a".to_string(),
            templates,
            rest: toml::Table::new(),
        };
        let bases = config.all_worktree_bases();
        assert_eq!(bases.len(), 2);
    }

    #[test]
    fn template_with_zero_repos_errors() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"
repos = []
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("at least one repo"), "error: {}", err);
    }

    #[test]
    fn valid_toml_wrong_shape_errors() {
        // template is a string instead of a table
        let toml = r#"
default_template = "default"
template = "not a table"
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
    }

    #[test]
    fn valid_toml_missing_template_section_errors() {
        let toml = r#"
default_template = "default"
"#;
        let result = parse_config(toml, Some(TEST_HOME));
        assert!(result.is_err());
    }

    #[test]
    fn config_without_version_check_still_parses() {
        // Existing configs without [version_check] section must still parse
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml, Some(TEST_HOME)).unwrap();
        assert!(config.rest.is_empty());
    }

    /// A config with sections the forest does not own: a top-level key, an `[other]` table, and a
    /// `[version_check]` table that is malformed for the update check and irrelevant here.
    const FOREIGN_SECTIONS_CONFIG: &str = r#"
note = "kept"
default_template = "default"

[version_check]
enabled = "no"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"

[other]
x = 1
"#;

    #[test]
    fn config_keeps_the_sections_it_does_not_own() {
        let config = parse_config(FOREIGN_SECTIONS_CONFIG, Some(TEST_HOME)).unwrap();

        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(*tmpl.worktree_base, *PathBuf::from("/tmp/worktrees"));
        assert_eq!(tmpl.repos[0].name.as_str(), "foo-api");
        let expected: toml::Table = toml::from_str(
            r#"
note = "kept"

[other]
x = 1

[version_check]
enabled = "no"
"#,
        )
        .unwrap();
        assert_eq!(config.rest, expected);
    }

    #[test]
    fn render_config_writes_back_the_sections_it_does_not_own() {
        let config = parse_config(FOREIGN_SECTIONS_CONFIG, Some(TEST_HOME)).unwrap();

        assert_eq!(
            render_config(&config).unwrap(),
            r#"default_template = "default"
note = "kept"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"
disposable_root_entries = []

[[template.default.repos]]
path = "/tmp/src/foo-api"
name = "foo-api"
base_branch = "dev"
remote = "origin"

[other]
x = 1

[version_check]
enabled = "no"
"#
        );
    }

    #[test]
    fn render_config_writes_each_template_in_full() {
        let template = ResolvedTemplate {
            worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap(),
            base_branch: "main".to_string(),
            feature_branch_template: "user/{name}".to_string(),
            disposable_root_entries: vec![DisposableRootEntry::new(".idea".to_string()).unwrap()],
            repos: vec![ResolvedRepo {
                path: AbsolutePath::new(PathBuf::from("/tmp/src/foo-api")).unwrap(),
                name: RepoName::new("foo-api".to_string()).unwrap(),
                base_branch: "main".to_string(),
                remote: "origin".to_string(),
            }],
        };
        let config = ResolvedConfig {
            default_template: "default".to_string(),
            templates: BTreeMap::from([("default".to_string(), template)]),
            rest: toml::from_str("[version_check]\nenabled = false\n").unwrap(),
        };

        assert_eq!(
            render_config(&config).unwrap(),
            r#"default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "main"
feature_branch_template = "user/{name}"
disposable_root_entries = [".idea"]

[[template.default.repos]]
path = "/tmp/src/foo-api"
name = "foo-api"
base_branch = "main"
remote = "origin"

[version_check]
enabled = false
"#
        );
    }
}
