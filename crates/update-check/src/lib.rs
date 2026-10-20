//! The update check: a client of the update server. It caches the latest released version in
//! its state file, prints an update notice from that cache after a command, and refreshes the
//! cache in a detached background process. It never names another workspace crate: the
//! composition root passes the running binary's version, the app and command names, the release
//! channel, the state file and the config file in one `Settings` value, and the update check reads
//! its own `[version_check]` table from that config file.
//!
//! Host protocol: when the cache is missing or stale, `check_cache_and_notify` spawns the host's
//! own executable with `INTERNAL_VERSION_CHECK_ARG` and passes no settings. The host must
//! intercept that argument before parsing its command line, rebuild the same settings in that
//! process and call `run_background_version_check`. A second host must honour this handshake or
//! supply its own spawning boundary; a custom `state_file` given to the foreground call does not
//! reach the background process by itself.
#![expect(
    clippy::disallowed_methods,
    clippy::print_stderr,
    reason = "the update-check edge: reads the clock, resolves and spawns its own executable, makes the update server request and prints its notices"
)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use ureq::Agent;

/// The argument the background refresh is started with; the host intercepts it before parsing
/// its command line (see the crate documentation).
pub const INTERNAL_VERSION_CHECK_ARG: &str = "--internal-version-check";

/// The state file's name; the host chooses its directory.
pub const STATE_FILE_NAME: &str = "state.toml";

const VERSION_CHECK_BASE_URL: &str = "https://forest.dliv.gg/api/latest";

const CHECK_INTERVAL_HOURS: i64 = 24;

/// Whether the update check runs, as its own `[version_check]` table in the config file says:
/// only `enabled = false` disables it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Enablement {
    Enabled,
    Disabled,
}

/// What the host tells the update check about itself. The host builds it the same way in the
/// foreground and in the background process (see the crate documentation).
#[derive(Clone, Debug)]
pub struct Settings {
    /// The running binary's version, read by the host package at build time.
    pub current_version: &'static str,
    /// The application's name, as notices and the request's user agent show it.
    pub app_name: &'static str,
    /// How the user runs the tool, as the upgrade hint shows it.
    pub command: &'static str,
    /// The release channel sent to the update server.
    pub channel: &'static str,
    /// The config file, from which the update check reads its own `[version_check]` table;
    /// None when its location could not be resolved, which reads as enabled.
    pub config_file: Option<PathBuf>,
    /// The state file; None when its directory could not be resolved, which skips every read
    /// and write of it.
    pub state_file: Option<PathBuf>,
}

pub struct UpdateNotice {
    pub current: String,
    pub latest: String,
}

pub enum ForceCheckResult {
    /// The config's `[version_check] enabled = false`; nothing was fetched.
    Disabled,
    UpdateAvailable(UpdateNotice),
    UpToDate,
    FetchFailed,
}

// --- Config file ---

/// The config file as the update check reads it: its own `[version_check]` table and nothing
/// else. Serde skips every other key without converting its value, so a value the update check
/// does not own, such as an integer `toml::Value` cannot hold, cannot change the result.
#[derive(Deserialize)]
struct ConfigView {
    version_check: Option<toml::Table>,
}

/// The update check's own section of the config file, read leniently: no path, a file that
/// cannot be read or is not TOML, a missing or non-table `version_check` or one TOML values
/// cannot represent, and an `enabled` that is missing or not a boolean all read as enabled.
/// Only `enabled = false` disables the check.
fn config_enablement(config_file: Option<&Path>) -> Enablement {
    let Some(path) = config_file else {
        return Enablement::Enabled;
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Enablement::Enabled;
    };
    let Ok(config) = toml::from_str::<ConfigView>(&contents) else {
        return Enablement::Enabled;
    };
    let enabled = config
        .version_check
        .as_ref()
        .and_then(|table| table.get("enabled"))
        .and_then(toml::Value::as_bool);
    match enabled {
        Some(false) => Enablement::Disabled,
        Some(true) | None => Enablement::Enabled,
    }
}

// --- State file ---

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    version_check: Option<VersionCheckState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionCheckState {
    last_checked: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latest_version: Option<String>,
}

fn read_state(state_file: Option<&Path>) -> Option<VersionCheckState> {
    let path = state_file?;
    let contents = std::fs::read_to_string(path).ok()?;
    let state: StateFile = toml::from_str(&contents).ok()?;
    state.version_check
}

fn write_state(state_file: Option<&Path>, state: &VersionCheckState) -> Option<()> {
    let path = state_file?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let file = StateFile {
        version_check: Some(state.clone()),
    };
    let content = toml::to_string_pretty(&file).ok()?;
    std::fs::write(path, content).ok()
}

/// Read-modify-write the state file, applying `f` to the existing state.
/// Creates a default state if none exists.
fn update_state(state_file: Option<&Path>, f: impl FnOnce(&mut VersionCheckState)) -> Option<()> {
    let path = state_file?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let mut state = read_state(Some(path)).unwrap_or(VersionCheckState {
        last_checked: Utc::now(),
        latest_version: None,
    });
    f(&mut state);
    let file = StateFile {
        version_check: Some(state),
    };
    let content = toml::to_string_pretty(&file).ok()?;
    std::fs::write(path, content).ok()
}

fn is_stale(last_checked: &DateTime<Utc>) -> bool {
    let elapsed = Utc::now().signed_duration_since(last_checked);
    elapsed.num_hours() >= CHECK_INTERVAL_HOURS
}

// --- Network ---

#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export_to = "worker/src/generated/"))]
#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

fn fetch_latest_version(settings: &Settings, debug: bool, timeout: Duration) -> Option<String> {
    let current = settings.current_version;
    let url = format!(
        "{}?v={}&channel={}",
        VERSION_CHECK_BASE_URL, current, settings.channel
    );

    if debug {
        eprintln!("[debug] version check: fetching {}", url);
    }

    let config = Agent::config_builder()
        .timeout_global(Some(timeout))
        .build();
    let agent: Agent = config.into();

    let body = agent
        .get(&url)
        .header("User-Agent", &format!("{}/{}", settings.app_name, current))
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;

    if debug {
        eprintln!("[debug] version check: response {}", body);
    }

    let resp: VersionResponse = serde_json::from_str(&body).ok()?;

    Some(resp.version)
}

// --- Comparison ---

fn version_newer(latest: &str, current: &str) -> bool {
    let latest = semver::Version::parse(latest).ok();
    let current = semver::Version::parse(current).ok();
    match (latest, current) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

// --- Background subprocess ---

fn spawn_background_check() {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };
    let _ = std::process::Command::new(exe)
        .arg(INTERNAL_VERSION_CHECK_ARG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Child handle is dropped — parent does not wait
}

/// Entry point for the detached subprocess spawned by `spawn_background_check`.
/// Fetches the latest version with a generous timeout and writes it to the state file.
/// It reads no config: the foreground call that spawned it already did.
pub fn run_background_version_check(settings: &Settings) {
    if let Some(latest) = fetch_latest_version(settings, false, Duration::from_secs(5)) {
        update_state(settings.state_file.as_deref(), |s| {
            s.latest_version = Some(latest)
        });
    }
}

// --- Public API ---

/// Called after successful commands. Non-blocking: reads from cache and prints
/// an update notice if one is available, then spawns a background subprocess
/// to refresh the cache if stale. Never blocks on network I/O.
pub fn check_cache_and_notify(settings: &Settings, debug: bool) {
    match config_enablement(settings.config_file.as_deref()) {
        Enablement::Disabled => {
            if debug {
                eprintln!("[debug] version check: disabled in config");
            }
            return;
        }
        Enablement::Enabled => {}
    }

    let current = settings.current_version;
    let state_file = settings.state_file.as_deref();
    let state = read_state(state_file);

    if state.is_none() {
        // First run: show privacy notice, seed state, spawn background check
        if debug {
            eprintln!("[debug] version check: state file not found, first run");
        }
        eprintln!(
            "Note: {} checks for updates daily (current version sent to forest.dliv.gg).",
            settings.app_name
        );
        eprintln!("Disable: set version_check.enabled = false in config.");
        write_state(
            state_file,
            &VersionCheckState {
                last_checked: Utc::now(),
                latest_version: None,
            },
        );
        spawn_background_check();
        return;
    }

    let cached = state.unwrap();

    // Show update notice from cache if available
    if let Some(ref latest) = cached.latest_version {
        if version_newer(latest, current) {
            eprintln!(
                "Update available: {} v{} (current: v{}). Run `{} update` to upgrade.",
                settings.app_name, latest, current, settings.command
            );
        }
    }

    // Refresh if stale (>= 24h) or if latest_version is still None (previous bg check failed)
    if cached.latest_version.is_none() || is_stale(&cached.last_checked) {
        if debug {
            eprintln!(
                "[debug] version check: cache stale or incomplete, spawning background check"
            );
        }
        update_state(state_file, |s| s.last_checked = Utc::now());
        spawn_background_check();
    } else if debug {
        eprintln!(
            "[debug] version check: cache fresh, latest={:?}",
            cached.latest_version
        );
    }
}

/// Called by `git forest version --check`. Forces a synchronous network check, unless the
/// config's `[version_check] enabled = false` disables the check, which it reports without
/// fetching.
pub fn force_check(settings: &Settings, debug: bool) -> ForceCheckResult {
    match config_enablement(settings.config_file.as_deref()) {
        Enablement::Disabled => return ForceCheckResult::Disabled,
        Enablement::Enabled => {}
    }
    let current = settings.current_version;
    match fetch_latest_version(settings, debug, Duration::from_secs(5)) {
        Some(latest) => {
            write_state(
                settings.state_file.as_deref(),
                &VersionCheckState {
                    last_checked: Utc::now(),
                    latest_version: Some(latest.clone()),
                },
            );
            if version_newer(&latest, current) {
                ForceCheckResult::UpdateAvailable(UpdateNotice {
                    current: current.to_string(),
                    latest,
                })
            } else {
                ForceCheckResult::UpToDate
            }
        }
        None => ForceCheckResult::FetchFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_newer_upgrade_available() {
        assert!(version_newer("0.2.0", "0.1.0"));
    }

    #[test]
    fn version_newer_same() {
        assert!(!version_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn version_newer_downgrade() {
        assert!(!version_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn version_newer_invalid_latest() {
        assert!(!version_newer("invalid", "0.1.0"));
    }

    #[test]
    fn version_newer_invalid_current() {
        assert!(!version_newer("0.2.0", "invalid"));
    }

    #[test]
    fn state_file_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let state_path = tmp.path().join("state.toml");

        let state = VersionCheckState {
            last_checked: Utc::now(),
            latest_version: Some("0.2.0".to_string()),
        };

        let file = StateFile {
            version_check: Some(state),
        };
        let content = toml::to_string_pretty(&file).unwrap();
        std::fs::write(&state_path, &content).unwrap();

        let read_back: StateFile =
            toml::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        let vc = read_back.version_check.unwrap();
        assert_eq!(vc.latest_version, Some("0.2.0".to_string()));
    }

    #[test]
    fn state_file_without_latest_version() {
        let toml_str = r#"
[version_check]
last_checked = "2026-01-01T00:00:00Z"
"#;
        let state: StateFile = toml::from_str(toml_str).unwrap();
        let vc = state.version_check.unwrap();
        assert_eq!(vc.latest_version, None);
    }

    #[test]
    fn staleness_check_old() {
        let old = Utc::now() - chrono::Duration::hours(25);
        assert!(is_stale(&old));
    }

    #[test]
    fn staleness_check_recent() {
        let recent = Utc::now() - chrono::Duration::hours(1);
        assert!(!is_stale(&recent));
    }

    /// A forest config without a `[version_check]` table, ending inside its one repo's table.
    const FOREST_CONFIG: &str = r#"default_template = "default"

[template.default]
worktree_base = "/tmp/worktrees"
base_branch = "dev"
feature_branch_template = "user/{name}"

[[template.default.repos]]
path = "/tmp/src/foo-api"
"#;

    #[test]
    fn config_enablement_reads_only_enabled_false_as_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let forest = FOREST_CONFIG;
        let write = |name: &str, contents: &str| {
            let path = tmp.path().join(name);
            std::fs::write(&path, contents).unwrap();
            path
        };
        let read = |path: &Path| config_enablement(Some(path));

        assert_eq!(config_enablement(None), Enablement::Enabled);
        assert_eq!(read(&tmp.path().join("missing.toml")), Enablement::Enabled);
        assert_eq!(read(tmp.path()), Enablement::Enabled);
        let enabled = [
            ("not-toml.toml", "default_template = [".to_string()),
            ("without-table.toml", forest.to_string()),
            (
                "without-enabled.toml",
                format!("{forest}\n[version_check]\n"),
            ),
            (
                "enabled-true.toml",
                format!("{forest}\n[version_check]\nenabled = true\n"),
            ),
            (
                "enabled-string.toml",
                format!("{forest}\n[version_check]\nenabled = \"no\"\n"),
            ),
            ("not-a-table.toml", "version_check = 5\n".to_string()),
        ];
        for (name, contents) in enabled {
            assert_eq!(read(&write(name, &contents)), Enablement::Enabled, "{name}");
        }
        let disabled = [
            (
                "disabled.toml",
                format!("{forest}\n[version_check]\nenabled = false\n"),
            ),
            (
                "disabled-without-forest-keys.toml",
                format!(
                    "{}\n[version_check]\nenabled = false\n",
                    forest.replace("default_template = \"default\"\n", "")
                ),
            ),
            (
                "disabled-alone.toml",
                "[version_check]\nenabled = false\n".to_string(),
            ),
        ];
        for (name, contents) in disabled {
            assert_eq!(
                read(&write(name, &contents)),
                Enablement::Disabled,
                "{name}"
            );
        }
    }

    #[test]
    fn config_enablement_skips_keys_it_does_not_own() {
        // TOML integers are signed 64-bit, and `toml::Value` cannot hold one outside that range.
        // In a key the update check does not own, such a value must not undo an explicit
        // opt-out; in its own section it is malformed and reads as enabled.
        const WIDE: &str = "9223372036854775808";
        const NEGATIVE_WIDE: &str = "-9223372036854775809";
        let tmp = tempfile::tempdir().unwrap();
        let read = |name: &str, contents: String| {
            let path = tmp.path().join(name);
            std::fs::write(&path, contents).unwrap();
            config_enablement(Some(&path))
        };
        let opt_out = "\n[version_check]\nenabled = false\n";

        let unowned = [
            (
                "in-a-repo.toml",
                format!("{FOREST_CONFIG}external_id = {WIDE}\n{opt_out}"),
            ),
            (
                "in-a-template.toml",
                FOREST_CONFIG.replace(
                    "[[template.default.repos]]",
                    &format!("external_id = {WIDE}\n\n[[template.default.repos]]"),
                ) + opt_out,
            ),
            (
                "top-level.toml",
                format!("external_id = {WIDE}\n{FOREST_CONFIG}{opt_out}"),
            ),
            (
                "top-level-negative.toml",
                format!("external_id = {NEGATIVE_WIDE}\n{FOREST_CONFIG}{opt_out}"),
            ),
            (
                "in-another-table.toml",
                format!("{FOREST_CONFIG}\n[other]\nexternal_id = {WIDE}\n{opt_out}"),
            ),
        ];
        for (name, contents) in unowned {
            assert_eq!(read(name, contents), Enablement::Disabled, "{name}");
        }

        let own = format!("{FOREST_CONFIG}{opt_out}external_id = {WIDE}\n");
        assert_eq!(read("in-its-own-section.toml", own), Enablement::Enabled);
    }

    #[test]
    fn check_cache_and_notify_honors_a_disabled_config() {
        let tmp = tempfile::tempdir().unwrap();
        let config_file = tmp.path().join("config.toml");
        std::fs::write(&config_file, "[version_check]\nenabled = false\n").unwrap();
        let state_file = tmp.path().join("state").join("state.toml");
        let settings = Settings {
            current_version: "0.0.0",
            app_name: "git-forest",
            command: "git forest",
            channel: "stable",
            config_file: Some(config_file),
            state_file: Some(state_file.clone()),
        };

        check_cache_and_notify(&settings, false);

        assert!(
            !state_file.exists(),
            "a disabled check must not write {}",
            state_file.display()
        );
    }

    #[test]
    #[ignore]
    fn export_bindings() {
        use ts_rs::TS;
        VersionResponse::export_all(
            &ts_rs::Config::from_env().with_out_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap();
    }
}
