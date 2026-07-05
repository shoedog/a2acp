//! The `implement` build+test VERIFY step: run each configured command as its own container (sharing a
//! per-repo cache), read each CONTAINER exit code (unforgeable — agent code in `cargo test` can't fake
//! it), aggregate a reported (non-gating) verdict for the operator hand-off. The Docker run is the only
//! impure piece (`docker_runner`, live-gated); everything else is pure + unit-tested.
//!
/// Parsed `[verify]`: verify infrastructure + a validated egress policy.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    pub runtime: Option<String>,
    pub image: String,
    pub cache: String,
    pub egress: bridge_core::domain::EgressPolicy,
}

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

/// PURE. Clamp captured output to ~`max` bytes, keeping the HEAD (early context: which command, the
/// first errors) AND the TAIL (where a failing command's failure list + summary live — cargo prints those
/// last). A head-only clamp would hide the very failure the operator needs.
pub fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = max / 4;
    let tail = max - head;
    let mut h = head;
    while h > 0 && !s.is_char_boundary(h) {
        h -= 1;
    }
    let mut t = s.len() - tail;
    while t < s.len() && !s.is_char_boundary(t) {
        t += 1;
    }
    format!("{}\n…[truncated {} bytes]…\n{}", &s[..h], t - h, &s[t..])
}

/// PURE. The fix-turn digest: ONLY the GATE failures (the ones that fail the verdict + drive `actionable`),
/// in order, each `### <name>` + its (truncated) output. Non-gate failures are reported in the hand-off but
/// never re-prompted. Empty when no gate failed. `run_verify` stops at the first gate failure, so this is
/// normally one entry; the per-result budget splits `max_bytes` across however many there are.
pub fn failure_digest(v: &VerifyVerdict, max_bytes: usize) -> String {
    let failed: Vec<&VerifyResult> = v.results.iter().filter(|r| r.gate && !r.ok).collect();
    if failed.is_empty() {
        return String::new();
    }
    let per = (max_bytes / failed.len()).max(1);
    let mut s = String::new();
    for r in failed {
        s.push_str("### ");
        s.push_str(&r.name);
        s.push('\n');
        let body = if r.output.trim().is_empty() {
            "(no output)"
        } else {
            &r.output
        };
        s.push_str(&truncate_output(body, per));
        s.push('\n');
    }
    s
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
    /// The step did not run to completion (e.g. a pre-verify worktree reset failed) — the loop sentinel +
    /// catch-all so the always-print hand-off has a defined value. (B2b-3b.)
    Incomplete,
    /// The user explicitly opted out via `--lang none`; distinct from NotConfigured so the hand-off shows
    /// SKIPPED rather than "not configured".
    Skipped {
        reason: String,
    },
}

/// PURE. The hand-off suffix (stdout) for each outcome. Failing-command OUTPUT is dumped to stderr by the
/// caller; this is the one-line summary appended to the operator hand-off.
pub fn outcome_suffix(o: &VerifyOutcome) -> String {
    match o {
        VerifyOutcome::Ran(v) => verdict_line(v),
        VerifyOutcome::NotConfigured => "verify: not configured".to_string(),
        VerifyOutcome::ConfigError => "verify: skipped (config error)".to_string(),
        VerifyOutcome::Incomplete => "verify: incomplete (did not finish)".to_string(),
        VerifyOutcome::Skipped { reason } => format!("verify: SKIPPED ({reason})"),
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
/// Returns `VerifyOutcome::Skipped` when `profile` is `None` (`--lang none`) without spawning any container.
pub fn run_verify(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
) -> VerifyOutcome {
    let profile = match profile {
        None => {
            return VerifyOutcome::Skipped {
                reason: "--lang none".into(),
            }
        }
        Some(p) => p,
    };
    let mut results = Vec::new();
    let binding = profile.cache_binding(bridge_core::profile::CacheCtx::Verify, "", cache_vol);
    let image = profile.image.as_deref().unwrap_or(&cfg.image);
    for c in &profile.verify_commands {
        let (prog, argv) = bridge_core::sandbox::compose_verify(
            cfg.runtime.as_deref(),
            image,
            &cfg.egress,
            clone,
            &binding,
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
    VerifyOutcome::Ran(aggregate(results))
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
    fn truncate_keeps_head_and_tail() {
        let s = format!("{}{}", "H".repeat(50), "T".repeat(50)); // 100 bytes
        let out = truncate_output(&s, 20); // head=5, tail=15
        assert!(out.starts_with("HHHHH")); // head kept (early context)
        assert!(out.ends_with(&"T".repeat(15))); // tail kept (the failure lives here)
        assert!(out.contains("truncated 80 bytes"));
        assert_eq!(truncate_output("short", 20), "short");
    }

    #[test]
    fn verdict_line_pass_and_fail() {
        let pass = aggregate(vec![r("fmt", true, true), r("test", true, true)]);
        assert_eq!(verdict_line(&pass), "verify: PASS  (fmt ✓ · test ✓)");
        let fail = aggregate(vec![r("fmt", true, true), r("clippy", true, false)]);
        assert_eq!(
            verdict_line(&fail),
            "verify: FAIL at clippy  (fmt ✓ · clippy ✗)"
        );
    }

    #[test]
    fn cache_volume_name_is_stable_and_per_repo() {
        let a = cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-a");
        let b = cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-b");
        assert_eq!(
            a,
            cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-a")
        );
        assert_ne!(a, b);
        assert!(a.starts_with("a2a-verify-cache-"));
    }

    #[test]
    fn outcome_suffix_covers_all_arms() {
        let ran = VerifyOutcome::Ran(aggregate(vec![r("fmt", true, true)]));
        assert!(outcome_suffix(&ran).starts_with("verify: PASS"));
        let failed = VerifyOutcome::Ran(aggregate(vec![r("clippy", true, false)]));
        assert!(outcome_suffix(&failed).starts_with("verify: FAIL"));
        assert_eq!(
            outcome_suffix(&VerifyOutcome::NotConfigured),
            "verify: not configured"
        );
        assert_eq!(
            outcome_suffix(&VerifyOutcome::ConfigError),
            "verify: skipped (config error)"
        );
        assert_eq!(
            outcome_suffix(&VerifyOutcome::Incomplete),
            "verify: incomplete (did not finish)"
        );
        assert_eq!(
            outcome_suffix(&VerifyOutcome::Skipped {
                reason: "--lang none".into()
            }),
            "verify: SKIPPED (--lang none)"
        );
    }

    #[test]
    fn failure_digest_only_failed_gates_with_budget() {
        let v = aggregate(vec![
            VerifyResult {
                name: "fmt".into(),
                gate: true,
                ok: true,
                output: "ok".into(),
            },
            VerifyResult {
                name: "clippy".into(),
                gate: true,
                ok: false,
                output: "E".repeat(50),
            },
        ]);
        let d = failure_digest(&v, 20);
        assert!(d.contains("### clippy"));
        assert!(!d.contains("### fmt"));
        assert!(d.contains("truncated"));
    }

    #[test]
    fn failure_digest_empty_when_no_gate_failures() {
        let v = aggregate(vec![
            VerifyResult {
                name: "test".into(),
                gate: true,
                ok: true,
                output: "ok".into(),
            },
            VerifyResult {
                name: "cov".into(),
                gate: false,
                ok: false,
                output: "x".into(),
            },
        ]);
        assert_eq!(failure_digest(&v, 4096), "");
    }

    #[test]
    fn failure_digest_empty_output_placeholder() {
        let v = aggregate(vec![VerifyResult {
            name: "build".into(),
            gate: true,
            ok: false,
            output: "   ".into(),
        }]);
        assert!(failure_digest(&v, 4096).contains("(no output)"));
    }

    fn cfg() -> VerifyConfig {
        VerifyConfig {
            runtime: None,
            image: "img".into(),
            cache: "cache".into(),
            egress: bridge_core::domain::EgressPolicy::Open,
        }
    }

    fn profile(
        cmds: &[(&str, bool)],
        image: Option<&str>,
    ) -> bridge_core::profile::LanguageProfile {
        bridge_core::profile::LanguageProfile::from_parts(
            "rust".into(),
            "cargo fetch --locked".into(),
            "a2a-impl-lsp-cache".into(),
            "/cargo".into(),
            "/cache".into(),
            vec![("CARGO_HOME".into(), "/cargo".into())],
            Vec::new(),
            vec![
                ("CARGO_HOME".into(), "/cache/cargo".into()),
                ("CARGO_TARGET_DIR".into(), "/cache/target".into()),
            ],
            image.map(str::to_string),
            cmds.iter()
                .map(|(c, gate)| bridge_core::profile::VerifyCommand {
                    name: (*c).into(),
                    cmd: format!("cargo {c}"),
                    gate: *gate,
                })
                .collect(),
        )
    }

    fn unwrap_ran(outcome: VerifyOutcome) -> VerifyVerdict {
        match outcome {
            VerifyOutcome::Ran(v) => v,
            other => panic!("expected VerifyOutcome::Ran, got {other:?}"),
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
        let v = unwrap_ran(run_verify(
            &cfg(),
            Some(&profile(
                &[
                    ("fmt", true),
                    ("clippy", true),
                    ("build", true),
                    ("test", true),
                ],
                None,
            )),
            &clone,
            "cache-x",
            &runner,
            4096,
        ));
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
        let v = unwrap_ran(run_verify(
            &cfg(),
            Some(&profile(&[("coverage", false), ("test", true)], None)),
            &clone,
            "cache-x",
            &runner,
            4096,
        ));
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
        let v = unwrap_ran(run_verify(
            &cfg(),
            Some(&profile(&[("build", true)], None)),
            &clone,
            "cache-x",
            &runner,
            4096,
        ));
        assert!(!v.passed);
        assert!(v.results[0].output.contains("docker missing"));
    }

    #[test]
    fn run_verify_none_profile_returns_skipped() {
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
            panic!("runner must NOT be called when profile is None")
        };
        let outcome = run_verify(&cfg(), None, &clone, "cache-x", &runner, 4096);
        assert!(
            matches!(outcome, VerifyOutcome::Skipped { .. }),
            "None profile must return Skipped, not run any container"
        );
    }

    #[test]
    fn run_verify_uses_profile_image_override_or_falls_back_to_verify_image() {
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let seen = std::cell::RefCell::new(Vec::new());
        let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            seen.borrow_mut().push(argv.to_vec());
            Ok((0, "ok".into()))
        };

        let _ = run_verify(
            &cfg(),
            Some(&profile(&[("test", true)], Some("override:img"))),
            &clone,
            "cache-x",
            &runner,
            4096,
        );
        let _ = run_verify(
            &cfg(),
            Some(&profile(&[("test", true)], None)),
            &clone,
            "cache-x",
            &runner,
            4096,
        );

        let seen = seen.borrow();
        assert!(seen[0].iter().any(|a| a == "override:img"));
        assert!(seen[1].iter().any(|a| a == "img"));
    }
}
