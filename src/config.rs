use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::paths::expand_tilde;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub repos: Vec<RepoConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
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

#[derive(Debug, Clone)]
pub struct ResolvedRepo {
    pub path: PathBuf,
    pub name: String,
    pub base_branch: String,
    pub remote: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub general: GeneralConfig,
    pub repos: Vec<ResolvedRepo>,
}

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
    let raw: Config = toml::from_str(contents).context("failed to parse config TOML")?;

    let worktree_base = expand_tilde(raw.general.worktree_base.to_str().unwrap_or(""));

    if !raw.general.branch_template.contains("{name}") {
        bail!("branch_template must contain {{name}}");
    }

    let general = GeneralConfig {
        worktree_base,
        base_branch: raw.general.base_branch,
        branch_template: raw.general.branch_template,
        username: raw.general.username,
    };

    let mut repos = Vec::new();
    let mut names = HashSet::new();

    for repo in &raw.repos {
        let path = expand_tilde(repo.path.to_str().unwrap_or(""));

        let name = repo.name.clone().unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        if name.is_empty() {
            bail!("repo has empty name (path: {})", path.display());
        }

        if !names.insert(name.clone()) {
            bail!("duplicate repo name: {}", name);
        }

        let base_branch = repo
            .base_branch
            .clone()
            .unwrap_or_else(|| general.base_branch.clone());

        let remote = repo.remote.clone().unwrap_or_else(|| "origin".to_string());

        repos.push(ResolvedRepo {
            path,
            name,
            base_branch,
            remote,
        });
    }

    let result = ResolvedConfig { general, repos };

    debug_assert!(
        result.general.worktree_base.is_absolute(),
        "worktree_base must be absolute after parsing"
    );
    debug_assert!(
        result.repos.iter().all(|r| !r.name.is_empty()),
        "all repo names must be non-empty"
    );
    debug_assert!(
        {
            let names: HashSet<&str> = result.repos.iter().map(|r| r.name.as_str()).collect();
            names.len() == result.repos.len()
        },
        "repo names must be unique"
    );

    Ok(result)
}

pub fn write_config_atomic(path: &Path, config: &ResolvedConfig, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "config already exists at {}\nUse --force to overwrite.",
            path.display()
        );
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let raw = Config {
        general: config.general.clone(),
        repos: config
            .repos
            .iter()
            .map(|r| RepoConfig {
                path: r.path.clone(),
                name: Some(r.name.clone()),
                base_branch: Some(r.base_branch.clone()),
                remote: Some(r.remote.clone()),
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
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/foo-api"
name = "foo-api"
base_branch = "dev"
remote = "upstream"

[[repos]]
path = "/tmp/src/foo-web"
name = "foo-web"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(
            config.general.worktree_base,
            PathBuf::from("/tmp/worktrees")
        );
        assert_eq!(config.general.base_branch, "dev");
        assert_eq!(config.general.username, "dliv");
        assert_eq!(config.repos.len(), 2);
        assert_eq!(config.repos[0].name, "foo-api");
        assert_eq!(config.repos[0].remote, "upstream");
        assert_eq!(config.repos[1].name, "foo-web");
        assert_eq!(config.repos[1].remote, "origin");
    }

    #[test]
    fn parse_minimal_config_defaults_applied() {
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.repos[0].name, "foo-api");
        assert_eq!(config.repos[0].base_branch, "dev");
        assert_eq!(config.repos[0].remote, "origin");
    }

    #[test]
    fn tilde_expansion_on_worktree_base() {
        let home = std::env::var("HOME").unwrap();
        let toml = r#"
[general]
worktree_base = "~/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(
            config.general.worktree_base,
            PathBuf::from(&home).join("worktrees")
        );
    }

    #[test]
    fn tilde_expansion_on_repo_path() {
        let home = std::env::var("HOME").unwrap();
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "~/src/foo-api"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(
            config.repos[0].path,
            PathBuf::from(&home).join("src/foo-api")
        );
    }

    #[test]
    fn name_derived_from_path_when_omitted() {
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/my-cool-repo"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.repos[0].name, "my-cool-repo");
    }

    #[test]
    fn base_branch_inherited_from_general() {
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "develop"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/foo"

[[repos]]
path = "/tmp/src/bar"
base_branch = "main"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.repos[0].base_branch, "develop");
        assert_eq!(config.repos[1].base_branch, "main");
    }

    #[test]
    fn remote_defaults_to_origin() {
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/foo"
"#;
        let config = parse_config(toml).unwrap();
        assert_eq!(config.repos[0].remote, "origin");
    }

    #[test]
    fn duplicate_repo_names_error() {
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/{name}"
username = "dliv"

[[repos]]
path = "/tmp/src/foo"
name = "foo"

[[repos]]
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
[general]
worktree_base = "/tmp/worktrees"
"#;
        let result = parse_config(toml);
        assert!(result.is_err());
    }

    #[test]
    fn branch_template_must_contain_name() {
        let toml = r#"
[general]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
branch_template = "{user}/feature"
username = "dliv"

[[repos]]
path = "/tmp/src/foo"
"#;
        let result = parse_config(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("branch_template"));
    }
}
