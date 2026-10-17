//! CLI-surface tests for the `storage reap` authorities (R2f1b custody plan §3 S3/S4).
//!
//! The S3 dual review carried "the `storage_cmd`/`storage_runtime_pass` CLI orchestration lacks
//! behavioral tests" to the ledger. These cover the part of that orchestration a unit test cannot
//! reach: argument routing on a DESTRUCTIVE command, where the failure modes are an operator reading
//! the wrong gate documentation, or a class flag being inferred rather than demanded.

use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(args)
        .output()
        .expect("spawn a2a-bridge")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Discriminates a `--help` check that fires before the subcommand is dispatched: it would print the
/// umbrella `storage` page for every reap invocation, so the gate documentation for the two
/// DESTRUCTIVE authorities — the pages an operator reads BEFORE authorizing a deletion — is
/// unreachable from the command line.
#[test]
fn each_reap_authority_prints_its_own_gate_documentation() {
    let clones = stdout(&run(&["storage", "reap", "--clones", "--help"]));
    assert!(
        clones.starts_with("usage: a2a-bridge storage reap --clones"),
        "`reap --clones --help` did not print the clone reaper's page:\n{clones}"
    );
    for expected in [
        "content on main",
        "yes(head)",
        "yes(tree)",
        "git status --porcelain",
        "clone_reap_lookback",
        "-fold.json",
        // S4b/F1: the chained-origin resolution an operator must know about before reading a
        // verdict — it changes WHICH repository the D-1 question was asked of.
        "origin chain",
        "8 in-root edges",
        // S4b/F2: the license, its exact document shape, and the wording discipline on its receipts.
        "--disposition-list",
        "OWNER DISPOSITION",
        "authorized_by",
        "list_sha256",
        "THE CONTENT-ON-MAIN GATE IS REPLACED",
        "A DIRTY dispositioned clone PARKS",
        // S4b review repairs: the loss under a license is enumerable, the chain's terminal root is
        // corroborated, and deletion order cannot strand a chained child.
        "ref_inventory",
        "root corrobor",
        "lineage root",
        "GIT_CEILING_DIRECTORIES",
        "BEFORE the first removal",
    ] {
        assert!(
            clones.contains(expected),
            "the clone page omits `{expected}`:\n{clones}"
        );
    }

    let targets = stdout(&run(&["storage", "reap", "--help"]));
    assert!(
        targets.starts_with("usage: a2a-bridge storage reap --build-targets"),
        "`reap --help` did not print the build-target reaper's page:\n{targets}"
    );

    // The umbrella page still answers for the non-destructive surfaces.
    for args in [
        vec!["storage", "--help"],
        vec!["storage", "report", "--help"],
    ] {
        let umbrella = stdout(&run(&args));
        assert!(
            umbrella.starts_with("usage: a2a-bridge storage report"),
            "{args:?} did not print the umbrella page:\n{umbrella}"
        );
    }
}

/// Discriminates a destructive command that infers a payload class. Neither a bare `storage reap` nor
/// both class flags at once may be interpreted: the first has no default, and the second would run two
/// different gate sets under one unauditable output.
#[test]
fn a_reap_without_exactly_one_class_flag_is_refused() {
    let bare = run(&["storage", "reap"]);
    assert!(!bare.status.success(), "a bare `storage reap` was accepted");
    assert!(
        stderr(&bare).contains("--build-targets or --clones"),
        "the refusal does not name the available classes: {}",
        stderr(&bare)
    );

    let both = run(&["storage", "reap", "--build-targets", "--clones"]);
    assert!(
        !both.status.success(),
        "both class flags at once were accepted"
    );
    assert!(
        stderr(&both).contains("separate authorities"),
        "the refusal does not explain why: {}",
        stderr(&both)
    );

    // And a class flag alone still needs a resolvable config, so neither refusal above is masking a
    // command that simply never runs.
    let missing = run(&[
        "storage",
        "reap",
        "--clones",
        "--dry-run",
        "--config",
        "/nonexistent/a2a-bridge.toml",
    ]);
    assert!(!missing.status.success());
    let text = stderr(&missing);
    assert!(
        text.contains("/nonexistent/a2a-bridge.toml"),
        "the refusal does not name the config it could not read: {text}"
    );
}

/// S4b/F2. The license belongs to ONE authority. `--build-targets` has no content-on-main gate for a
/// disposition to replace, so a list handed to it would be silently inert — and an operator who
/// believed a deletion was authorized when nothing read the authorization is precisely the state this
/// refuses. Discriminates a flag parsed for every class and used by one.
#[test]
fn a_disposition_list_without_the_clones_authority_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("d.json");
    std::fs::write(
        &list,
        "{\"authorized_by\":\"owner\",\"reason\":\"abandoned\",\"run_ids\":[\"impl-1-a\"]}",
    )
    .unwrap();
    let path = list.to_str().unwrap();

    for args in [
        vec!["storage", "reap", "--disposition-list", path],
        vec![
            "storage",
            "reap",
            "--build-targets",
            "--disposition-list",
            path,
        ],
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "{args:?} was accepted");
        assert!(
            stderr(&out).contains("--disposition-list applies only to `--clones`"),
            "{args:?} did not name the misuse: {}",
            stderr(&out)
        );
    }

    // The value is required, and a missing file is refused before anything is scanned.
    let bare = run(&["storage", "reap", "--clones", "--disposition-list"]);
    assert!(!bare.status.success());
    assert!(stderr(&bare).contains("--disposition-list needs a <path>"));
}

/// S4b/F2. A license is a DELETION AUTHORIZATION: a document this binary cannot read exactly is a
/// command failure, never a warning that leaves the reap running with the license silently ignored.
/// Discriminates a lenient decode and a swallowed load error — the operator would otherwise read
/// "0 deleted" as "my authorized runs were considered and refused".
#[test]
fn a_malformed_disposition_list_fails_the_command_before_anything_is_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let repo: PathBuf = dir.path().join("workspace");
    std::fs::create_dir_all(repo.join(".a2a-implement")).unwrap();
    let config = dir.path().join("a2a-bridge.toml");
    std::fs::write(
        &config,
        format!(
            "default = \"codex\"\nallowed_cwd_root = {repo:?}\n[server]\naddr = \"127.0.0.1:0\"\n\
             [[agents]]\nid = \"codex\"\ncmd = \"codex\"\n"
        ),
    )
    .unwrap();

    let cases = [
        (
            "unknown-field.json",
            "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[\"impl-1-a\"],\"note\":\"x\"}",
            "unknown field",
        ),
        (
            "empty-runs.json",
            "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[]}",
            "`run_ids` is empty",
        ),
        ("empty.json", "", "is empty"),
    ];
    for (name, body, expected) in cases {
        let list = dir.path().join(name);
        std::fs::write(&list, body).unwrap();
        let out = run(&[
            "storage",
            "reap",
            "--clones",
            "--dry-run",
            "--config",
            config.to_str().unwrap(),
            "--disposition-list",
            list.to_str().unwrap(),
        ]);
        assert!(
            !out.status.success(),
            "{name} was accepted as a license:\n{}",
            stdout(&out)
        );
        let text = stderr(&out);
        assert!(
            text.contains("--disposition-list") && text.contains(expected),
            "{name}: the refusal does not say what was wrong ({expected}): {text}"
        );
    }

    // The control: the SAME invocation with a well-formed list runs to completion, so the refusals
    // above are the document's doing and not a command that never works with the flag.
    let good = dir.path().join("good.json");
    std::fs::write(
        &good,
        "{\"authorized_by\":\"owner\",\"reason\":\"abandoned\",\"run_ids\":[\"impl-1-a\"]}",
    )
    .unwrap();
    let out = run(&[
        "storage",
        "reap",
        "--clones",
        "--dry-run",
        "--config",
        config.to_str().unwrap(),
        "--disposition-list",
        good.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "a well-formed license was refused:\n{}\n{}",
        stdout(&out),
        stderr(&out)
    );
    let text = stdout(&out);
    assert!(
        text.contains("OWNER DISPOSITION LICENSE IN FORCE"),
        "the report does not disclose the license: {text}"
    );
    assert!(
        text.contains("impl-1-a"),
        "the report does not note the run id the scan never found: {text}"
    );
}

/// The `--clones` reaper must never enumerate a root it was not pointed at. Discriminates a command
/// that falls back to the process cwd (or to `[worktrees]`) when `allowed_cwd_root` names a directory
/// with no implement root: it would then classify, and gate, whatever happened to be underneath.
#[test]
fn a_missing_implement_root_refuses_rather_than_scanning_something_else() {
    let dir = tempfile::tempdir().unwrap();
    let repo: PathBuf = dir.path().join("workspace");
    std::fs::create_dir_all(&repo).unwrap();
    let config = dir.path().join("a2a-bridge.toml");
    std::fs::write(
        &config,
        format!(
            "default = \"codex\"\nallowed_cwd_root = {repo:?}\n[server]\naddr = \"127.0.0.1:0\"\n\
             [[agents]]\nid = \"codex\"\ncmd = \"codex\"\n"
        ),
    )
    .unwrap();

    let out = run(&[
        "storage",
        "reap",
        "--clones",
        "--dry-run",
        "--config",
        config.to_str().unwrap(),
    ]);
    assert!(
        !out.status.success(),
        "a reap with no implement root reported success:\n{}",
        stdout(&out)
    );
    let text = stderr(&out);
    assert!(
        text.contains(".a2a-implement"),
        "the refusal does not name the root it expected: {text}"
    );
}

/// A daemon timeout cannot turn missing evidence into permission to discard a clone, even with an
/// explicit owner disposition. Exercise invocation-scan and recorded-runtime queries separately.
#[cfg(unix)]
fn hung_runtime_keeps_clone(scan_hangs: bool) {
    use std::{
        os::unix::{fs::PermissionsExt, process::CommandExt},
        process::Stdio,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let source = root.join("source");
    std::fs::create_dir(&source).unwrap();
    let git = |path: &std::path::Path, args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&source, &["init", "-q", "--initial-branch=main"]);
    git(&source, &["commit", "--allow-empty", "-qm", "base"]);
    let run_id = "impl-4294967294-deadline";
    let clone = root.join(".a2a-implement").join(run_id);
    std::fs::create_dir(clone.parent().unwrap()).unwrap();
    git(
        &source,
        &[
            "clone",
            "-q",
            source.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(&clone, &["update-ref", "refs/heads/wip/deadline", "HEAD"]);
    let refs_before = git(&clone, &["show-ref"]);
    let marker = root.join("started");
    let hanging = root.join("hanging-runtime");
    let healthy = root.join("healthy-runtime");
    // The descendant owns the inherited pipes. Base behavior kills only the shell at the existing
    // recorded-runtime timeout; checking the descendant catches that incomplete settlement.
    std::fs::write(&hanging, "#!/bin/sh\n[ \"$1\" = volume ] && exit 0\nsleep 120 &\necho \"$$ $!\" >> \"$STARTED\"\nwait\n").unwrap();
    std::fs::write(&healthy, "#!/bin/sh\nexit 0\n").unwrap();
    for path in [&hanging, &healthy] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let invocation = root.join("invocation.toml");
    let recorded = root.join("recorded.toml");
    for (config, runtime) in [
        (&invocation, if scan_hangs { &hanging } else { &healthy }),
        (&recorded, if scan_hangs { &healthy } else { &hanging }),
    ] {
        std::fs::write(config, format!("default = \"test\"\nallowed_cwd_root = {root:?}\n[server]\naddr = \"127.0.0.1:0\"\n[verify]\nruntime = {runtime:?}\nimage = \"test\"\ncache = \"test\"\negress = \"open\"\n")).unwrap();
    }
    let checkpoint = serde_json::json!({
        "schema_version": 1, "resume_id": run_id, "task_id": run_id, "task_brief": "deadline test",
        "source_repo": source, "clone_path": clone, "config_path": recorded,
        "branch": "main", "base_commit": git(&clone, &["rev-parse", "HEAD"]),
        "edit_workflow": "edit", "fix_workflow": "fix", "loop_max_attempts": 3,
        "attempt_next": 1, "phase": "Approved", "created_at_ms": 0, "updated_at_ms": 0
    });
    let ck_dir = clone.join(".git/a2a-bridge");
    std::fs::create_dir_all(&ck_dir).unwrap();
    std::fs::write(
        ck_dir.join("implement-checkpoint.json"),
        serde_json::to_vec(&checkpoint).unwrap(),
    )
    .unwrap();
    let disposition = root.join("disposition.json");
    std::fs::write(&disposition, serde_json::to_vec(&serde_json::json!({"authorized_by":"owner", "reason":"discard parked run", "run_ids":[run_id]})).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["storage", "reap", "--clones", "--json", "--config"])
        .arg(&invocation)
        .arg("--disposition-list")
        .arg(&disposition)
        .env("STARTED", &marker)
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let budget = Duration::from_secs(if scan_hangs { 38 } else { 18 });
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() >= budget {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            panic!("storage reap exceeded runtime query budget");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(
        text.contains("ContainerAxisUnanswered")
            || text.contains("container_axis_unanswered")
            || text.contains("container axis unanswered"),
        "{text}"
    );
    assert!(clone.exists());
    assert_eq!(git(&clone, &["show-ref"]), refs_before);
    let records = std::fs::read_to_string(&marker).expect("hanging runtime started");
    for record in records.lines() {
        let pids: Vec<i32> = record
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        for pid in pids {
            let settled_by = Instant::now() + Duration::from_secs(2);
            let alive = loop {
                let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                    s.split_once(") ")
                        .is_some_and(|(_, rest)| rest.starts_with('Z'))
                });
                let alive = !zombie && unsafe { libc::kill(pid, 0) } == 0;
                if !alive || Instant::now() >= settled_by {
                    break alive;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            if alive {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            assert!(!alive, "runtime process {pid} survived query timeout");
        }
    }
}

#[cfg(unix)]
#[test]
fn hung_storage_scan_preserves_clone_and_refs_with_owner_disposition() {
    hung_runtime_keeps_clone(true);
}

#[cfg(unix)]
#[test]
fn hung_recorded_runtime_preserves_clone_and_refs_with_owner_disposition() {
    hung_runtime_keeps_clone(false);
}
