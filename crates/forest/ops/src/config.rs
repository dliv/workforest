//! The config file's I/O: its location from the environment the composition root read, reading
//! and parsing it, and writing it atomically. Its format (the schema, parsing and rendering) is
//! the core's, in `git_forest_core::config`.

use anyhow::{bail, ensure, Context, Result};
use std::path::{Path, PathBuf};

use git_forest_core::config::{
    parse_config, render_config, version_check_enabled_in, ResolvedConfig,
};

/// The environment variables the config and state locations and `~` expansion read, as the
/// composition root read them once: each `None` when unset or not Unicode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    /// `HOME`.
    pub home: Option<String>,
    /// `XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<String>,
    /// `XDG_STATE_HOME`.
    pub xdg_state_home: Option<String>,
}

// --- The resolved template, for the forest ops crate's other users ---

pub(crate) use git_forest_core::plan::new::ResolvedTemplate;

// --- XDG path helpers ---

/// Returns the XDG config directory for the application `app_name`.
///
/// Resolution order:
/// 1. `$XDG_CONFIG_HOME/<app_name>/` (if set and not empty)
/// 2. `~/.config/<app_name>/` (Unix/macOS default, with `HOME` even when empty)
/// 3. `directories` crate fallback (Windows, or when `HOME` is unset)
pub(crate) fn xdg_config_dir(app_name: &str, environment: &Environment) -> Result<PathBuf> {
    xdg_dir(
        app_name,
        environment.xdg_config_home.as_deref(),
        environment.home.as_deref(),
        ".config",
        |proj| proj.config_dir().to_path_buf(),
    )
}

/// Returns the XDG state directory for the application `app_name`.
///
/// Resolution order:
/// 1. `$XDG_STATE_HOME/<app_name>/` (if set and not empty)
/// 2. `~/.local/state/<app_name>/` (Unix/macOS default, with `HOME` even when empty)
/// 3. `directories` crate fallback (Windows, or when `HOME` is unset)
pub fn xdg_state_dir(app_name: &str, environment: &Environment) -> Result<PathBuf> {
    xdg_dir(
        app_name,
        environment.xdg_state_home.as_deref(),
        environment.home.as_deref(),
        ".local/state",
        |proj| {
            proj.state_dir()
                .unwrap_or_else(|| proj.data_local_dir())
                .to_path_buf()
        },
    )
}

/// The directory of `app_name` under `explicit` (the XDG variable) when it is set and not
/// empty, else under `home` joined with `default_suffix` on Unix, else from `fallback`.
fn xdg_dir(
    app_name: &str,
    explicit: Option<&str>,
    home: Option<&str>,
    default_suffix: &str,
    fallback: fn(&directories::ProjectDirs) -> PathBuf,
) -> Result<PathBuf> {
    // 1. Explicit env var
    if let Some(val) = explicit {
        if !val.is_empty() {
            return Ok(PathBuf::from(val).join(app_name));
        }
    }

    // 2. Unix/macOS default: ~/.<suffix>/git-forest/
    #[cfg(unix)]
    {
        if let Some(home) = home {
            return Ok(PathBuf::from(home).join(default_suffix).join(app_name));
        }
    }

    // 3. directories crate fallback (Windows, or if HOME is unset)
    #[cfg(not(unix))]
    let _ = (home, default_suffix);
    let proj = directories::ProjectDirs::from("", "", app_name)
        .context("could not determine config directory")?;
    Ok(fallback(&proj))
}

// --- Config loading ---

/// The config file of the application `app_name`, in its XDG config directory.
pub fn default_config_path(app_name: &str, environment: &Environment) -> Result<PathBuf> {
    Ok(xdg_config_dir(app_name, environment)?.join("config.toml"))
}

/// Loads the config file at `path`, which the composition root resolved, with a hint to run
/// `init` when it does not exist; its `~` paths expand with `home`.
pub fn load_default_config(path: &Path, home: Option<&str>) -> Result<ResolvedConfig> {
    ensure!(
        path.exists(),
        "config not found at {}\n  hint: run `git forest init` to create one",
        path.display()
    );
    load_config(path, home)
}

/// Reads and parses the config file at `path`; its `~` paths expand with `home`.
pub(crate) fn load_config(path: &Path, home: Option<&str>) -> Result<ResolvedConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    parse_config(&contents, home)
}

/// Whether the update check is enabled: the config's `version_check.enabled`, read leniently.
/// An unresolved location (`None`), a missing, unreadable or unparsable file, and a config
/// without a `[version_check]` table all read as enabled.
pub fn version_check_enabled(config_file: Option<&Path>) -> bool {
    let Some(path) = config_file else {
        return true;
    };
    match std::fs::read_to_string(path) {
        Ok(contents) => version_check_enabled_in(&contents),
        Err(_) => true,
    }
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

/// Writes `config`, rendered by the core, to `path` through a temporary file and a rename,
/// creating the directory first with a diagnosis when permission is denied.
pub(crate) fn write_config_atomic(path: &Path, config: &ResolvedConfig) -> Result<()> {
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

    let content = render_config(config)?;

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

    /// A fixed fake home directory, so that every expected path proves the passed value is used.
    const TEST_HOME: &str = "/home/tester";

    #[test]
    fn write_config_round_trips_disposable_root_entries() {
        let parsed = parse_config(
            r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "main"
feature_branch_template = "user/{name}"
disposable_root_entries = [".idea", ".claude"]

[[template.default.repos]]
path = "/tmp/src/repo-a"
"#,
            Some(TEST_HOME),
        )
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");

        write_config_atomic(&path, &parsed).unwrap();
        let reparsed = load_config(&path, Some(TEST_HOME)).unwrap();

        assert_eq!(
            reparsed
                .resolve_template(None)
                .unwrap()
                .disposable_root_entries,
            parsed
                .resolve_template(None)
                .unwrap()
                .disposable_root_entries
        );
    }

    // --- Multi-template tests ---

    #[test]
    fn version_check_enabled_reads_the_flag_leniently() {
        let tmp = tempfile::tempdir().unwrap();
        let config = r#"
default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;
        let write = |name: &str, contents: &str| {
            let path = tmp.path().join(name);
            std::fs::write(&path, contents).unwrap();
            path
        };

        assert!(super::version_check_enabled(None));
        assert!(super::version_check_enabled(Some(
            &tmp.path().join("missing.toml")
        )));
        let unparsable = write("unparsable.toml", "default_template = [");
        assert!(super::version_check_enabled(Some(&unparsable)));
        let without_table = write("without-table.toml", config);
        assert!(super::version_check_enabled(Some(&without_table)));
        let disabled = write(
            "disabled.toml",
            &format!("{config}\n[version_check]\nenabled = false\n"),
        );
        assert!(!super::version_check_enabled(Some(&disabled)));
        let enabled = write(
            "enabled.toml",
            &format!("{config}\n[version_check]\nenabled = true\n"),
        );
        assert!(super::version_check_enabled(Some(&enabled)));
    }

    // --- XDG path tests ---

    #[test]
    fn xdg_config_dir_respects_env_var() {
        let environment = Environment {
            home: Some(TEST_HOME.to_string()),
            xdg_config_home: Some("/tmp/test-xdg-config".to_string()),
            xdg_state_home: None,
        };
        let result = super::xdg_config_dir("git-forest", &environment).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/tmp/test-xdg-config").join("git-forest")
        );
    }

    #[test]
    fn xdg_state_dir_respects_env_var() {
        let environment = Environment {
            home: Some(TEST_HOME.to_string()),
            xdg_config_home: None,
            xdg_state_home: Some("/tmp/test-xdg-state".to_string()),
        };
        let result = super::xdg_state_dir("git-forest", &environment).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/tmp/test-xdg-state").join("git-forest")
        );
    }

    #[cfg(unix)]
    #[test]
    fn xdg_config_dir_defaults_to_dot_config() {
        let environment = Environment {
            home: Some("/home/testuser".to_string()),
            xdg_config_home: None,
            xdg_state_home: None,
        };
        let result = super::xdg_config_dir("git-forest", &environment).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/home/testuser/.config").join("git-forest")
        );
    }

    #[cfg(unix)]
    #[test]
    fn xdg_state_dir_defaults_to_dot_local_state() {
        let environment = Environment {
            home: Some("/home/testuser".to_string()),
            xdg_config_home: None,
            xdg_state_home: None,
        };
        let result = super::xdg_state_dir("git-forest", &environment).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/home/testuser/.local/state").join("git-forest")
        );
    }

    #[test]
    fn default_config_path_ends_with_config_toml() {
        let environment = Environment {
            home: Some(TEST_HOME.to_string()),
            xdg_config_home: Some("/tmp/test-xdg-config".to_string()),
            xdg_state_home: None,
        };
        let path = super::default_config_path("git-forest", &environment).unwrap();
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
