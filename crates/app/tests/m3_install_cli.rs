use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static TEST_ID: AtomicU64 = AtomicU64::new(0);
static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

struct TestDir(PathBuf, #[allow(dead_code)] MutexGuard<'static, ()>);

impl TestDir {
    fn new(name: &str) -> Self {
        // These cases launch and copy the same debug executable repeatedly.
        // Serializing this test binary avoids macOS executable-inspection and
        // disk-pressure delays being mistaken for product timeout failures.
        // Runtime multi-process concurrency is covered in its own integration
        // suite; it is not the subject of these installer/doctor cases.
        let guard = PROCESS_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/tmp").join(format!(
            "flow-agent-install-cli-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path, guard)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_json(path: &Path, value: &Value) {
    write(path, &serde_json::to_vec_pretty(value).unwrap(), 0o600);
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn command(root: &TestDir) -> Command {
    let home = root.0.join("home");
    let codex_home = root.0.join("alternate-codex-home");
    let flow_home = root.0.join("flow-home");
    let fake_bin = root.0.join("fake-bin");
    let mut command = Command::new(env!("CARGO_BIN_EXE_flow-agent"));
    command
        .env("HOME", home)
        .env("CODEX_HOME", codex_home)
        .env("FLOW_AGENT_HOME", flow_home)
        .env("PATH", fake_bin);
    command
}

fn install_fake_provider(root: &TestDir, provider: &str) {
    let fake_bin = root.0.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    // A freshly-created shell script can be delayed by macOS executable
    // inspection when these process-heavy integration tests run in parallel.
    // /bin/echo is a stable native stand-in that still proves PATH discovery,
    // --version execution, output capture, and the two-second kill boundary.
    symlink("/bin/echo", fake_bin.join(provider)).unwrap();
}

struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Value,
}

fn http(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> HttpResponse {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let marker = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = String::from_utf8_lossy(&response[..marker]);
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let bytes = &response[marker + 4..];
    HttpResponse {
        status,
        headers,
        body: if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(bytes).unwrap()
        },
    }
}

#[test]
fn cli_all_installs_and_uninstalls_without_losing_user_hooks() {
    let root = TestDir::new("round-trip");
    install_fake_provider(&root, "claude");
    install_fake_provider(&root, "codex");
    let claude_path = root.0.join("home/.claude/settings.json");
    let codex_path = root.0.join("alternate-codex-home/hooks.json");
    let codex_config = root.0.join("alternate-codex-home/config.toml");
    let claude_original = json!({
        "user": "claude",
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "user-claude"}]}]}
    });
    let codex_original = json!({
        "user": "codex",
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "user-codex"}]}]}
    });
    write_json(&claude_path, &claude_original);
    write_json(&codex_path, &codex_original);
    write(
        &codex_config,
        b"[features]\nhooks = true\n\n[hooks.state]\n",
        0o600,
    );

    let installed = command(&root)
        .args(["install-hooks", "all"])
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let stdout = String::from_utf8_lossy(&installed.stdout);
    assert!(stdout.contains("installed claude hooks"));
    assert!(stdout.contains("installed codex hooks"));
    assert!(stdout.contains("run /hooks"));
    assert_eq!(read_json(&claude_path)["user"], "claude");
    assert_eq!(read_json(&codex_path)["user"], "codex");
    assert!(root.0.join("flow-home/bin/flow-agent").exists());

    let uninstalled = command(&root)
        .args(["uninstall-hooks", "all"])
        .output()
        .unwrap();
    assert!(
        uninstalled.status.success(),
        "{}",
        String::from_utf8_lossy(&uninstalled.stderr)
    );
    assert_eq!(read_json(&claude_path), claude_original);
    assert_eq!(read_json(&codex_path), codex_original);
    assert!(!root.0.join("flow-home/bin/flow-agent").exists());
    assert_eq!(
        fs::read_to_string(codex_config).unwrap(),
        "[features]\nhooks = true\n\n[hooks.state]\n"
    );
}

#[test]
fn cli_refuses_to_create_configuration_for_a_missing_provider() {
    let root = TestDir::new("missing-provider");
    let output = command(&root)
        .args(["install-hooks", "claude"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("claude CLI is not installed"));
    assert!(!root.0.join("home/.claude/settings.json").exists());
    assert!(!root.0.join("flow-home/bin/flow-agent").exists());
}

#[test]
fn doctor_json_reports_versions_config_trust_runtime_and_silent_pass_through() {
    let root = TestDir::new("doctor-json");
    install_fake_provider(&root, "claude");
    install_fake_provider(&root, "codex");
    let installed = command(&root)
        .args(["install-hooks", "all"])
        .output()
        .unwrap();
    assert!(installed.status.success());

    let output = command(&root).args(["doctor", "--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["checks"].as_array().unwrap();
    let by_id = |id: &str| {
        checks
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("missing doctor check {id}"))
    };
    assert_eq!(
        by_id("claude.cli")["status"],
        "pass",
        "{}",
        serde_json::to_string_pretty(by_id("claude.cli")).unwrap()
    );
    assert!(by_id("claude.cli")["detail"]
        .as_str()
        .unwrap()
        .contains("fake-bin/claude"));
    assert_eq!(by_id("codex.config")["status"], "pass");
    assert_eq!(by_id("codex.trust")["status"], "warning");
    assert_eq!(by_id("runtime.control_loop")["status"], "fail");
    assert_eq!(
        by_id("hook.pass_through")["status"],
        "pass",
        "{}",
        serde_json::to_string_pretty(by_id("hook.pass_through")).unwrap()
    );
    assert_eq!(
        by_id("hook.pass_through")["detail"],
        "No approval directive was written to stdout"
    );
}

#[test]
fn overlong_socket_path_blocks_install_before_provider_configuration_is_touched() {
    let root = TestDir::new("overlong-socket");
    install_fake_provider(&root, "claude");
    let long_flow_home = root.0.join("x".repeat(140));
    let output = command(&root)
        .env("FLOW_AGENT_HOME", &long_flow_home)
        .args(["install-hooks", "claude"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unix socket path"));
    assert!(!root.0.join("home/.claude/settings.json").exists());

    let doctor = command(&root)
        .env("FLOW_AGENT_HOME", &long_flow_home)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(doctor.status.success());
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let socket = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "socket.path")
        .unwrap();
    assert_eq!(socket["status"], "fail");
    assert_eq!(socket["repairability"], "manual");
}

#[test]
fn doctor_runtime_probe_round_trips_without_creating_a_provider_event() {
    let root = TestDir::new("doctor-runtime");
    install_fake_provider(&root, "claude");
    install_fake_provider(&root, "codex");
    let socket = root.0.join("flow-home/run/bridge.sock");
    let mut runtime = command(&root)
        .args(["serve", "--approval", "widget"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !socket.exists() {
        let _ = runtime.kill();
        let output = runtime.wait_with_output().unwrap();
        panic!(
            "runtime did not create its socket\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = command(&root).args(["doctor", "--json"]).output().unwrap();
    let _ = runtime.kill();
    let _ = runtime.wait();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let control = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "runtime.control_loop")
        .unwrap();
    assert_eq!(control["status"], "pass");
    assert!(control["detail"]
        .as_str()
        .unwrap()
        .contains("without creating an Agent session"));
}

#[test]
fn onboarding_api_uses_the_installer_and_requires_a_post_install_real_event() {
    let root = TestDir::new("onboarding-api");
    install_fake_provider(&root, "claude");
    install_fake_provider(&root, "codex");
    let claude_config = root.0.join("home/.claude/settings.json");
    let original = json!({"keepUserSetting": true});
    write_json(&claude_config, &original);

    let mut runtime = command(&root)
        .args(["serve", "--approval", "widget"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(runtime.stdout.take().unwrap());
    let mut control_line = String::new();
    stdout.read_line(&mut control_line).unwrap();
    assert!(control_line.starts_with("Flow Agent control panel: http://"));
    let url = control_line
        .trim()
        .strip_prefix("Flow Agent control panel: ")
        .unwrap();
    let (origin, bootstrap_token) = url.split_once("/#bootstrap=").unwrap();
    let address: SocketAddr = origin.strip_prefix("http://").unwrap().parse().unwrap();
    runtime.stdout = Some(stdout.into_inner());

    let bootstrap = http(
        address,
        "POST",
        "/api/v1/bootstrap",
        &[("Origin", origin), ("Content-Type", "application/json")],
        Some(&json!({"token": bootstrap_token})),
    );
    assert_eq!(bootstrap.status, 200);
    let cookie = bootstrap.headers["set-cookie"]
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = bootstrap.body["csrfToken"].as_str().unwrap().to_owned();

    let initial = http(
        address,
        "GET",
        "/api/v1/setup",
        &[("Cookie", &cookie)],
        None,
    );
    assert_eq!(initial.status, 200);
    assert!(initial.body["firstRun"].as_bool().unwrap());

    let installed = http(
        address,
        "POST",
        "/api/v1/setup",
        &[
            ("Origin", origin),
            ("Cookie", &cookie),
            ("x-flow-agent-csrf", &csrf),
            ("Content-Type", "application/json"),
        ],
        Some(&json!({"provider": "claude", "action": "install"})),
    );
    assert_eq!(installed.status, 200, "{}", installed.body);
    let claude = installed.body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["provider"] == "claude")
        .unwrap();
    assert_eq!(claude["status"], "installed_unverified");
    assert_eq!(claude["realEventVerified"], false);

    let stable = root.0.join("flow-home/bin/flow-agent");
    let mut hook = Command::new(&stable)
        .args(["hook", "--provider", "claude"])
        .env("FLOW_AGENT_HOME", root.0.join("flow-home"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"hook_event_name":"SessionStart","session_id":"onboarding-real-session","cwd":"/tmp/example-project","source":"startup"}"#,
        )
        .unwrap();
    let hook_output = hook.wait_with_output().unwrap();
    assert!(hook_output.status.success());
    assert!(hook_output.stdout.is_empty());

    let mut verified = None;
    for _ in 0..100 {
        let current = http(
            address,
            "GET",
            "/api/v1/setup",
            &[("Cookie", &cookie)],
            None,
        );
        let claude = current.body["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["provider"] == "claude")
            .unwrap()
            .clone();
        if claude["status"] == "connected" {
            verified = Some(claude);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let verified = verified.expect("real provider event did not verify onboarding");
    assert_eq!(verified["realEventVerified"], true);

    let doctor = command(&root).args(["doctor", "--json"]).output().unwrap();
    assert!(doctor.status.success());
    let doctor: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let real_event = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "claude.real_event")
        .unwrap();
    assert_eq!(real_event["status"], "pass");

    let uninstalled = http(
        address,
        "POST",
        "/api/v1/setup",
        &[
            ("Origin", origin),
            ("Cookie", &cookie),
            ("x-flow-agent-csrf", &csrf),
            ("Content-Type", "application/json"),
        ],
        Some(&json!({"provider": "claude", "action": "uninstall"})),
    );
    assert_eq!(uninstalled.status, 200);
    assert_eq!(read_json(&claude_config), original);
    let _ = runtime.kill();
    let _ = runtime.wait();
}
