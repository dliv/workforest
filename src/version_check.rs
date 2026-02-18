use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use ureq::Agent;

const VERSION_CHECK_URL: &str = "https://forest.dliv.gg/api/latest";
const CHECK_INTERVAL_HOURS: i64 = 24;

pub struct UpdateNotice {
    pub current: String,
    pub latest: String,
}

// --- State file ---

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    version_check: Option<VersionCheckState>,
}

#[derive(Debug, Serialize, Deserialize)]
struct VersionCheckState {
    last_checked: DateTime<Utc>,
    latest_version: String,
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
        version_check: Some(VersionCheckState {
            last_checked: state.last_checked,
            latest_version: state.latest_version.clone(),
        }),
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

fn fetch_latest_version(current: &str, debug: bool) -> Option<String> {
    let url = format!("{}?v={}", VERSION_CHECK_URL, current);

    if debug {
        eprintln!("[debug] version check: fetching {}", url);
    }

    let config = Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(500)))
        .build();
    let agent: Agent = config.into();

    let resp: VersionResponse = agent
        .get(&url)
        .header("User-Agent", &format!("git-forest/{}", current))
        .call()
        .ok()?
        .body_mut()
        .read_json::<VersionResponse>()
        .ok()?;

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

/// Called after successful commands. Returns Some if an update is available.
/// All errors are swallowed — returns None on any failure.
pub fn check_for_update(debug: bool) -> Option<UpdateNotice> {
    if !is_enabled() {
        if debug {
            eprintln!("[debug] version check: disabled in config");
        }
        return None;
    }

    let current = env!("CARGO_PKG_VERSION");
    let state = read_state();

    match state {
        None => {
            // First run — show notice
            if debug {
                eprintln!("[debug] version check: state file not found, first run");
            }
            eprintln!(
                "Note: git-forest checks for updates daily (current version sent to forest.dliv.gg)."
            );
            eprintln!("Disable: set version_check.enabled = false in config.");

            let latest = fetch_latest_version(current, debug)?;
            write_state(&VersionCheckState {
                last_checked: Utc::now(),
                latest_version: latest.clone(),
            });
            if version_newer(&latest, current) {
                Some(UpdateNotice {
                    current: current.to_string(),
                    latest,
                })
            } else {
                None
            }
        }
        Some(cached) => {
            if is_stale(&cached.last_checked) {
                if debug {
                    eprintln!("[debug] version check: cache stale, fetching");
                }
                let latest = fetch_latest_version(current, debug)?;
                write_state(&VersionCheckState {
                    last_checked: Utc::now(),
                    latest_version: latest.clone(),
                });
                if version_newer(&latest, current) {
                    Some(UpdateNotice {
                        current: current.to_string(),
                        latest,
                    })
                } else {
                    None
                }
            } else {
                if debug {
                    eprintln!(
                        "[debug] version check: cache fresh, latest={}",
                        cached.latest_version
                    );
                }
                if version_newer(&cached.latest_version, current) {
                    Some(UpdateNotice {
                        current: current.to_string(),
                        latest: cached.latest_version,
                    })
                } else {
                    None
                }
            }
        }
    }
}

/// Called by `git forest version --check`. Forces a network check (ignores cache).
/// Returns None on network failure.
pub fn force_check(debug: bool) -> Option<UpdateNotice> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = fetch_latest_version(current, debug)?;

    write_state(&VersionCheckState {
        last_checked: Utc::now(),
        latest_version: latest.clone(),
    });

    if version_newer(&latest, current) {
        Some(UpdateNotice {
            current: current.to_string(),
            latest,
        })
    } else {
        None
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
            latest_version: "0.2.0".to_string(),
        };

        let file = StateFile {
            version_check: Some(VersionCheckState {
                last_checked: state.last_checked,
                latest_version: state.latest_version.clone(),
            }),
        };
        let content = toml::to_string_pretty(&file).unwrap();
        std::fs::write(&state_path, &content).unwrap();

        let read_back: StateFile =
            toml::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        let vc = read_back.version_check.unwrap();
        assert_eq!(vc.latest_version, "0.2.0");
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
