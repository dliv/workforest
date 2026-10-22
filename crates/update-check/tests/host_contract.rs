#![allow(
    clippy::disallowed_methods,
    reason = "test-only second host uses environment and process APIs to launch its real parent/child fixture, plus bounded Instant deadlines for cleanup"
)]

use chrono::{DateTime, Utc};
use git_forest_update_check::{
    check_cache_and_notify, run_background_version_check, RefreshDisposition, Settings,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use ureq::Agent;

const MODE: &str = "UPDATE_OWNER_HOST_MODE";
const ENDPOINT: &str = "UPDATE_OWNER_HOST_ENDPOINT";
const STATE_FILE: &str = "UPDATE_OWNER_HOST_STATE_FILE";
const CONFIG_FILE: &str = "UPDATE_OWNER_HOST_CONFIG_FILE";
const COMPLETE_FILE: &str = "UPDATE_OWNER_HOST_COMPLETE_FILE";
const CHILD_PID_FILE: &str = "UPDATE_OWNER_HOST_CHILD_PID_FILE";

#[derive(Debug, Deserialize, Serialize)]
struct DiskState {
    version_check: Option<DiskVersionCheck>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DiskVersionCheck {
    last_checked: DateTime<Utc>,
    latest_version: Option<String>,
}

struct GatedServer {
    endpoint: String,
    release: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<String>>,
}

struct HostCleanup {
    server: Option<GatedServer>,
    parent: Option<Child>,
    complete_file: PathBuf,
    child_pid_file: PathBuf,
}

impl HostCleanup {
    fn new(complete_file: PathBuf, child_pid_file: PathBuf) -> Self {
        Self {
            server: None,
            parent: None,
            complete_file,
            child_pid_file,
        }
    }

    fn set_server(&mut self, server: GatedServer) {
        self.server = Some(server);
    }

    fn endpoint(&self) -> &str {
        &self
            .server
            .as_ref()
            .expect("fixture server is installed")
            .endpoint
    }

    fn set_parent(&mut self, parent: Child) {
        self.parent = Some(parent);
    }

    fn wait_parent(&mut self) -> Option<ExitStatus> {
        let mut parent = self.parent.take()?;
        wait_bounded(&mut parent)
    }

    fn release_response(&mut self) {
        if let Some(server) = &mut self.server {
            server.release_response();
        }
    }

    fn finish_server(&mut self) -> thread::Result<String> {
        self.server
            .as_mut()
            .expect("fixture server is installed")
            .finish()
    }

    fn wait_for_child(&self, timeout: Duration) -> bool {
        wait_for_file(&self.complete_file, timeout)
    }
}

impl Drop for HostCleanup {
    fn drop(&mut self) {
        self.release_response();
        if let Some(server) = &mut self.server {
            server.join();
        }
        if let Some(mut parent) = self.parent.take() {
            let _ = wait_bounded(&mut parent);
        }
        if let Some(pid) = read_child_pid(&self.child_pid_file) {
            if !wait_for_file(&self.complete_file, Duration::from_secs(3)) {
                terminate_owned_child(pid);
            }
        }
    }
}

impl GatedServer {
    fn release_response(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }

    fn finish(&mut self) -> thread::Result<String> {
        self.release_response();
        self.thread
            .take()
            .expect("server thread should be present until finish")
            .join()
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for GatedServer {
    fn drop(&mut self) {
        self.release_response();
        self.join();
    }
}

fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn settings_from_environment() -> (Settings, PathBuf, PathBuf) {
    let endpoint = std::env::var(ENDPOINT).expect("host helper needs an endpoint");
    let state_file =
        PathBuf::from(std::env::var_os(STATE_FILE).expect("host helper needs a state file"));
    let config_file = std::env::var_os(CONFIG_FILE).map(PathBuf::from);
    let complete_file = PathBuf::from(
        std::env::var_os(COMPLETE_FILE).expect("host helper needs a completion marker"),
    );
    (
        Settings {
            current_version: "0.1.0",
            app_name: "git-forest-test-host",
            command: "git forest test-host",
            channel: "stable",
            endpoint,
            config_file,
            state_file: Some(state_file.clone()),
        },
        state_file,
        complete_file,
    )
}

/// Test-only second host. The outer test invokes this exact ignored test in a real child process.
#[test]
#[ignore = "invoked as a subprocess by the host contract test"]
fn host_process_parent_helper() {
    assert_eq!(std::env::var(MODE).unwrap(), "parent");
    let (settings, state_file, complete_file) = settings_from_environment();
    let mode = std::env::var("UPDATE_OWNER_HOST_CASE").expect("host helper needs a case");
    let expected = match mode.as_str() {
        "refresh" => RefreshDisposition::RefreshRequested,
        "fresh" | "disabled" => RefreshDisposition::NoRefresh,
        other => panic!("unknown host contract case {other}"),
    };
    let disposition = check_cache_and_notify(&settings, false, || at("2026-10-22T20:00:00Z"));
    assert_eq!(disposition, expected);

    if disposition == RefreshDisposition::RefreshRequested {
        let child_pid_file =
            PathBuf::from(std::env::var_os(CHILD_PID_FILE).expect("parent needs child PID path"));
        let executable = std::env::current_exe().expect("test host executable should resolve");
        let mut child = Command::new(executable)
            .args([
                "--exact",
                "host_process_child_helper",
                "--ignored",
                "--nocapture",
            ])
            .env(MODE, "child")
            .env(STATE_FILE, &state_file)
            .env(COMPLETE_FILE, &complete_file)
            .env(
                CHILD_PID_FILE,
                std::env::var_os(CHILD_PID_FILE).expect("parent needs child PID path"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test host should be able to start its refresh child");
        if let Err(error) = std::fs::write(&child_pid_file, child.id().to_string()) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("parent should publish the exact refresh child PID: {error}");
        }
        drop(child);
    }
}

/// Test-only refresh child. It reconstructs settings from the environment and calls the public
/// edge workflow, just as a second production host would do at its own entrypoint.
#[test]
#[ignore = "invoked as a subprocess by the host contract test"]
fn host_process_child_helper() {
    assert_eq!(std::env::var(MODE).unwrap(), "child");
    let (settings, _, complete_file) = settings_from_environment();
    let agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    run_background_version_check(&settings, &agent, || at("2026-10-22T20:01:00Z"));
    std::fs::write(complete_file, "refresh returned").expect("write child completion marker");
}

fn spawn_parent(
    case: &str,
    endpoint: &str,
    state_file: &Path,
    config_file: Option<&Path>,
    complete_file: &Path,
    child_pid_file: &Path,
) -> Child {
    let executable = std::env::current_exe().expect("host contract test executable should resolve");
    let mut command = Command::new(executable);
    command
        .args([
            "--exact",
            "host_process_parent_helper",
            "--ignored",
            "--nocapture",
        ])
        .env(MODE, "parent")
        .env("UPDATE_OWNER_HOST_CASE", case)
        .env(ENDPOINT, endpoint)
        .env(STATE_FILE, state_file)
        .env(COMPLETE_FILE, complete_file)
        .env(CHILD_PID_FILE, child_pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(config_file) = config_file {
        command.env(CONFIG_FILE, config_file);
    } else {
        command.env_remove(CONFIG_FILE);
    }
    command
        .spawn()
        .expect("test should launch second-host parent")
}

fn wait_bounded(child: &mut Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    path.exists()
}

fn read_child_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn terminate_owned_child(pid: u32) {
    if pid <= 1 {
        return;
    }
    let pid = pid.to_string();
    #[cfg(unix)]
    {
        let _ = run_kill(&["-KILL", &pid]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && run_kill(&["-0", &pid]) {
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(windows)]
    {
        if let Ok(mut signal) = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let _ = wait_bounded(&mut signal);
        }
    }
    #[cfg(not(any(unix, windows)))]
    let _ = pid;
}

#[cfg(unix)]
fn run_kill(arguments: &[&str]) -> bool {
    let Ok(mut process) = Command::new("kill")
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    wait_bounded(&mut process).is_some_and(|status| status.success())
}

fn read_state(path: &Path) -> Option<DiskVersionCheck> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| toml::from_str::<DiskState>(&contents).ok())
        .and_then(|state| state.version_check)
}

fn write_state(path: &Path, checked: DateTime<Utc>, latest: Option<&str>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let state = DiskState {
        version_check: Some(DiskVersionCheck {
            last_checked: checked,
            latest_version: latest.map(str::to_owned),
        }),
    };
    std::fs::write(path, toml::to_string_pretty(&state).unwrap()).unwrap();
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(6)))?;
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

fn start_gated_server() -> GatedServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}/api/latest");
    let (release, gate) = mpsc::channel();
    let thread = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "background request did not arrive"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("could not accept update request: {error}"),
            }
        };
        let request = read_request(&mut stream).expect("read background update request");
        gate.recv_timeout(Duration::from_secs(4))
            .expect("outer test did not release server response");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("set bounded server write timeout");
        let body = br#"{"version":"0.2.0"}"#;
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
        request
    });
    GatedServer {
        endpoint,
        release: Some(release),
        thread: Some(thread),
    }
}

fn assert_no_request(listener: &TcpListener) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        match listener.accept() {
            Ok((stream, _)) => panic!("unexpected refresh connection from {stream:?}"),
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
fn second_host_refresh_finishes_only_after_parent_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let state_file = tmp.path().join("state/state.toml");
    let complete_file = tmp.path().join("child.complete");
    let child_pid_file = tmp.path().join("child.pid");
    let mut cleanup = HostCleanup::new(complete_file.clone(), child_pid_file.clone());
    cleanup.set_server(start_gated_server());
    let parent = spawn_parent(
        "refresh",
        cleanup.endpoint(),
        &state_file,
        None,
        &complete_file,
        &child_pid_file,
    );
    cleanup.set_parent(parent);
    let parent_status = cleanup.wait_parent();
    let parent_state = read_state(&state_file);
    let parent_timestamp = parent_state.as_ref().map(|state| state.last_checked);

    cleanup.release_response();
    let request = cleanup
        .finish_server()
        .expect("loopback server thread panicked");
    let child_completed = cleanup.wait_for_child(Duration::from_secs(8));
    let final_state = read_state(&state_file);

    assert!(
        parent_status.is_some_and(|status| status.success()),
        "parent did not exit cleanly"
    );
    assert!(
        child_pid_file.exists(),
        "parent did not publish the child identity"
    );
    let parent_state = parent_state.expect("parent did not seed the cache before exiting");
    assert_eq!(parent_state.latest_version, None);
    assert!(
        child_completed,
        "detached child did not return before the deadline"
    );
    assert!(request.starts_with("GET /api/latest?v=0.1.0&channel=stable HTTP/1.1"));
    let final_state = final_state.expect("child did not write the refreshed cache");
    assert_eq!(final_state.latest_version.as_deref(), Some("0.2.0"));
    assert_eq!(final_state.last_checked, parent_timestamp.unwrap());
}

#[test]
fn second_host_fresh_and_disabled_paths_do_not_launch_a_child() {
    for case in ["fresh", "disabled"] {
        let tmp = tempfile::tempdir().unwrap();
        let state_file = tmp.path().join("state/state.toml");
        let complete_file = tmp.path().join("child.complete");
        let child_pid_file = tmp.path().join("child.pid");
        let mut cleanup = HostCleanup::new(complete_file.clone(), child_pid_file.clone());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/api/latest", listener.local_addr().unwrap());
        let config_file = tmp.path().join("config.toml");
        let config_path = if case == "disabled" {
            std::fs::write(&config_file, "[version_check]\nenabled = false\n").unwrap();
            Some(config_file.as_path())
        } else {
            let future = at("2099-01-01T00:00:00Z");
            write_state(&state_file, future, Some("0.1.0"));
            None
        };
        let before = std::fs::read_to_string(&state_file).ok();

        let parent = spawn_parent(
            case,
            &endpoint,
            &state_file,
            config_path,
            &complete_file,
            &child_pid_file,
        );
        cleanup.set_parent(parent);
        let parent_status = cleanup.wait_parent();
        assert_no_request(&listener);

        assert!(
            parent_status.is_some_and(|status| status.success()),
            "{case} parent failed"
        );
        assert!(
            !complete_file.exists(),
            "{case} path started a refresh child"
        );
        assert!(
            !child_pid_file.exists(),
            "{case} path recorded a refresh child"
        );
        assert_eq!(std::fs::read_to_string(&state_file).ok(), before);
    }
}
