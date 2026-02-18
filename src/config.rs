use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::paths::{expand_tilde, AbsolutePath, RepoName};

// --- Raw deserialization structs (TOML shape) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTemplateConfig {
    pub default_template: String,
    pub template: BTreeMap<String, TemplateConfig>,
    #[serde(default)]
    pub version_check: Option<VersionCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCheckConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
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
    pub path: AbsolutePath,
    pub name: RepoName,
    pub base_branch: String,
    pub remote: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub default_template: String,
    pub templates: BTreeMap<String, ResolvedTemplate>,
    pub version_check: Option<VersionCheckConfig>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTemplate {
    pub worktree_base: AbsolutePath,
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
            .map(|t| t.worktree_base.as_ref())
            .collect();
        bases.sort();
        bases.dedup();
        bases
    }
}

// --- XDG path helpers ---

const APP_NAME: &str = "git-forest";

/// Returns the XDG config directory for git-forest.
///
/// Resolution order:
/// 1. `$XDG_CONFIG_HOME/git-forest/` (if env var set)
/// 2. `~/.config/git-forest/` (Unix/macOS default)
/// 3. `directories` crate fallback (Windows)
pub(crate) fn xdg_config_dir() -> Result<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config", |proj| {
        proj.config_dir().to_path_buf()
    })
}

/// Returns the XDG state directory for git-forest.
///
/// Resolution order:
/// 1. `$XDG_STATE_HOME/git-forest/` (if env var set)
/// 2. `~/.local/state/git-forest/` (Unix/macOS default)
/// 3. `directories` crate fallback (Windows)
pub(crate) fn xdg_state_dir() -> Result<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state", |proj| {
        proj.state_dir()
            .unwrap_or_else(|| proj.data_local_dir())
            .to_path_buf()
    })
}

fn xdg_dir(
    env_var: &str,
    default_suffix: &str,
    fallback: fn(&directories::ProjectDirs) -> PathBuf,
) -> Result<PathBuf> {
    // 1. Explicit env var
    if let Ok(val) = std::env::var(env_var) {
        if !val.is_empty() {
            return Ok(PathBuf::from(val).join(APP_NAME));
        }
    }

    // 2. Unix/macOS default: ~/.<suffix>/git-forest/
    #[cfg(unix)]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join(default_suffix).join(APP_NAME));
        }
    }

    // 3. directories crate fallback (Windows, or if HOME is unset)
    #[cfg(not(unix))]
    let _ = default_suffix;
    let proj = directories::ProjectDirs::from("", "", APP_NAME)
        .context("could not determine config directory")?;
    Ok(fallback(&proj))
}

// --- Config loading ---

pub fn default_config_path() -> Result<PathBuf> {
    Ok(xdg_config_dir()?.join("config.toml"))
}

pub fn load_default_config() -> Result<ResolvedConfig> {
    let path = default_config_path()?;
    if !path.exists() {
        bail!(
            "config not found at {}\n  hint: run `git forest init` to create one",
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
        let worktree_base = expand_tilde(tmpl_config.worktree_base.to_str().unwrap_or(""))
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
            let path = expand_tilde(repo.path.to_str().unwrap_or(""))
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

        let resolved_tmpl = ResolvedTemplate {
            worktree_base,
            base_branch: tmpl_config.base_branch.clone(),
            feature_branch_template: tmpl_config.feature_branch_template.clone(),
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
        version_check: raw.version_check.clone(),
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

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: getuid() is always safe — no arguments, no failure mode.
    unsafe { libc::getuid() }
}

/// Walk ancestors of `target` to find the first existing directory that isn't writable,
/// and produce an actionable hint string.
fn diagnose_permission_denied(target: &Path) -> String {
    // Find the first existing ancestor
    let blocking = target
        .ancestors()
        .skip(1) // skip target itself
        .find(|p| p.exists());

    let Some(dir) = blocking else {
        return "check directory permissions".to_string();
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(dir) {
            let dir_uid = meta.uid();
            // Root ownership is the common macOS case (e.g. ~/.config created by sudo)
            if dir_uid == 0 {
                return format!(
                    "{} is owned by root. Run: sudo chown $(whoami) {}",
                    dir.display(),
                    dir.display()
                );
            }
            if dir_uid != current_uid() {
                return format!(
                    "{} is owned by another user (uid {}). Run: sudo chown $(whoami) {}",
                    dir.display(),
                    dir_uid,
                    dir.display()
                );
            }
            // Owned by us but not writable (e.g. chmod 555)
            return format!(
                "{} is not writable. Run: chmod u+w {}",
                dir.display(),
                dir.display()
            );
        }
    }

    format!("{} is not writable", dir.display())
}

pub fn write_config_atomic(path: &Path, config: &ResolvedConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                let hint = diagnose_permission_denied(parent);
                bail!(
                    "failed to create config directory {}: {}\n  hint: {}",
                    parent.display(),
                    e,
                    hint
                );
            }
            return Err(e).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            });
        }
    }

    let raw = MultiTemplateConfig {
        default_template: config.default_template.clone(),
        version_check: config.version_check.clone(),
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
        assert_eq!(*tmpl.worktree_base, *PathBuf::from("/tmp/worktrees"));
        assert_eq!(tmpl.base_branch, "dev");
        assert_eq!(tmpl.feature_branch_template, "dliv/{name}");
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
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        let tmpl = config.resolve_template(None).unwrap();
        assert_eq!(tmpl.repos[0].name.as_str(), "foo-api");
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
        assert_eq!(*tmpl.worktree_base, *PathBuf::from(&home).join("worktrees"));
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
        assert_eq!(
            *tmpl.repos[0].path,
            *PathBuf::from(&home).join("src/foo-api")
        );
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
        assert_eq!(tmpl.repos[0].name.as_str(), "my-cool-repo");
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
            version_check: None,
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
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        templates.insert(
            "b".to_string(),
            ResolvedTemplate {
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        let config = ResolvedConfig {
            default_template: "a".to_string(),
            templates,
            version_check: None,
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
                repos: vec![],
            },
        );
        templates.insert(
            "b".to_string(),
            ResolvedTemplate {
                worktree_base: AbsolutePath::new(PathBuf::from("/tmp/worktrees/b")).unwrap(),
                base_branch: "main".to_string(),
                feature_branch_template: "test/{name}".to_string(),
                repos: vec![],
            },
        );
        let config = ResolvedConfig {
            default_template: "a".to_string(),
            templates,
            version_check: None,
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
feature_branch_template = "dliv/{name}"
repos = []
"#;
        let result = parse_config(toml);
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
        let result = parse_config(toml);
        assert!(result.is_err());
    }

    #[test]
    fn valid_toml_missing_template_section_errors() {
        let toml = r#"
default_template = "default"
"#;
        let result = parse_config(toml);
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
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        assert!(config.version_check.is_none());
    }

    #[test]
    fn config_with_version_check_enabled() {
        let toml = r#"
default_template = "default"

[version_check]
enabled = false

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        let vc = config.version_check.unwrap();
        assert!(!vc.enabled);
    }

    #[test]
    fn config_with_version_check_defaults_to_enabled() {
        let toml = r#"
default_template = "default"

[version_check]

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "dliv/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        let vc = config.version_check.unwrap();
        assert!(vc.enabled);
    }

    // --- XDG path tests ---

    #[test]
    fn xdg_config_dir_respects_env_var() {
        let saved = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/test-xdg-config");
        let result = super::xdg_config_dir().unwrap();
        assert_eq!(result, PathBuf::from("/tmp/test-xdg-config/git-forest"));
        match saved {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    #[test]
    fn xdg_state_dir_respects_env_var() {
        let saved = std::env::var("XDG_STATE_HOME").ok();
        std::env::set_var("XDG_STATE_HOME", "/tmp/test-xdg-state");
        let result = super::xdg_state_dir().unwrap();
        assert_eq!(result, PathBuf::from("/tmp/test-xdg-state/git-forest"));
        match saved {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn xdg_config_dir_defaults_to_dot_config() {
        let saved_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        let home = std::env::var("HOME").unwrap();
        let result = super::xdg_config_dir().unwrap();
        assert_eq!(
            result,
            PathBuf::from(&home).join(".config").join("git-forest")
        );
        if let Some(v) = saved_xdg {
            std::env::set_var("XDG_CONFIG_HOME", v);
        }
    }

    #[cfg(unix)]
    #[test]
    fn xdg_state_dir_defaults_to_dot_local_state() {
        let saved_xdg = std::env::var("XDG_STATE_HOME").ok();
        std::env::remove_var("XDG_STATE_HOME");
        let home = std::env::var("HOME").unwrap();
        let result = super::xdg_state_dir().unwrap();
        assert_eq!(
            result,
            PathBuf::from(&home).join(".local/state").join("git-forest")
        );
        if let Some(v) = saved_xdg {
            std::env::set_var("XDG_STATE_HOME", v);
        }
    }

    #[test]
    fn default_config_path_ends_with_config_toml() {
        let path = super::default_config_path().unwrap();
        assert!(path.ends_with("git-forest/config.toml"));
    }

    // --- Permission diagnosis tests ---

    #[cfg(unix)]
    #[test]
    fn diagnose_permission_denied_unwritable_parent() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let target = blocked.join("child").join("grandchild");

        // Verify create_dir_all actually fails (skips if running as root)
        if std::fs::create_dir_all(&target).is_ok() {
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!("skipping: test running as root (permission check bypassed)");
            return;
        }

        let hint = super::diagnose_permission_denied(&target);
        assert!(
            hint.contains(&blocked.display().to_string()),
            "hint should mention blocking dir: {}",
            hint
        );
        assert!(
            hint.contains("not writable"),
            "hint should say not writable: {}",
            hint
        );
        assert!(
            hint.contains("chmod u+w"),
            "hint should suggest chmod: {}",
            hint
        );

        // Restore permissions so tempdir cleanup works
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn diagnose_permission_denied_root_owned() {
        // Test the formatting branch for root-owned directories.
        // We can't create root-owned dirs without sudo, so test the function
        // with a path where the first existing ancestor is "/" (owned by root).
        let target = Path::new("/nonexistent-abc123/git-forest");
        let hint = super::diagnose_permission_denied(target);
        // "/" is owned by root
        assert!(
            hint.contains("owned by root"),
            "hint should detect root ownership: {}",
            hint
        );
        assert!(
            hint.contains("sudo chown"),
            "hint should suggest chown: {}",
            hint
        );
    }

    #[test]
    fn diagnose_permission_denied_no_ancestors() {
        // Edge case: empty path or root
        let hint = super::diagnose_permission_denied(Path::new(""));
        assert!(hint.contains("not writable") || hint.contains("check directory permissions"));
    }
}
