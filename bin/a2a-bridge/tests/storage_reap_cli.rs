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
