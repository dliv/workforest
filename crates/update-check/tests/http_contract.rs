#![allow(
    clippy::disallowed_methods,
    reason = "test-only HTTP fixture uses bounded Instant deadlines to prove request ordering and cleanup"
)]

use chrono::{DateTime, Utc};
use git_forest_update_check::{
    check_cache_and_notify, force_check, run_background_version_check, ForceCheckResult,
    RefreshDisposition, Settings,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use ureq::Agent;

#[derive(Debug, Deserialize, Serialize)]
struct DiskState {
    version_check: Option<DiskVersionCheck>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DiskVersionCheck {
    last_checked: DateTime<Utc>,
    latest_version: Option<String>,
}

struct ResponseReceipt {
    request: String,
    response_written: bool,
}

struct OneShotServer {
    endpoint: String,
    release: Option<mpsc::Sender<()>>,
    response_released: Arc<AtomicBool>,
    thread: Option<JoinHandle<ResponseReceipt>>,
}

impl OneShotServer {
    fn new(body: Vec<u8>, status: &str, wait_for_release: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = format!("http://{address}/api/latest");
        let response_released = Arc::new(AtomicBool::new(false));
        let server_response_released = Arc::clone(&response_released);
        let (release, gate) = if wait_for_release {
            let (sender, receiver) = mpsc::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let status = status.to_owned();
        let thread = thread::spawn(move || {
            let mut stream = accept_bounded(&listener).expect("update request did not arrive");
            let request = read_request(&mut stream).expect("could not read update request");
            if let Some(gate) = gate {
                gate.recv_timeout(Duration::from_secs(3))
                    .expect("test did not release the response gate");
            }
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .expect("set bounded server write timeout");
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            // The client cannot complete its body read until these bytes are written. Set this
            // causal marker before the first byte so a fast client cannot outrun the signal.
            server_response_released.store(true, Ordering::SeqCst);
            let response_written = stream
                .write_all(headers.as_bytes())
                .and_then(|()| stream.write_all(&body))
                .and_then(|()| stream.flush())
                .is_ok();
            ResponseReceipt {
                request,
                response_written,
            }
        });
        Self {
            endpoint,
            release,
            response_released,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> ResponseReceipt {
        self.release_response();
        self.thread
            .take()
            .expect("server thread should be present until finish")
            .join()
            .expect("update server thread panicked")
    }

    fn release_response(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

impl Drop for OneShotServer {
    fn drop(&mut self) {
        self.release_response();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn accept_bounded(listener: &TcpListener) -> std::io::Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out waiting for update request",
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 16 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers exceeded 16 KiB",
            ));
        }
    }
    Ok(String::from_utf8_lossy(&request).into_owned())
}

fn agent(timeout: Duration) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into()
}

fn make_settings(
    endpoint: String,
    config_file: Option<PathBuf>,
    state_file: Option<PathBuf>,
) -> Settings {
    Settings {
        current_version: "0.1.0",
        app_name: "git-forest",
        command: "git forest",
        channel: "stable",
        endpoint,
        config_file,
        state_file,
    }
}

fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn write_state(path: &Path, last_checked: DateTime<Utc>, latest: Option<&str>) {
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    let state = DiskState {
        version_check: Some(DiskVersionCheck {
            last_checked,
            latest_version: latest.map(str::to_owned),
        }),
    };
    std::fs::write(path, toml::to_string_pretty(&state).unwrap()).unwrap();
}

fn read_state(path: &Path) -> DiskVersionCheck {
    toml::from_str::<DiskState>(&std::fs::read_to_string(path).unwrap())
        .unwrap()
        .version_check
        .unwrap()
}

fn assert_request(request: &str) {
    let mut lines = request.lines();
    assert_eq!(
        lines.next(),
        Some("GET /api/latest?v=0.1.0&channel=stable HTTP/1.1")
    );
    assert!(
        lines.any(|line| line.eq_ignore_ascii_case("User-Agent: git-forest/0.1.0")),
        "request lacked the expected user agent: {request}"
    );
}

fn assert_no_connection(listener: &TcpListener) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        match listener.accept() {
            Ok((stream, _)) => panic!("unexpected update request from {stream:?}"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("could not inspect update listener: {error}"),
        }
    }
}

#[test]
fn force_check_uses_host_agent_endpoint_and_samples_time_after_response() {
    let tmp = tempfile::tempdir().unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let server = OneShotServer::new(br#"{"version":"0.2.0"}"#.to_vec(), "200 OK", false);
    let response_released = Arc::clone(&server.response_released);
    let now = at("2026-10-22T20:00:00Z");
    let settings = make_settings(server.endpoint.clone(), None, Some(state_file.clone()));

    let result = force_check(&settings, &agent(Duration::from_secs(1)), false, || {
        assert!(
            response_released.load(Ordering::SeqCst),
            "force check sampled the clock before the server released response bytes"
        );
        now
    });
    let receipt = server.finish();

    assert!(receipt.response_written);
    assert_request(&receipt.request);
    assert!(matches!(result, ForceCheckResult::UpdateAvailable(_)));
    let state = read_state(&state_file);
    assert_eq!(state.last_checked, now);
    assert_eq!(state.latest_version.as_deref(), Some("0.2.0"));
}

#[test]
fn background_refresh_updates_version_and_preserves_readable_timestamp() {
    let tmp = tempfile::tempdir().unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let old = at("2026-10-20T10:00:00Z");
    write_state(&state_file, old, Some("0.0.9"));
    let server = OneShotServer::new(br#"{"version":"0.1.2"}"#.to_vec(), "200 OK", false);
    let settings = make_settings(server.endpoint.clone(), None, Some(state_file.clone()));
    run_background_version_check(&settings, &agent(Duration::from_secs(1)), || {
        at("2026-10-22T20:00:00Z")
    });
    let receipt = server.finish();

    assert!(receipt.response_written);
    assert_request(&receipt.request);
    let state = read_state(&state_file);
    assert_eq!(state.last_checked, old);
    assert_eq!(state.latest_version.as_deref(), Some("0.1.2"));
}

#[test]
fn background_refresh_supplies_default_timestamp_when_state_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let server = OneShotServer::new(br#"{"version":"0.1.2"}"#.to_vec(), "200 OK", false);
    let settings = make_settings(server.endpoint.clone(), None, Some(state_file.clone()));
    let now = at("2026-10-22T20:00:00Z");

    run_background_version_check(&settings, &agent(Duration::from_secs(1)), || now);
    let receipt = server.finish();

    assert!(receipt.response_written);
    assert_request(&receipt.request);
    let state = read_state(&state_file);
    assert_eq!(state.last_checked, now);
    assert_eq!(state.latest_version.as_deref(), Some("0.1.2"));
}

#[test]
fn disabled_and_fresh_cached_reads_make_no_request() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/api/latest", listener.local_addr().unwrap());
    let config_file = tmp.path().join("config.toml");
    std::fs::write(&config_file, "[version_check]\nenabled = false\n").unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let old = at("2026-10-20T10:00:00Z");
    write_state(&state_file, old, Some("0.1.0"));
    let before = std::fs::read_to_string(&state_file).unwrap();
    let settings = make_settings(endpoint, Some(config_file), Some(state_file.clone()));

    assert!(matches!(
        force_check(&settings, &agent(Duration::from_millis(200)), false, || {
            at("2026-10-22T20:00:00Z")
        }),
        ForceCheckResult::Disabled
    ));
    assert_eq!(
        check_cache_and_notify(&settings, false, || at("2026-10-22T20:00:00Z")),
        RefreshDisposition::NoRefresh
    );
    assert_eq!(std::fs::read_to_string(state_file).unwrap(), before);

    let fresh_file = tmp.path().join("fresh/state.toml");
    write_state(&fresh_file, at("2099-01-01T00:00:00Z"), Some("0.1.0"));
    let fresh_before = std::fs::read_to_string(&fresh_file).unwrap();
    let fresh_settings = make_settings(
        format!("http://{}/api/latest", listener.local_addr().unwrap()),
        None,
        Some(fresh_file.clone()),
    );
    assert_eq!(
        check_cache_and_notify(&fresh_settings, false, || at("2026-10-22T20:00:00Z")),
        RefreshDisposition::NoRefresh
    );
    assert_eq!(std::fs::read_to_string(fresh_file).unwrap(), fresh_before);
    assert_no_connection(&listener);
}

#[test]
fn malformed_json_and_body_do_not_change_the_cache() {
    for body in [b"{not-json}".to_vec(), vec![0xff, 0xfe, 0xfd]] {
        let tmp = tempfile::tempdir().unwrap();
        let state_file = tmp.path().join("state/state.toml");
        let last_checked = at("2026-10-20T10:00:00Z");
        write_state(&state_file, last_checked, Some("0.0.9"));
        let before = std::fs::read_to_string(&state_file).unwrap();
        let server = OneShotServer::new(body, "200 OK", false);
        let settings = make_settings(server.endpoint.clone(), None, Some(state_file.clone()));

        assert!(matches!(
            force_check(&settings, &agent(Duration::from_secs(1)), false, || {
                at("2026-10-22T20:00:00Z")
            }),
            ForceCheckResult::FetchFailed
        ));
        let receipt = server.finish();

        assert_request(&receipt.request);
        assert_eq!(std::fs::read_to_string(state_file).unwrap(), before);
    }
}

#[test]
fn connection_failure_does_not_change_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let last_checked = at("2026-10-20T10:00:00Z");
    write_state(&state_file, last_checked, Some("0.0.9"));
    let before = std::fs::read_to_string(&state_file).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/api/latest", listener.local_addr().unwrap());
    drop(listener);
    let settings = make_settings(endpoint, None, Some(state_file.clone()));

    assert!(matches!(
        force_check(&settings, &agent(Duration::from_millis(250)), false, || {
            at("2026-10-22T20:00:00Z")
        }),
        ForceCheckResult::FetchFailed
    ));
    assert_eq!(std::fs::read_to_string(state_file).unwrap(), before);
}

#[test]
fn client_timeout_does_not_change_the_cache_and_releases_server() {
    let tmp = tempfile::tempdir().unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let last_checked = at("2026-10-20T10:00:00Z");
    write_state(&state_file, last_checked, Some("0.0.9"));
    let before = std::fs::read_to_string(&state_file).unwrap();
    let server = OneShotServer::new(br#"{"version":"0.2.0"}"#.to_vec(), "200 OK", true);
    let settings = make_settings(server.endpoint.clone(), None, Some(state_file.clone()));

    assert!(matches!(
        force_check(&settings, &agent(Duration::from_millis(100)), false, || {
            at("2026-10-22T20:00:00Z")
        }),
        ForceCheckResult::FetchFailed
    ));
    let receipt = server.finish();

    assert_request(&receipt.request);
    assert_eq!(std::fs::read_to_string(state_file).unwrap(), before);
}
