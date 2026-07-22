use std::io::{BufRead, BufReader};
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
