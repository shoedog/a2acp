use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn write_config(path: &Path, body: &str) {
    fs::write(path, body).expect("write models test config");
}

fn bridge_models(config: &Path, extra: &[&str]) -> std::process::Output {
    bridge_models_with_env(config, extra, &[])
}

fn bridge_models_with_env(
    config: &Path,
    extra: &[&str],
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command
        .arg("models")
        .arg("--config")
        .arg(config)
        .args(extra)
        .envs(env.iter().copied());
    command.output().expect("run a2a-bridge models")
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake executable");
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn serve_one_response(
    status: &'static str,
    body: &'static [u8],
) -> (String, thread::JoinHandle<()>) {
    serve_one_response_observed(status, body, None)
}

fn serve_one_response_observed(
    status: &'static str,
    body: &'static [u8],
    mut first_read: Option<mpsc::Sender<()>>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake model endpoint");
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "model request was never received"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept model request: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0_u8; 16 * 1024];
        let mut request_len = 0;
        loop {
            let read = stream
                .read(&mut request[request_len..])
                .expect("read fake model request");
            assert!(read > 0, "model request closed before headers completed");
            request_len += read;
            if let Some(first_read) = first_read.take() {
                first_read.send(()).unwrap();
            }
            if request[..request_len]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
            assert!(
                request_len < request.len(),
                "model request headers exceeded fixture limit"
            );
        }
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{address}/v1"), handle)
}

#[test]
fn fake_model_endpoint_waits_for_complete_request_headers() {
    let (first_read_tx, first_read_rx) = mpsc::channel();
    let (base_url, server) =
        serve_one_response_observed("200 OK", br#"{"data":[]}"#, Some(first_read_tx));
    let address = base_url
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix("/v1"))
        .unwrap();
    let mut client = std::net::TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    client
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    client.flush().unwrap();
    first_read_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let mut first_response_byte = [0_u8; 1];
    let early_read = client.read(&mut first_response_byte);
    assert!(
        matches!(
            early_read,
            Err(ref error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
        ),
        "endpoint responded before the request header terminator: {early_read:?}"
    );

    client.write_all(b"Connection: close\r\n\r\n").unwrap();
    client.flush().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).unwrap();
    server.join().unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
}

#[test]
fn configured_agent_probe_failure_is_structured_and_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-acp-adapter");
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"broken\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = [{missing:?}]\n\n\
             [[agents]]\nid = \"broken\"\ncmd = {missing:?}\nversion = \"9.9.9\"\n",
        ),
    );

    let output = bridge_models(&config, &["--agent", "broken", "--json"]);
    assert!(
        !output.status.success(),
        "a configured agent whose probe failed must not exit successfully: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("failure stdout remains machine-readable JSON");
    assert_eq!(value["broken"]["available"], false);
    let failure = &value["broken"]["failure"];
    assert_eq!(failure["agent"], "broken");
    assert_eq!(failure["strategy"], "acp");
    assert_eq!(failure["phase"], "spawn");
    assert_eq!(failure["category"], "spawn");
    assert_eq!(failure["executable"], missing.to_string_lossy().as_ref());
    assert_eq!(failure["configured_version"], "9.9.9");
    assert!(
        failure["error"]
            .as_str()
            .is_some_and(|error| error.contains("spawn") && !error.is_empty()),
        "deepest bounded error was not retained: {failure}",
    );
    assert_eq!(failure["diagnostic"]["failed_phase"], "spawn");
    let deepest = failure["diagnostic"]["causes"]
        .as_array()
        .and_then(|causes| causes.last())
        .and_then(serde_json::Value::as_str)
        .expect("structured ACP failure retains its deepest safe cause");
    assert!(!deepest.is_empty());
    assert!(failure["error"].as_str().unwrap().contains(deepest));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("models: probe failed for agent")
            && String::from_utf8_lossy(&output.stderr).contains("'broken'")
            && String::from_utf8_lossy(&output.stderr).contains("during spawn"),
        "stderr must explain the nonzero exit: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn all_agent_json_keeps_healthy_caps_and_represents_probe_failure() {
    let dir = tempfile::tempdir().unwrap();
    let healthy = dir.path().join("kiro-cli");
    executable(
        &healthy,
        "#!/bin/sh\nprintf '%s\\n' '* auto 1.00x credits Models chosen by task'\n",
    );

    let missing = dir.path().join("missing-acp-adapter");
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"healthy\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = [{healthy:?}, {missing:?}]\n\n\
             [[agents]]\nid = \"healthy\"\ncmd = {healthy:?}\n\n\
             [[agents]]\nid = \"broken\"\ncmd = {missing:?}\n",
        ),
    );

    let output = bridge_models(&config, &["--json"]);
    assert!(
        output.status.success(),
        "all-agent discovery should preserve partial-success semantics: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["healthy"]["models"], serde_json::json!(["auto"]));
    assert_eq!(value["broken"]["available"], false);
    assert_eq!(value["broken"]["failure"]["agent"], "broken");
    assert_eq!(value.as_object().unwrap().len(), 2);
}

#[test]
fn process_exit_keeps_deepest_error_in_text_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let failing = dir.path().join("kiro-cli");
    executable(
        &failing,
        "#!/bin/sh\nprintf '%s\\n' 'earlier harmless context' >&2\nprintf '%s\\n' 'deepest-probe-sentinel-30' >&2\nexit 23\n",
    );
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"broken\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = [{failing:?}]\n\n\
             [[agents]]\nid = \"broken\"\ncmd = {failing:?}\n",
        ),
    );

    let text = bridge_models(&config, &["--agent", "broken"]);
    assert!(!text.status.success());
    let text_output = format!(
        "{}{}",
        String::from_utf8_lossy(&text.stdout),
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(text_output.contains("deepest-probe-sentinel-30"));
    assert!(text_output.contains("process_exit"));

    let json = bridge_models(&config, &["--agent", "broken", "--json"]);
    assert!(!json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let failure = &value["broken"]["failure"];
    assert_eq!(failure["phase"], "discovery");
    assert_eq!(failure["category"], "process_exit");
    assert!(failure["error"]
        .as_str()
        .is_some_and(|error| error.contains("deepest-probe-sentinel-30")));
}

#[test]
fn credential_shaped_process_error_is_redacted_and_bounded_in_text_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let failing = dir.path().join("kiro-cli");
    let noise = "x".repeat(4096);
    executable(
        &failing,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"fatal credential $A2A_BRIDGE_MODELS_TEST_SECRET {noise}\" >&2\nexit 19\n"
        ),
    );
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"broken\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = [{failing:?}]\n\n\
             [[agents]]\nid = \"broken\"\ncmd = {failing:?}\n",
        ),
    );
    let secret = "sk-catalog-env-secret-30-483920";

    let text = bridge_models_with_env(
        &config,
        &["--agent", "broken"],
        &[("A2A_BRIDGE_MODELS_TEST_SECRET", secret)],
    );
    let text_output = format!(
        "{}{}",
        String::from_utf8_lossy(&text.stdout),
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(!text_output.contains(secret));
    assert!(!text_output.contains(&"x".repeat(600)));
    assert!(text_output.contains("[REDACTED]"));

    let json = bridge_models_with_env(
        &config,
        &["--agent", "broken", "--json"],
        &[("A2A_BRIDGE_MODELS_TEST_SECRET", secret)],
    );
    let json_text = format!(
        "{}{}",
        String::from_utf8_lossy(&json.stdout),
        String::from_utf8_lossy(&json.stderr)
    );
    assert!(!json_text.contains(secret));
    assert!(!json_text.contains(&"x".repeat(600)));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let error = value["broken"]["failure"]["error"].as_str().unwrap();
    assert!(error.len() <= 512);
    assert!(error.contains("[REDACTED]"));
}

#[test]
fn malformed_api_response_has_response_parse_category() {
    let dir = tempfile::tempdir().unwrap();
    let (base_url, server) = serve_one_response("200 OK", b"not-json");
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"api\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = []\n\n\
             [[agents]]\nid = \"api\"\nkind = \"api\"\nbase_url = {base_url:?}\napi_key_env = \"A2A_BRIDGE_MODELS_TEST_API_KEY\"\nmodel = \"fake-model\"\n",
        ),
    );

    let output = bridge_models_with_env(
        &config,
        &["--agent", "api", "--json"],
        &[("A2A_BRIDGE_MODELS_TEST_API_KEY", "test-api-key-value")],
    );
    server.join().unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let failure = &value["api"]["failure"];
    assert_eq!(failure["strategy"], "api");
    assert_eq!(failure["phase"], "discovery");
    assert_eq!(failure["category"], "response_parse");
}

#[test]
fn non_success_api_status_is_provider_error_and_body_is_not_exposed() {
    let dir = tempfile::tempdir().unwrap();
    let (base_url, server) = serve_one_response(
        "401 Unauthorized",
        br#"{"data":[{"id":"body-must-not-be-treated-as-models-or-echoed"}]}"#,
    );
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"api\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = []\n\n\
             [[agents]]\nid = \"api\"\nkind = \"api\"\nbase_url = {base_url:?}\napi_key_env = \"A2A_BRIDGE_MODELS_TEST_API_KEY\"\nmodel = \"fake-model\"\n",
        ),
    );

    let output = bridge_models_with_env(
        &config,
        &["--agent", "api", "--json"],
        &[("A2A_BRIDGE_MODELS_TEST_API_KEY", "test-api-key-value")],
    );
    server.join().unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let failure = &value["api"]["failure"];
    assert_eq!(failure["category"], "provider_acp");
    assert!(failure["error"]
        .as_str()
        .is_some_and(|error| error.contains("HTTP 401")));
    assert!(!value
        .to_string()
        .contains("body-must-not-be-treated-as-models-or-echoed"));
}
