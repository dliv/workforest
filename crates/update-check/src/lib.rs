//! The update check: a client of the update server. It owns the cache and protocol workflow,
//! including reading its `[version_check]` table, printing notices and deciding whether a
//! refresh is needed. The host supplies its concrete HTTP agent, endpoint, and clock through
//! `Settings` and the public function arguments. `check_cache_and_notify` returns a disposition;
//! the host decides how to launch a requested one-shot refresh. This crate creates no process or
//! runtime, so a second host can call the same cache and refresh workflows directly.
#![expect(
    clippy::disallowed_methods,
    clippy::print_stderr,
    reason = "the update-check edge: makes the update server request through a host-supplied agent and prints its notices"
)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use ureq::Agent;

/// The state file's name; the host chooses its directory.
pub const STATE_FILE_NAME: &str = "state.toml";

const CHECK_INTERVAL_HOURS: i64 = 24;

/// Whether the update check runs, as its own `[version_check]` table in the config file says:
/// only `enabled = false` disables it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Enablement {
    Enabled,
    Disabled,
}

/// Host-owned settings used by the update-check cache and protocol workflows.
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
    /// The update endpoint selected by trusted host code.
    pub endpoint: String,
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

/// Whether the host should launch its own background refresh after displaying cached information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshDisposition {
    NoRefresh,
    RefreshRequested,
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
fn update_state(
    state_file: Option<&Path>,
    now: &impl Fn() -> DateTime<Utc>,
    f: impl FnOnce(&mut VersionCheckState),
) -> Option<()> {
    let path = state_file?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let mut state = read_state(Some(path)).unwrap_or(VersionCheckState {
        last_checked: now(),
        latest_version: None,
    });
    f(&mut state);
    let file = StateFile {
        version_check: Some(state),
    };
    let content = toml::to_string_pretty(&file).ok()?;
    std::fs::write(path, content).ok()
}

fn is_stale(last_checked: &DateTime<Utc>, now: &impl Fn() -> DateTime<Utc>) -> bool {
    let elapsed = now().signed_duration_since(last_checked);
    elapsed.num_hours() >= CHECK_INTERVAL_HOURS
}

// --- Network ---

#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export_to = "worker/src/generated/"))]
#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

fn fetch_latest_version(settings: &Settings, agent: &Agent, debug: bool) -> Option<String> {
    let current = settings.current_version;
    let url = format!(
        "{}?v={}&channel={}",
        settings.endpoint, current, settings.channel
    );

    if debug {
        eprintln!("[debug] version check: fetching {}", url);
    }

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

/// Performs one background refresh using the host's configured HTTP agent and clock.
/// A successful response updates the cached version; fetch and persistence failures remain best
/// effort and do not change the caller's process status.
pub fn run_background_version_check(
    settings: &Settings,
    agent: &Agent,
    now: impl Fn() -> DateTime<Utc>,
) {
    if let Some(latest) = fetch_latest_version(settings, agent, false) {
        update_state(settings.state_file.as_deref(), &now, |s| {
            s.latest_version = Some(latest)
        });
    }
}

// --- Public API ---

/// Reads the cached notice and reports whether the host should launch a refresh.
/// It never performs network I/O or starts a process.
pub fn check_cache_and_notify(
    settings: &Settings,
    debug: bool,
    now: impl Fn() -> DateTime<Utc>,
) -> RefreshDisposition {
    match config_enablement(settings.config_file.as_deref()) {
        Enablement::Disabled => {
            if debug {
                eprintln!("[debug] version check: disabled in config");
            }
            return RefreshDisposition::NoRefresh;
        }
        Enablement::Enabled => {}
    }

    let current = settings.current_version;
    let state_file = settings.state_file.as_deref();
    let state = read_state(state_file);

    if state.is_none() {
        // First run: show the privacy notice, seed state and request a background refresh.
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
                last_checked: now(),
                latest_version: None,
            },
        );
        return RefreshDisposition::RefreshRequested;
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
    if cached.latest_version.is_none() || is_stale(&cached.last_checked, &now) {
        if debug {
            eprintln!(
                "[debug] version check: cache stale or incomplete, spawning background check"
            );
        }
        update_state(state_file, &now, |s| s.last_checked = now());
        RefreshDisposition::RefreshRequested
    } else if debug {
        eprintln!(
            "[debug] version check: cache fresh, latest={:?}",
            cached.latest_version
        );
        RefreshDisposition::NoRefresh
    } else {
        RefreshDisposition::NoRefresh
    }
}

/// Called by `git forest version --check`. Forces a synchronous network check, unless the
/// config's `[version_check] enabled = false` disables the check, which it reports without
/// fetching.
pub fn force_check(
    settings: &Settings,
    agent: &Agent,
    debug: bool,
    now: impl Fn() -> DateTime<Utc>,
) -> ForceCheckResult {
    match config_enablement(settings.config_file.as_deref()) {
        Enablement::Disabled => return ForceCheckResult::Disabled,
        Enablement::Enabled => {}
    }
    let current = settings.current_version;
    match fetch_latest_version(settings, agent, debug) {
        Some(latest) => {
            write_state(
                settings.state_file.as_deref(),
                &VersionCheckState {
                    last_checked: now(),
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
    use std::cell::Cell;

    fn at(value: &str) -> DateTime<Utc> {
        value.parse().unwrap()
    }

    fn test_settings(config_file: Option<PathBuf>, state_file: Option<PathBuf>) -> Settings {
        Settings {
            current_version: "0.2.0",
            app_name: "git-forest",
            command: "git forest",
            channel: "stable",
            endpoint: "https://forest.dliv.gg/api/latest".to_owned(),
            config_file,
            state_file,
        }
    }

    fn save_state(path: &Path, checked: DateTime<Utc>, latest: Option<&str>) {
        let state = VersionCheckState {
            last_checked: checked,
            latest_version: latest.map(str::to_owned),
        };
        write_state(Some(path), &state).unwrap();
    }

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
            last_checked: at("2026-10-22T19:00:00Z"),
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
        let now = at("2026-10-22T20:00:00Z");
        let old = now - chrono::Duration::hours(25);
        assert!(is_stale(&old, &|| now));
    }

    #[test]
    fn staleness_check_recent() {
        let now = at("2026-10-22T20:00:00Z");
        let recent = now - chrono::Duration::hours(1);
        assert!(!is_stale(&recent, &|| now));
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
        let settings = test_settings(Some(config_file), Some(state_file.clone()));
        let clock_reads = Cell::new(0);

        assert_eq!(
            check_cache_and_notify(&settings, false, || {
                clock_reads.set(clock_reads.get() + 1);
                at("2026-10-22T20:00:00Z")
            }),
            RefreshDisposition::NoRefresh
        );

        assert_eq!(clock_reads.get(), 0);
        assert!(
            !state_file.exists(),
            "a disabled check must not write {}",
            state_file.display()
        );
    }

    #[test]
    fn cache_workflow_returns_disposition_and_preserves_fresh_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state_file = tmp.path().join("state.toml");
        let now = at("2026-10-22T20:00:00Z");
        save_state(
            &state_file,
            now - chrono::Duration::hours(23) - chrono::Duration::minutes(59),
            Some("0.2.0"),
        );
        let before = std::fs::read_to_string(&state_file).unwrap();
        let settings = test_settings(None, Some(state_file.clone()));

        assert_eq!(
            check_cache_and_notify(&settings, false, || now),
            RefreshDisposition::NoRefresh
        );
        assert_eq!(std::fs::read_to_string(state_file).unwrap(), before);
    }

    #[test]
    fn missing_and_malformed_state_seed_the_clock_and_request_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let now = at("2026-10-22T20:00:00Z");
        let missing = tmp.path().join("missing/state.toml");
        let settings = test_settings(None, Some(missing.clone()));

        assert_eq!(
            check_cache_and_notify(&settings, false, || now),
            RefreshDisposition::RefreshRequested
        );
        let state = read_state(Some(&missing)).unwrap();
        assert_eq!(state.last_checked, now);
        assert_eq!(state.latest_version, None);

        let malformed = tmp.path().join("malformed/state.toml");
        std::fs::create_dir_all(malformed.parent().unwrap()).unwrap();
        std::fs::write(&malformed, "not valid TOML = [").unwrap();
        let settings = test_settings(None, Some(malformed.clone()));
        assert_eq!(
            check_cache_and_notify(&settings, false, || now),
            RefreshDisposition::RefreshRequested
        );
        let state = read_state(Some(&malformed)).unwrap();
        assert_eq!(state.last_checked, now);
        assert_eq!(state.latest_version, None);
    }

    #[test]
    fn stale_threshold_is_twenty_four_whole_hours_and_future_state_is_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let now = at("2026-10-22T20:00:00Z");
        let under = tmp.path().join("under/state.toml");
        save_state(
            &under,
            now - chrono::Duration::hours(24) + chrono::Duration::seconds(1),
            Some("0.2.0"),
        );
        assert_eq!(
            check_cache_and_notify(&test_settings(None, Some(under.clone())), false, || now),
            RefreshDisposition::NoRefresh
        );

        let exact = tmp.path().join("exact/state.toml");
        save_state(&exact, now - chrono::Duration::hours(24), Some("0.2.0"));
        assert_eq!(
            check_cache_and_notify(&test_settings(None, Some(exact.clone())), false, || now),
            RefreshDisposition::RefreshRequested
        );
        assert_eq!(read_state(Some(&exact)).unwrap().last_checked, now);

        let future = tmp.path().join("future/state.toml");
        save_state(&future, now + chrono::Duration::days(1), Some("0.2.0"));
        assert_eq!(
            check_cache_and_notify(&test_settings(None, Some(future)), false, || now),
            RefreshDisposition::NoRefresh
        );
    }

    #[test]
    fn incomplete_cache_requests_refresh_even_when_its_timestamp_is_future() {
        let tmp = tempfile::tempdir().unwrap();
        let state_file = tmp.path().join("state.toml");
        let now = at("2026-10-22T20:00:00Z");
        save_state(&state_file, now + chrono::Duration::days(1), None);

        assert_eq!(
            check_cache_and_notify(
                &test_settings(None, Some(state_file.clone())),
                false,
                || now
            ),
            RefreshDisposition::RefreshRequested
        );
        let state = read_state(Some(&state_file)).unwrap();
        assert_eq!(state.last_checked, now);
        assert_eq!(state.latest_version, None);
    }

    #[test]
    fn missing_state_path_still_requests_refresh_without_persisting() {
        let settings = test_settings(None, None);
        assert_eq!(
            check_cache_and_notify(&settings, false, || at("2026-10-22T20:00:00Z")),
            RefreshDisposition::RefreshRequested
        );
    }

    #[test]
    fn failed_state_write_does_not_change_refresh_disposition() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let state_file = blocker.join("state.toml");

        assert_eq!(
            check_cache_and_notify(
                &test_settings(None, Some(state_file.clone())),
                false,
                || { at("2026-10-22T20:00:00Z") }
            ),
            RefreshDisposition::RefreshRequested
        );
        assert!(!state_file.exists());
        assert_eq!(std::fs::read_to_string(blocker).unwrap(), "not a directory");
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
