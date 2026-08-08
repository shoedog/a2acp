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
