use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::paths::expand_tilde;

// --- Raw deserialization structs (TOML shape) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTemplateConfig {
    pub default_template: String,
    pub template: BTreeMap<String, TemplateConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub repos: Vec<RepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
}

// --- Resolved types (post-parse) ---

#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    pub path: PathBuf,
    pub name: String,
    pub base_branch: String,
    pub remote: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub default_template: String,
    pub templates: BTreeMap<String, ResolvedTemplate>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTemplate {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub repos: Vec<ResolvedRepo>,
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
            .map(|t| t.worktree_base.as_path())
            .collect();
        bases.sort();
        bases.dedup();
        bases
    }
}

// --- Config loading ---

pub fn default_config_path() -> Result<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "git-forest")
        .context("could not determine config directory")?;
    Ok(proj.config_dir().join("config.toml"))
}

pub fn load_default_config() -> Result<ResolvedConfig> {
    let path = default_config_path()?;
    if !path.exists() {
        bail!(
            "config not found at {}\nRun `git forest init` to create one.",
            path.display()
        );
    }
    load_config(&path)
}

pub fn load_config(path: &Path) -> Result<ResolvedConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    parse_config(&contents)
}

pub fn parse_config(contents: &str) -> Result<ResolvedConfig> {
    let raw: MultiTemplateConfig =
        toml::from_str(contents).context("failed to parse config TOML")?;

    let mut templates = BTreeMap::new();

    for (tmpl_name, tmpl_config) in &raw.template {
        let worktree_base = expand_tilde(tmpl_config.worktree_base.to_str().unwrap_or(""));

        if !tmpl_config.feature_branch_template.contains("{name}") {
            bail!(
                "template {:?}: feature_branch_template must contain {{name}}",
                tmpl_name
            );
        }

        let mut repos = Vec::new();
        let mut names = HashSet::new();

        for repo in &tmpl_config.repos {
            let path = expand_tilde(repo.path.to_str().unwrap_or(""));

            let name = repo.name.clone().unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });

            if name.is_empty() {
                bail!(
                    "template {:?}: repo has empty name (path: {})",
                    tmpl_name,
                    path.display()
                );
            }

            if !names.insert(name.clone()) {
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

        let resolved_tmpl = ResolvedTemplate {
            worktree_base,
            base_branch: tmpl_config.base_branch.clone(),
            feature_branch_template: tmpl_config.feature_branch_template.clone(),
            repos,
        };

        debug_assert!(
            resolved_tmpl.worktree_base.is_absolute(),
            "worktree_base must be absolute after parsing"
        );
        debug_assert!(
            resolved_tmpl.repos.iter().all(|r| !r.name.is_empty()),
            "all repo names must be non-empty"
        );
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

pub fn write_config_atomic(path: &Path, config: &ResolvedConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let raw = MultiTemplateConfig {
        default_template: config.default_template.clone(),
        template: config
            .templates
            .iter()
            .map(|(name, tmpl)| {
                (
                    name.clone(),
                    TemplateConfig {
                        worktree_base: tmpl.worktree_base.clone(),
                        base_branch: tmpl.base_branch.clone(),
                        feature_branch_template: tmpl.feature_branch_template.clone(),
                        repos: tmpl
                            .repos
                            .iter()
                            .map(|r| RepoConfig {
                                path: r.path.clone(),
                                name: Some(r.name.clone()),
                                base_branch: Some(r.base_branch.clone()),
                                remote: Some(r.remote.clone()),
                            })
                            .collect(),
                    },
                )
            })
            .collect(),
    };

    let content = toml::to_string_pretty(&raw).context("failed to serialize config")?;

    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &content)
        .with_context(|| format!("failed to write temp config to {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("failed to rename config to {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let toml = r#"
default_template = "opencop"

[template.opencop]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "/tmp/src/foo-api"
name = "foo-api"
base_branch = "dev"
remote = "upstream"

[[template.opencop.repos]]
path = "/tmp/src/foo-web"
name = "foo-web"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.worktree_base, PathBuf::from("/tmp/worktrees"));
        assert_eq!(tmpl.base_branch, "dev");
        assert_eq!(tmpl.feature_branch_template, "dliv/{name}");
        assert_eq!(tmpl.repos.len(), 2);
        assert_eq!(tmpl.repos[0].name, "foo-api");
        assert_eq!(tmpl.repos[0].remote, "upstream");
        assert_eq!(tmpl.repos[1].name, "foo-web");
        assert_eq!(tmpl.repos[1].remote, "origin");
    }

    #[test]
    fn parse_minimal_config_defaults_applied() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name, "foo-api");
        assert_eq!(tmpl.repos[0].base_branch, "dev");
        assert_eq!(tmpl.repos[0].remote, "origin");
    }

    #[test]
    fn tilde_expansion_on_worktree_base() {
        let home = std::env::var("HOME").unwrap();
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "~/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.worktree_base, PathBuf::from(&home).join("worktrees"));
    }

    #[test]
    fn tilde_expansion_on_repo_path() {
        let home = std::env::var("HOME").unwrap();
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "~/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].path, PathBuf::from(&home).join("src/foo-api"));
    }

    #[test]
    fn name_derived_from_path_when_omitted() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/my-cool-repo"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name, "my-cool-repo");
    }

    #[test]
    fn base_branch_inherited_from_general() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "develop"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"

[[template.default.repos]]
path = "/tmp/src/bar"
base_branch = "main"
"#;
        let config = parse_config(toml).unwrap();
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
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml).unwrap();
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
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo"
name = "foo"

[[template.default.repos]]
path = "/tmp/src/bar"
name = "foo"
"#;
        let result = parse_config(toml);
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
        let result = parse_config(toml);
        assert!(result.is_err());
    }

    #[test]
    fn branch_template_must_contain_name() {
        let toml = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/feature"

[[template.default.repos]]
path = "/tmp/src/foo"
"#;
        let result = parse_config(toml);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("feature_branch_template"));
    }

    // --- Multi-template tests ---

    #[test]
    fn parse_multi_template_config() {
        let toml = r#"
default_template = "opencop"

[template.opencop]
worktree_base = "/tmp/worktrees/opencop"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "/tmp/src/opencop-java"

[[template.opencop.repos]]
path = "/tmp/src/opencop-web"
base_branch = "main"

[template.acme]
worktree_base = "/tmp/worktrees/acme"
base_branch = "main"
feature_branch_template = "dliv/{name}"

[[template.acme.repos]]
path = "/tmp/src/acme-api"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.templates.len(), 2);

        let opencop = config.resolve_template(Some("opencop")).unwrap();
        assert_eq!(opencop.repos.len(), 2);
        assert_eq!(opencop.repos[0].base_branch, "dev");
        assert_eq!(opencop.repos[1].base_branch, "main");

        let acme = config.resolve_template(Some("acme")).unwrap();
        assert_eq!(acme.repos.len(), 1);
        assert_eq!(acme.base_branch, "main");
    }

    #[test]
    fn parse_multi_template_default_resolution() {
        let toml = r#"
default_template = "opencop"

[template.opencop]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "/tmp/src/foo"

[template.acme]
worktree_base = "/tmp/worktrees/acme"
base_branch = "main"
feature_branch_template = "dliv/{name}"

[[template.acme.repos]]
path = "/tmp/src/bar"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.base_branch, "dev"); // opencop is the default
    }

    #[test]
    fn parse_multi_template_explicit_resolution() {
        let toml = r#"
default_template = "opencop"

[template.opencop]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "/tmp/src/foo"

[template.acme]
worktree_base = "/tmp/worktrees/acme"
base_branch = "main"
feature_branch_template = "dliv/{name}"

[[template.acme.repos]]
path = "/tmp/src/bar"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(Some("acme")).unwrap();
        assert_eq!(tmpl.base_branch, "main");
    }

    #[test]
    fn parse_multi_template_unknown_template_errors() {
        let toml = r#"
default_template = "opencop"

[template.opencop]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml).unwrap();
        let result = config.resolve_template(Some("nonexistent"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "error: {}", err);
        assert!(err.contains("opencop"), "should list available: {}", err);
    }

    #[test]
    fn parse_multi_template_empty_templates_errors() {
        let config = ResolvedConfig {
            default_template: "default".to_string(),
            templates: BTreeMap::new(),
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

[template.opencop]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.opencop.repos]]
path = "/tmp/src/foo"
"#;
        let result = parse_config(toml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("default_template"), "error: {}", err);
        assert!(err.contains("nonexistent"), "error: {}", err);
        assert!(err.contains("opencop"), "should list available: {}", err);
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
feature_branch_template = "dliv/{name}"

[[template."".repos]]
path = "/tmp/src/foo"
"#;
        // This should parse since "" is a valid TOML key and default_template matches
        let result = parse_config(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn all_worktree_bases_deduplicates() {
        let mut templates = BTreeMap::new();
        templates.insert(
            "a".to_string(),
            ResolvedTemplate {
                worktree_base: PathBuf::from("/tmp/worktrees"),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        templates.insert(
            "b".to_string(),
            ResolvedTemplate {
                worktree_base: PathBuf::from("/tmp/worktrees"),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        let config = ResolvedConfig {
            default_template: "a".to_string(),
            templates,
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
                worktree_base: PathBuf::from("/tmp/worktrees/a"),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        templates.insert(
            "b".to_string(),
            ResolvedTemplate {
                worktree_base: PathBuf::from("/tmp/worktrees/b"),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        let config = ResolvedConfig {
            default_template: "a".to_string(),
            templates,
        };
        let bases = config.all_worktree_bases();
        assert_eq!(bases.len(), 2);
    }
}
