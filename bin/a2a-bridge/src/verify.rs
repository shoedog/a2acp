//! The `implement` build+test VERIFY step: run each configured command as its own container (sharing a
//! per-repo cache), read each CONTAINER exit code (unforgeable — agent code in `cargo test` can't fake
//! it), aggregate a reported (non-gating) verdict for the operator hand-off. The Docker run is the only
//! impure piece (`docker_runner`, live-gated); everything else is pure + unit-tested.
//!
use crate::config::VerifyConfig;

/// One command's outcome. `gate=false` commands are reported but never fail the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyResult {
    pub name: String,
    pub gate: bool,
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyVerdict {
    pub results: Vec<VerifyResult>,
    pub passed: bool,
}

/// PURE. The verdict passes iff every GATE command succeeded (non-gate commands are reported only).
pub fn aggregate(results: Vec<VerifyResult>) -> VerifyVerdict {
    let passed = results.iter().all(|r| !r.gate || r.ok);
    VerifyVerdict { results, passed }
}

/// PURE. Clamp captured output to `max` bytes on a char boundary, marking truncation.
pub fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated {} bytes]", &s[..end], s.len() - end)
}

/// PURE. The one-line verdict for the operator hand-off (stdout). Failing-command OUTPUT goes to
/// stderr separately; this is the summary line.
pub fn verdict_line(v: &VerifyVerdict) -> String {
    let marks: Vec<String> = v
        .results
        .iter()
        .map(|r| format!("{} {}", r.name, if r.ok { "✓" } else { "✗" }))
        .collect();
    if v.passed {
        format!("verify: PASS  ({})", marks.join(" · "))
    } else {
        let failed = v
            .results
            .iter()
            .find(|r| r.gate && !r.ok)
            .map(|r| r.name.as_str())
            .unwrap_or("?");
        format!("verify: FAIL at {}  ({})", failed, marks.join(" · "))
    }
}

/// The three terminal states of the verify step (the riskiest classification — extracted pure so the
/// `Action::Commit` wiring is unit-tested, mirroring B2b-1's `implement::decide`).
#[derive(Debug, Clone)]
pub enum VerifyOutcome {
    Ran(VerifyVerdict),
    NotConfigured,
    /// The `[verify]` block failed validation; the detail is logged to stderr at the call site.
    ConfigError,
}

/// PURE. The hand-off suffix (stdout) for each outcome. Failing-command OUTPUT is dumped to stderr by the
/// caller; this is the one-line summary appended to the operator hand-off.
pub fn outcome_suffix(o: &VerifyOutcome) -> String {
    match o {
        VerifyOutcome::Ran(v) => verdict_line(v),
        VerifyOutcome::NotConfigured => "verify: not configured".to_string(),
        VerifyOutcome::ConfigError => "verify: skipped (config error)".to_string(),
    }
}

/// PURE. A stable per-repo cache volume name: `<base>-<hash(canonical repo path)>`. Per-repo keying
/// isolates repos; same-repo runs share (single-flight serializes them — see `run_verify`'s caller).
/// Reuses the codebase's `DefaultHasher` owner-token pattern (`main::container_owner`). The CALLER passes
/// the CANONICAL repo path (Task 5 canonicalizes `a.repo`) — two spellings must not split the cache.
pub fn cache_volume_name(base: &str, repo_canon: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    repo_canon.hash(&mut h);
    format!("{base}-{:016x}", h.finish())
}

/// A command runner: given `(program, argv)`, run it and return `(exit_code, combined_output)`. The real
/// impl spawns Docker; tests inject a stub. The exit code is the CONTAINER's — unforgeable by in-container
/// agent code.
pub type Runner<'a> = dyn Fn(&str, &[String]) -> std::io::Result<(i32, String)> + 'a;

/// Run every configured command as its own container (sharing the per-repo cache volume), reading each
/// container's exit code. Stops at the FIRST gate failure. Pure given an injected `runner`.
pub fn run_verify(
    cfg: &VerifyConfig,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
) -> VerifyVerdict {
    let mut results = Vec::new();
    for c in &cfg.commands {
        let (prog, argv) = bridge_core::sandbox::compose_verify(
            cfg.runtime.as_deref(),
            &cfg.image,
            &cfg.egress,
            clone,
            cache_vol,
            &c.cmd,
        );
        let (exit, out) = match runner(&prog, &argv) {
            Ok((e, o)) => (e, o),
            Err(e) => (-1, format!("verify: runner error: {e}")),
        };
        let ok = exit == 0;
        results.push(VerifyResult {
            name: c.name.clone(),
            gate: c.gate,
            ok,
            output: truncate_output(&out, max_bytes),
        });
        if c.gate && !ok {
            break; // stop at the first gate failure
        }
    }
    aggregate(results)
}

/// The real runner: spawn the container, capture stdout+stderr combined, return the exit code.
pub fn docker_runner(program: &str, argv: &[String]) -> std::io::Result<(i32, String)> {
    let out = std::process::Command::new(program).args(argv).output()?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), combined))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, gate: bool, ok: bool) -> VerifyResult {
        VerifyResult {
            name: name.into(),
            gate,
            ok,
            output: String::new(),
        }
    }

    #[test]
    fn aggregate_passes_when_all_gates_pass() {
        let v = aggregate(vec![r("fmt", true, true), r("test", true, true)]);
        assert!(v.passed);
    }

    #[test]
    fn aggregate_fails_on_a_gate_failure() {
        let v = aggregate(vec![r("fmt", true, true), r("clippy", true, false)]);
        assert!(!v.passed);
    }

    #[test]
    fn aggregate_ignores_a_nongate_failure() {
        let v = aggregate(vec![r("test", true, true), r("coverage", false, false)]);
        assert!(v.passed);
    }

    #[test]
    fn truncate_marks_oversized_output() {
        let out = truncate_output(&"x".repeat(100), 10);
        assert!(out.starts_with(&"x".repeat(10)));
        assert!(out.contains("truncated 90 bytes"));
        assert_eq!(truncate_output("short", 10), "short");
    }

    #[test]
    fn verdict_line_pass_and_fail() {
        let pass = aggregate(vec![r("fmt", true, true), r("test", true, true)]);
        assert_eq!(verdict_line(&pass), "verify: PASS  (fmt ✓ · test ✓)");
        let fail = aggregate(vec![r("fmt", true, true), r("clippy", true, false)]);
        assert_eq!(verdict_line(&fail), "verify: FAIL at clippy  (fmt ✓ · clippy ✗)");
    }

    #[test]
    fn cache_volume_name_is_stable_and_per_repo() {
        let a = cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-a");
        let b = cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-b");
        assert_eq!(a, cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-a"));
        assert_ne!(a, b);
        assert!(a.starts_with("a2a-verify-cache-"));
    }

    #[test]
    fn outcome_suffix_covers_three_arms() {
        let ran = VerifyOutcome::Ran(aggregate(vec![r("fmt", true, true)]));
        assert!(outcome_suffix(&ran).starts_with("verify: PASS"));
        let failed = VerifyOutcome::Ran(aggregate(vec![r("clippy", true, false)]));
        assert!(outcome_suffix(&failed).starts_with("verify: FAIL"));
        assert_eq!(outcome_suffix(&VerifyOutcome::NotConfigured), "verify: not configured");
        assert_eq!(
            outcome_suffix(&VerifyOutcome::ConfigError),
            "verify: skipped (config error)"
        );
    }

    use crate::config::VerifyCommand;

    fn cfg(cmds: &[(&str, bool)]) -> VerifyConfig {
        VerifyConfig {
            runtime: None,
            image: "img".into(),
            cache: "cache".into(),
            egress: bridge_core::domain::EgressPolicy::Open,
            commands: cmds
                .iter()
                .map(|(c, gate)| VerifyCommand {
                    name: (*c).into(),
                    cmd: format!("cargo {c}"),
                    gate: *gate,
                })
                .collect(),
        }
    }

    #[test]
    fn run_verify_stops_at_first_gate_failure() {
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        // runner: fmt ok, clippy FAILS (gate) -> build/test must NOT run.
        let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            let script = argv.last().unwrap();
            if script.contains("cargo clippy") {
                Ok((1, "error: clippy".into()))
            } else {
                Ok((0, "ok".into()))
            }
        };
        let v = run_verify(
            &cfg(&[("fmt", true), ("clippy", true), ("build", true), ("test", true)]),
            &clone,
            "cache-x",
            &runner,
            4096,
        );
        assert!(!v.passed);
        assert_eq!(v.results.len(), 2); // stopped after clippy
        assert_eq!(v.results[1].name, "clippy");
        assert!(!v.results[1].ok);
    }

    #[test]
    fn run_verify_reports_nongate_failure_then_continues_to_a_later_gate() {
        // The failing NON-GATE command is FIRST, a GATE command FOLLOWS — so a buggy "stop on ANY failure"
        // impl would stop after coverage (len==1) and this test would catch it. (codex plan-review catch.)
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            let script = argv.last().unwrap();
            if script.contains("cargo coverage") {
                Ok((1, "cov fail".into()))
            } else {
                Ok((0, "ok".into()))
            }
        };
        let v = run_verify(
            &cfg(&[("coverage", false), ("test", true)]),
            &clone,
            "cache-x",
            &runner,
            4096,
        );
        assert!(v.passed); // the non-gate coverage failure doesn't fail the verdict
        assert_eq!(v.results.len(), 2); // the later GATE ran (did NOT stop on the non-gate failure)
        assert_eq!(v.results[1].name, "test");
        assert!(v.results[1].ok);
    }

    #[test]
    fn run_verify_runner_error_is_a_failure() {
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
            Err(std::io::Error::other("docker missing"))
        };
        let v = run_verify(&cfg(&[("build", true)]), &clone, "cache-x", &runner, 4096);
        assert!(!v.passed);
        assert!(v.results[0].output.contains("docker missing"));
    }
}
