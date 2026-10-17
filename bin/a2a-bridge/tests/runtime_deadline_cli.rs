//! Real, supervised subprocess regressions for runtime daemon hangs. No provider sessions.
#![cfg(unix)]
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::{
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn supervise(mut command: Command, budget: Duration) -> Output {
    command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > budget {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            panic!("bridge exceeded supervised runtime deadline ({budget:?})");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fake_runtime(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("runtime");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn assert_runtime_dead(marker: &Path) {
    let text = std::fs::read_to_string(marker).expect("runtime demonstrably started");
    for line in text.lines() {
        let pid: i32 = line.trim().parse().unwrap();
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "runtime {pid} survived");
        assert_ne!(
            unsafe { libc::kill(-pid, 0) },
            0,
            "runtime group {pid} survived"
        );
    }
}

#[test]
fn containers_list_reports_hung_runtime_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("started");
    let runtime = fake_runtime(
        dir.path(),
        "case \"$*\" in *a2a.managed*) echo $$ >> \"$STARTED\"; exec sleep 120;; *) exit 0;; esac",
    );
    let config = dir.path().join("bridge.toml");
    std::fs::write(&config, format!("default = \"test\"\n[server]\naddr = \"127.0.0.1:0\"\n[verify]\nruntime = {runtime:?}\nimage = \"test\"\ncache = \"test\"\negress = \"open\"\n")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command
        .args(["containers", "list", "--config"])
        .arg(config)
        .env("STARTED", &marker);
    let out = supervise(command, Duration::from_secs(38));
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unavailable"),
        "{out:?}"
    );
    assert_runtime_dead(&marker);
}
