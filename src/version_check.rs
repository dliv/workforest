use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use ureq::Agent;

use crate::channel;
const CHECK_INTERVAL_HOURS: i64 = 24;

pub struct UpdateNotice {
    pub current: String,
    pub latest: String,
}

pub enum ForceCheckResult {
    UpdateAvailable(UpdateNotice),
    UpToDate,
    FetchFailed,
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

fn state_file_path() -> Option<PathBuf> {
    let dir = crate::config::xdg_state_dir().ok()?;
    Some(dir.join("state.toml"))
}

fn read_state() -> Option<VersionCheckState> {
    let path = state_file_path()?;
    let contents = std::fs::read_to_string(path).ok()?;
    let state: StateFile = toml::from_str(&contents).ok()?;
    state.version_check
}

fn write_state(state: &VersionCheckState) -> Option<()> {
    let path = state_file_path()?;
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
fn update_state(f: impl FnOnce(&mut VersionCheckState)) -> Option<()> {
    let path = state_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let mut state = read_state().unwrap_or(VersionCheckState {
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
#[cfg_attr(test, ts(export, export_to = "worker/src/generated/"))]
#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

fn fetch_latest_version(current: &str, debug: bool, timeout: Duration) -> Option<String> {
    let url = format!(
        "{}?v={}&channel={}",
        channel::VERSION_CHECK_BASE_URL,
        current,
        channel::VERSION_CHANNEL
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
        .header("User-Agent", &format!("{}/{}", channel::APP_NAME, current))
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
        .arg(channel::INTERNAL_VERSION_CHECK_ARG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    // Child handle is dropped — parent does not wait
}

/// Entry point for the detached subprocess spawned by `spawn_background_check`.
/// Fetches the latest version with a generous timeout and writes it to the state file.
pub fn run_background_version_check() {
    let current = env!("CARGO_PKG_VERSION");
    if let Some(latest) = fetch_latest_version(current, false, Duration::from_secs(5)) {
        update_state(|s| s.latest_version = Some(latest));
    }
}

// --- Config integration ---

/// Returns true if version checking is enabled in config.
/// Returns true if config doesn't exist or can't be read (default-on).
pub fn is_enabled() -> bool {
    let path = match crate::config::default_config_path() {
        Ok(p) => p,
        Err(_) => return true,
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return true,
    };
    // Parse just the raw TOML to check version_check.enabled
    let raw: Result<crate::config::MultiTemplateConfig, _> = toml::from_str(&contents);
    match raw {
        Ok(config) => config.version_check.map(|vc| vc.enabled).unwrap_or(true),
        Err(_) => true,
    }
}

// --- Public API ---

/// Called after successful commands. Non-blocking: reads from cache and prints
/// an update notice if one is available, then spawns a background subprocess
/// to refresh the cache if stale. Never blocks on network I/O.
pub fn check_cache_and_notify(debug: bool) {
    if !is_enabled() {
        if debug {
            eprintln!("[debug] version check: disabled in config");
        }
        return;
    }

    let current = env!("CARGO_PKG_VERSION");
    let state = read_state();

    if state.is_none() {
        // First run: show privacy notice, seed state, spawn background check
        if debug {
            eprintln!("[debug] version check: state file not found, first run");
        }
        eprintln!(
            "Note: {} checks for updates daily (current version sent to forest.dliv.gg).",
            channel::APP_NAME
        );
        eprintln!("Disable: set version_check.enabled = false in config.");
        write_state(&VersionCheckState {
            last_checked: Utc::now(),
            latest_version: None,
        });
        spawn_background_check();
        return;
    }

    let cached = state.unwrap();

    // Show update notice from cache if available
    if let Some(ref latest) = cached.latest_version {
        if version_newer(latest, current) {
            eprintln!(
                "Update available: {} v{} (current: v{}). Run `git forest update` to upgrade.",
                channel::APP_NAME,
                latest,
                current
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
        update_state(|s| s.last_checked = Utc::now());
        spawn_background_check();
    } else if debug {
        eprintln!(
            "[debug] version check: cache fresh, latest={:?}",
            cached.latest_version
        );
    }
}

/// Called by `git forest version --check`. Forces a synchronous network check.
pub fn force_check(debug: bool) -> ForceCheckResult {
    let current = env!("CARGO_PKG_VERSION");
    match fetch_latest_version(current, debug, Duration::from_secs(5)) {
        Some(latest) => {
            write_state(&VersionCheckState {
                last_checked: Utc::now(),
                latest_version: Some(latest.clone()),
            });
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

    #[test]
    #[ignore]
    fn export_bindings() {
        use ts_rs::TS;
        VersionResponse::export_all().unwrap();
    }
}
