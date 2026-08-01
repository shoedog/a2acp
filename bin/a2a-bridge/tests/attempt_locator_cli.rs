use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

fn locator_lines(child: &mut Child) -> mpsc::Receiver<Result<[String; 2], String>> {
    let stdout = child.stdout.take().expect("child stdout pipe");
    let (sender, receiver) = mpsc::channel();
    let _reader = thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        let first = lines
            .next()
            .transpose()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "stdout closed before execution locator".to_owned())?;
        let second = lines
            .next()
            .transpose()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "stdout closed before attempt locator".to_owned())?;
        let _ = sender.send(Ok([first, second]));
        Ok::<(), String>(())
    });
    receiver
}

fn assert_locator(lines: &[String; 2]) {
    assert!(
        lines[0].starts_with("execution_id=exec-"),
        "unexpected execution locator: {:?}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("attempt_id=attempt-"),
        "unexpected attempt locator: {:?}",
        lines[1]
    );
}

#[test]
fn run_workflow_flushes_locator_to_a_pipe_before_blocking_on_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["run-workflow", "code-review", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn run-workflow");
    let stdin = child.stdin.take().expect("child stdin pipe");
    let locator = locator_lines(&mut child)
        .recv_timeout(WAIT)
        .expect("locator must be readable while stdin remains blocked")
        .expect("read locator");
    assert_locator(&locator);
    assert!(
        child.try_wait().unwrap().is_none(),
        "run-workflow should still be blocked waiting for local input"
    );

    child.kill().expect("kill blocked run-workflow");
    child.wait().expect("reap blocked run-workflow");
    drop(stdin);
}

#[test]
fn submit_flushes_locator_to_a_pipe_before_a_blocked_network_response() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind blocking endpoint");
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let (accepted_sender, accepted_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + WAIT;
        loop {
            match listener.accept() {
                Ok((_stream, _)) => {
                    accepted_sender.send(()).unwrap();
                    let _ = release_receiver.recv_timeout(WAIT);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "submit never reached the endpoint"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept submit request: {error}"),
            }
        }
    });

    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.txt");
    std::fs::write(&input, "hello").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args([
            "submit",
            "--input",
            input.to_str().unwrap(),
            "--url",
            &format!("http://{address}"),
        ])
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env("NO_PROXY", "*")
        .env("no_proxy", "*")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn submit");
    let locator = locator_lines(&mut child)
        .recv_timeout(WAIT)
        .expect("locator must be readable before the network response")
        .expect("read locator");
    assert_locator(&locator);
    accepted_receiver
        .recv_timeout(WAIT)
        .expect("submit reaches blocking endpoint");
    assert!(
        child.try_wait().unwrap().is_none(),
        "submit should still be blocked waiting for the response"
    );

    child.kill().expect("kill blocked submit");
    child.wait().expect("reap blocked submit");
    release_sender.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn submit_prints_locator_and_deepest_cause_then_exits_nonzero() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind error endpoint");
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept submit request");
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = stream.read(&mut chunk).expect("read submit request");
            assert!(read > 0, "submit request closed before its body");
            request.extend_from_slice(&chunk[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .expect("submit request content length");
            if request.len() >= header_end + content_length {
                break;
            }
        }
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32603,
                "message": "internal",
                "data": {
                    "code": "provider.limit",
                    "deepest_cause": "sanitized provider cause"
                }
            }
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        stream.flush().unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.txt");
    std::fs::write(&input, "hello").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args([
            "submit",
            "--input",
            input.to_str().unwrap(),
            "--url",
            &format!("http://{address}"),
        ])
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env("NO_PROXY", "*")
        .env("no_proxy", "*")
        .output()
        .expect("run submit error case");
    server.join().unwrap();

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines = stdout.lines().map(str::to_owned).collect::<Vec<_>>();
    assert!(lines.len() >= 2, "missing locator: {lines:?}");
    assert_locator(&[lines[0].clone(), lines[1].clone()]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("submit failed: sanitized provider cause"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("provider.limit"),
        "deepest cause must win: {stderr}"
    );
}

#[test]
fn local_input_failures_still_leave_a_complete_locator_on_stdout() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-input");
    let missing = missing.to_str().unwrap();
    for args in [
        vec!["run-workflow", "code-review", "--input", missing],
        vec!["submit", "--input", missing],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(&args)
            .output()
            .expect("run early-input-failure case");
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        let lines = String::from_utf8(output.stdout).unwrap();
        let lines = lines.lines().map(str::to_owned).collect::<Vec<_>>();
        assert!(lines.len() >= 2, "missing locator for {args:?}: {lines:?}");
        assert_locator(&[lines[0].clone(), lines[1].clone()]);
    }
}
