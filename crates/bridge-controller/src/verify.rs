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
    pub reach: VerifyReach,
    pub failure_class: Option<VerifyFailureClass>,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyReach {
    Reached { exit_status: i32 },
    NotReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyFailureClass {
    Command,
    Runner,
    LockSync,
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

/// PURE. The fix-turn digest: reached GATE failures that fail the verdict + drive `actionable`, each
/// `### <name>` + its truncated output, followed by an explicit list of later gates that were not reached.
/// Non-gate failures are reported in the hand-off but never re-prompted. Empty when no reached gate failed.
pub fn failure_digest(v: &VerifyVerdict, max_bytes: usize) -> String {
    let failed: Vec<&VerifyResult> = v
        .results
        .iter()
        .filter(|r| r.gate && !r.ok && matches!(r.reach, VerifyReach::Reached { .. }))
        .collect();
    if failed.is_empty() {
        return String::new();
    }
    let per = (max_bytes / failed.len()).max(1);
    let mut s = String::new();
    for r in failed {
        s.push_str("### ");
        s.push_str(&r.name);
        if let VerifyReach::Reached { exit_status } = r.reach {
            s.push_str(&format!(" (reached, exit={exit_status}"));
            if let Some(class) = r.failure_class {
                s.push_str(", ");
                s.push_str(failure_class_label(class));
            }
            s.push(')');
        }
        s.push('\n');
        let body = if r.output.trim().is_empty() {
            "(no output)"
        } else {
            &r.output
        };
        s.push_str(&truncate_output(body, per));
        s.push('\n');
    }
    let not_reached: Vec<&str> = v
        .results
        .iter()
        .filter(|r| r.gate && matches!(r.reach, VerifyReach::NotReached))
        .map(|r| r.name.as_str())
        .collect();
    if !not_reached.is_empty() {
        s.push_str("### Not reached gates\n");
        s.push_str(&not_reached.join(", "));
        s.push('\n');
    }
    s
}

/// PURE. The one-line verdict for the operator hand-off (stdout). Failing-command OUTPUT goes to
/// stderr separately; this is the summary line.
pub fn verdict_line(v: &VerifyVerdict) -> String {
    let marks: Vec<String> = v.results.iter().map(result_mark).collect();
    if v.passed {
        format!("verify: PASS  ({})", marks.join(" · "))
    } else {
        let failed = v
            .results
            .iter()
            .find(|r| r.gate && !r.ok && matches!(r.reach, VerifyReach::Reached { .. }))
            .or_else(|| v.results.iter().find(|r| r.gate && !r.ok))
            .map(|r| {
                if r.failure_class == Some(VerifyFailureClass::LockSync) {
                    format!("lock-sync before {}", r.name)
                } else {
                    r.name.clone()
                }
            })
            .unwrap_or_else(|| "?".to_string());
        format!("verify: FAIL at {}  ({})", failed, marks.join(" · "))
    }
}

fn result_mark(r: &VerifyResult) -> String {
    match r.reach {
        VerifyReach::Reached { exit_status } => {
            let mut mark = format!(
                "{} reached exit={} {}",
                r.name,
                exit_status,
                if r.ok { "✓" } else { "✗" }
            );
            if let Some(class) = r.failure_class {
                mark.push(' ');
                mark.push_str(failure_class_label(class));
            }
            mark
        }
        VerifyReach::NotReached => format!("{} not-reached", r.name),
    }
}

fn failure_class_label(class: VerifyFailureClass) -> &'static str {
    match class {
        VerifyFailureClass::Command => "command",
        VerifyFailureClass::Runner => "runner",
        VerifyFailureClass::LockSync => "lock-sync",
    }
}

fn classify_verify_failure(
    exit_status: i32,
    output: &str,
    runner_error: bool,
) -> Option<VerifyFailureClass> {
    if exit_status == 0 {
        return None;
    }
    if runner_error {
        return Some(VerifyFailureClass::Runner);
    }
    if is_lock_sync_failure(output) {
        return Some(VerifyFailureClass::LockSync);
    }
    Some(VerifyFailureClass::Command)
}

fn is_lock_sync_failure(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("--locked")
        && (lower.contains("cargo.lock")
            || lower.contains("lock file")
            || lower.contains("lockfile")
            || lower.contains("needs to be updated"))
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
/// isolates repos; same-repo runs SHARE one volume, and `CARGO_HOME`/`CARGO_TARGET_DIR` point inside it —
/// so concurrent same-repo verifies would write one another's cargo state. They do not, because
/// [`run_verify_serialized`] holds an exclusive flock named for this volume across the container run.
/// (Before that lock existed this comment claimed a "single-flight" serialization that nothing
/// implemented — the S5 defect.)
/// Reuses the codebase's `DefaultHasher` owner-token pattern (`main::container_owner`). The CALLER passes
/// the CANONICAL repo path (Task 5 canonicalizes `a.repo`) — two spellings must not split the cache.
pub fn cache_volume_name(base: &str, repo_canon: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    repo_canon.hash(&mut h);
    format!("{base}-{:016x}", h.finish())
}

/// The prefix that keeps cache-volume locks disjoint from the run operation locks sharing their
/// directory. Run locks are named for a run id — `impl-<pid>-<nonce>` (`implement::task_id`) — which can
/// never begin with `verify-`, so the two namespaces cannot collide.
pub const CACHE_VOLUME_LOCK_PREFIX: &str = "verify-";

/// PURE. The lock id serializing verify runs that share one cache volume (file: `verify-<volume>.lock`).
pub fn cache_volume_lock_id(cache_vol: &str) -> String {
    format!("{CACHE_VOLUME_LOCK_PREFIX}{cache_vol}")
}

/// PURE. The operator note printed once when a verify is about to wait for another verify of the same
/// repository. Pure so its content is asserted by a test rather than by scraping stderr.
pub fn cache_volume_wait_note(cache_vol: &str) -> String {
    format!(
        "[implement] verify: waiting for a concurrent verify of the same repository \
         (cache volume {cache_vol}) to finish..."
    )
}

/// The Docker volume-name charset (`[a-zA-Z0-9][a-zA-Z0-9_.-]*`). A `[verify].cache` base outside it is
/// already unusable as a volume name; refusing it here also keeps the lock id a single, literal path
/// component (no `/`, no `..`, no NUL) so a lock can never be created outside its namespace.
fn is_volume_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Where the cache-volume lock lives for a run whose quarantine clone is `clone`.
///
/// `<implement root>/.operation-locks/` — the SAME namespace as the run operation locks
/// (`implement_resume::acquire_operation_lock`) — whenever `clone`'s parent is an `.a2a-implement` root:
/// it is a sibling of the clones (guarded clone reaping can never unlink a held lock inode) and it is the
/// directory `storage report` already knows about. Any other shape (a verify outside an implement run)
/// falls back to the host-global lease dir, which always exists as a namespace.
///
/// KNOWN SCOPE (disclosed, not closed): the cache volume is host-global (one Docker daemon), while this
/// namespace is per-implement-root. Two implement roots on ONE host pointing at the same source repo would
/// therefore key the same volume under two different lock directories and would not serialize. Deployment
/// keeps a single `allowed_cwd_root` per host (it is the container mount anchor), so one root holds every
/// run; if that ever stops holding, move this to `liveness::lease_dir()`, whose domain matches the volume's.
pub fn cache_volume_lock_dir(clone: &std::path::Path) -> std::path::PathBuf {
    match clone.parent() {
        Some(root) if root.file_name() == Some(std::ffi::OsStr::new(IMPLEMENT_ROOT_DIR)) => {
            root.join(crate::implement_resume::OPERATION_LOCK_DIR)
        }
        _ => bridge_core::liveness::lease_dir(),
    }
}

/// The quarantine-clone root created by `implement` (`<allowed_cwd_root>/.a2a-implement`).
const IMPLEMENT_ROOT_DIR: &str = ".a2a-implement";

/// Take the exclusive per-cache-volume verify lock, WAITING (not failing) for any concurrent same-volume
/// verify: a verify legitimately runs for minutes, so a `WouldBlock` refusal would strand real work.
pub fn acquire_cache_volume_lock(
    lock_dir: &std::path::Path,
    cache_vol: &str,
) -> std::io::Result<bridge_core::liveness::PersistentLockGuard> {
    if !is_volume_name(cache_vol) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("cache volume name {cache_vol:?} is not a usable lock id"),
        ));
    }
    let note = || eprintln!("{}", cache_volume_wait_note(cache_vol));
    bridge_core::liveness::acquire_persistent_lock_blocking_in(
        lock_dir,
        &cache_volume_lock_id(cache_vol),
        &note,
    )
}

/// A command runner: given `(program, argv)`, run it and return `(exit_code, combined_output)`. The real
/// impl spawns Docker; tests inject a stub. The exit code is the CONTAINER's — unforgeable by in-container
/// agent code.
pub type Runner<'a> = dyn Fn(&str, &[String]) -> std::io::Result<(i32, String)> + 'a;

/// Run every configured command as its own container (sharing the per-repo cache volume), reading each
/// container's exit code. Every configured population is reached even after an earlier command failure,
/// so one repair round receives the complete bounded defect set. Pure given an injected `runner`.
/// Returns `VerifyOutcome::Skipped` when `profile` is `None` (`--lang none`) without spawning any container.
pub fn run_verify_recorded(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
    recorder: &dyn bridge_core::attempt_activity::AttemptRecorder,
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
    let mut completed_count = 0_u64;
    for (index, c) in profile.verify_commands.iter().enumerate() {
        let ordinal = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
        let _ = recorder.record(
            bridge_core::attempt_activity::AttemptPhase::Verification,
            bridge_core::attempt_activity::ActivityReason::GateStarted,
            ordinal,
        );
        let (prog, argv) = bridge_core::sandbox::compose_verify(
            cfg.runtime.as_deref(),
            image,
            &cfg.egress,
            clone,
            &binding,
            &c.cmd,
        );
        let (exit, out, runner_error) = match runner(&prog, &argv) {
            Ok((e, o)) => (e, o, false),
            Err(e) => (-1, format!("verify: runner error: {e}"), true),
        };
        let _ = recorder.record(
            bridge_core::attempt_activity::AttemptPhase::Verification,
            bridge_core::attempt_activity::ActivityReason::GateExited,
            ordinal,
        );
        let ok = exit == 0;
        let failure_class = classify_verify_failure(exit, &out, runner_error);
        results.push(VerifyResult {
            name: c.name.clone(),
            gate: c.gate,
            ok,
            reach: VerifyReach::Reached { exit_status: exit },
            failure_class,
            output: truncate_output(&out, max_bytes),
        });
        if ok {
            completed_count = completed_count.saturating_add(1);
            let _ = recorder.record(
                bridge_core::attempt_activity::AttemptPhase::Verification,
                bridge_core::attempt_activity::ActivityReason::CompletedSetGrowth,
                completed_count,
            );
        }
    }
    VerifyOutcome::Ran(aggregate(results))
}

/// [`run_verify_recorded`] with the same-repo serialization its shared cache volume requires: hold an
/// exclusive flock named for `cache_vol` across the WHOLE container run (every configured command — cargo
/// state written by command 1 is read by command 2), and release it on return.
///
/// SCOPE: the guard covers the container run only. Edit turns, review turns and the tweak loop run outside
/// it, so a repair round never holds a repo-wide lock while an agent thinks.
///
/// DEADLOCK ANALYSIS (against the real call graph as of this commit):
/// * Two lock families exist on this path — the per-run operation lock
///   (`<implement root>/.operation-locks/<run id>.lock`, taken by `implement --resume`, `merge` and the
///   clone reaper) and this per-cache-volume lock. Where both are held, the order is fixed: `--resume`
///   takes the operation lock for the run's whole life BEFORE its loop reaches verify, and nothing
///   acquires an operation lock while this guard is alive (the guarded region spawns containers via the
///   injected `runner` and takes no other lock). A cycle would need the reverse order; no path has it.
/// * This lock is single-level: no code path acquires two cache-volume locks, so no hold-and-wait chain
///   between volumes can form. A fresh (non-resume) `implement` holds no operation lock at all while
///   verifying, and takes the merge lock only after this guard has been dropped.
/// * The holder never awaits the waiter: it runs the container to completion, then releases. Waiting is
///   therefore bounded by the holder's own verify, not by any lock the waiter owns.
///
/// A lock that cannot be TAKEN (unwritable namespace, a filesystem without working `flock`, a
/// non-volume-shaped name) degrades LOUDLY to an unserialized run rather than failing the run: the risk it
/// guards is concurrent same-repo verifies, while refusing here would strand a single run that has no
/// contender at all. Contention itself never lands in this arm — that path waits.
#[allow(clippy::too_many_arguments)] // one more than `run_verify_recorded`: its lock namespace
pub fn run_verify_serialized(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
    recorder: &dyn bridge_core::attempt_activity::AttemptRecorder,
    lock_dir: &std::path::Path,
) -> VerifyOutcome {
    if profile.is_none() {
        // No profile ⇒ no container ⇒ nothing to serialize; never create a lock namespace for a skip.
        return run_verify_recorded(cfg, profile, clone, cache_vol, runner, max_bytes, recorder);
    }
    let _cache_volume = match acquire_cache_volume_lock(lock_dir, cache_vol) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!(
                "[implement] verify: could not take the cache-volume lock in {} ({e}) — running \
                 UNSERIALIZED; a concurrent verify of the same repository could corrupt the shared \
                 {cache_vol} cache",
                lock_dir.display()
            );
            None
        }
    };
    run_verify_recorded(cfg, profile, clone, cache_vol, runner, max_bytes, recorder)
}

pub fn run_verify(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
) -> VerifyOutcome {
    run_verify_recorded(
        cfg,
        profile,
        clone,
        cache_vol,
        runner,
        max_bytes,
        &bridge_core::attempt_activity::NoopAttemptRecorder,
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, gate: bool, ok: bool) -> VerifyResult {
        VerifyResult {
            name: name.into(),
            gate,
            ok,
            reach: VerifyReach::Reached {
                exit_status: if ok { 0 } else { 1 },
            },
            failure_class: (!ok).then_some(VerifyFailureClass::Command),
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
        assert_eq!(
            verdict_line(&pass),
            "verify: PASS  (fmt reached exit=0 ✓ · test reached exit=0 ✓)"
        );
        let fail = aggregate(vec![r("fmt", true, true), r("clippy", true, false)]);
        assert_eq!(
            verdict_line(&fail),
            "verify: FAIL at clippy  (fmt reached exit=0 ✓ · clippy reached exit=1 ✗ command)"
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
                reach: VerifyReach::Reached { exit_status: 0 },
                failure_class: None,
                output: "ok".into(),
            },
            VerifyResult {
                name: "clippy".into(),
                gate: true,
                ok: false,
                reach: VerifyReach::Reached { exit_status: 1 },
                failure_class: Some(VerifyFailureClass::Command),
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
                reach: VerifyReach::Reached { exit_status: 0 },
                failure_class: None,
                output: "ok".into(),
            },
            VerifyResult {
                name: "cov".into(),
                gate: false,
                ok: false,
                reach: VerifyReach::Reached { exit_status: 1 },
                failure_class: Some(VerifyFailureClass::Command),
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
            reach: VerifyReach::Reached { exit_status: 1 },
            failure_class: Some(VerifyFailureClass::Command),
            output: "   ".into(),
        }]);
        assert!(failure_digest(&v, 4096).contains("(no output)"));
    }

    #[test]
    fn verdict_reports_reached_exit_and_not_reached_gates() {
        let v = aggregate(vec![
            VerifyResult {
                name: "fmt".into(),
                gate: true,
                ok: true,
                reach: VerifyReach::Reached { exit_status: 0 },
                failure_class: None,
                output: "ok".into(),
            },
            VerifyResult {
                name: "build".into(),
                gate: true,
                ok: false,
                reach: VerifyReach::Reached { exit_status: 101 },
                failure_class: Some(VerifyFailureClass::LockSync),
                output:
                    "error: the lock file Cargo.lock needs to be updated but --locked was passed"
                        .into(),
            },
            VerifyResult {
                name: "test".into(),
                gate: true,
                ok: false,
                reach: VerifyReach::NotReached,
                failure_class: None,
                output: String::new(),
            },
        ]);
        assert_eq!(
            verdict_line(&v),
            "verify: FAIL at lock-sync before build  (fmt reached exit=0 ✓ · build reached exit=101 ✗ lock-sync · test not-reached)"
        );
        let digest = failure_digest(&v, 4096);
        assert!(digest.contains("### build (reached, exit=101, lock-sync)"));
        assert!(digest.contains("### Not reached gates\ntest"));
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
    fn r2f0b_recorded_verify_tracks_only_reached_gate_boundaries_and_successes() {
        use bridge_core::attempt_activity::{
            AttemptRecorder, SharedAttemptRecorder, SystemMonotonicClock,
        };

        let clone = bridge_core::SessionCwd::parse("/repo/private-sentinel").unwrap();
        let runner = |_program: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            if argv.last().is_some_and(|script| script.contains("clippy")) {
                Ok((1, "private failure output".into()))
            } else {
                Ok((0, "private success output".into()))
            }
        };
        let recorder = SharedAttemptRecorder::new(SystemMonotonicClock::start());
        let verdict = unwrap_ran(run_verify_recorded(
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
            "private-cache-sentinel",
            &runner,
            4096,
            &recorder,
        ));
        assert!(!verdict.passed);
        assert!(verdict
            .results
            .iter()
            .all(|result| matches!(result.reach, VerifyReach::Reached { .. })));

        let tally = recorder.tally().expect("recorded verifier owns a tally");
        assert_eq!(tally.meaningful_progress, 11);
        assert_eq!(tally.activity, 0);
        assert_eq!(tally.max_advance, 4);
        let encoded = serde_json::to_string(&tally).unwrap();
        for private in [
            "private-sentinel",
            "private failure output",
            "fmt",
            "clippy",
        ] {
            assert!(!encoded.contains(private), "activity leaked {private:?}");
        }
    }

    #[test]
    fn run_verify_enumerates_every_gate_after_a_failure() {
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let calls = std::cell::Cell::new(0_u32);
        let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            calls.set(calls.get() + 1);
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
        assert_eq!(calls.get(), 4);
        assert_eq!(v.results.len(), 4);
        assert_eq!(v.results[1].name, "clippy");
        assert!(!v.results[1].ok);
        assert!(v.results[2].ok);
        assert!(v.results[3].ok);
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
        assert_eq!(v.results[0].failure_class, Some(VerifyFailureClass::Runner));
    }

    #[test]
    fn run_verify_classifies_locked_precompile_death() {
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
            Ok((
                101,
                "error: the lock file Cargo.lock needs to be updated but --locked was passed"
                    .into(),
            ))
        };
        let v = unwrap_ran(run_verify(
            &cfg(),
            Some(&profile(&[("build", true), ("test", true)], None)),
            &clone,
            "cache-x",
            &runner,
            4096,
        ));
        assert_eq!(
            v.results[0].failure_class,
            Some(VerifyFailureClass::LockSync)
        );
        assert!(matches!(
            v.results[1].reach,
            VerifyReach::Reached { exit_status: 101 }
        ));
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

    // ---- S5: same-repo verify serialization -------------------------------------------------
    //
    // The harness below runs two verifies concurrently against a tempdir lock namespace with an
    // injected runner (no container, no Docker). Ordering is carried by channels and by the flock
    // itself. The ONE wall-clock wait is a NEGATIVE one (`WHILE_HELD_GRACE`): it can only ever weaken
    // the assertion — a run that IS serialized can never deliver the event it waits for, so this wait
    // cannot produce a false failure. Every positive wait uses a generous deadline and fails explicitly.

    /// Generous bound for every POSITIVE wait: exceeded ⇒ the test fails loudly instead of hanging.
    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
    /// Bound for the single NEGATIVE wait (see above).
    const WHILE_HELD_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

    struct ConcurrentVerify {
        entered: std::sync::mpsc::Receiver<()>,
        release: std::sync::mpsc::Sender<()>,
        done: std::sync::mpsc::Receiver<()>,
        handle: std::thread::JoinHandle<VerifyOutcome>,
    }

    /// Spawn one `run_verify_serialized` whose single verify command signals `entered`, then parks until
    /// `release` fires (`park=true`) or returns at once (`park=false`), appending `<label> enter` /
    /// `<label> exit` to the shared log. `done` fires when the whole verify returns (guard dropped).
    fn spawn_verify(
        lock_dir: &std::path::Path,
        cache_vol: &str,
        label: &str,
        park: bool,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> ConcurrentVerify {
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let (done_tx, done) = std::sync::mpsc::channel();
        let lock_dir = lock_dir.to_path_buf();
        let cache_vol = cache_vol.to_string();
        let label = label.to_string();
        let handle = std::thread::spawn(move || {
            let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
                log.lock().unwrap().push(format!("{label} enter"));
                entered_tx.send(()).unwrap();
                if park {
                    release_rx.recv().expect("release channel");
                }
                log.lock().unwrap().push(format!("{label} exit"));
                Ok((0, "ok".into()))
            };
            let outcome = run_verify_serialized(
                &cfg(),
                Some(&profile(&[("test", true)], None)),
                &bridge_core::SessionCwd::parse("/repo/clone").unwrap(),
                &cache_vol,
                &runner,
                4096,
                &bridge_core::attempt_activity::NoopAttemptRecorder,
                &lock_dir,
            );
            done_tx.send(()).unwrap();
            outcome
        });
        ConcurrentVerify {
            entered,
            release,
            done,
            handle,
        }
    }

    /// FAIL-FIRST for the S5 defect: with `run_verify_recorded` (no lock) the second verify runs its
    /// container while the first is still inside its own, so the log interleaves and the ordering
    /// assertion fails. The lock makes the two container runs strictly sequential.
    #[test]
    fn same_cache_volume_verify_runs_never_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let vol = "a2a-verify-cache-000000000000000a";
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let first = spawn_verify(dir.path(), vol, "A", true, log.clone());
        first
            .entered
            .recv_timeout(DEADLINE)
            .expect("the first verify must reach its container run");

        // Exact: the guard is held ACROSS the container run, not merely taken and dropped around it.
        assert!(
            bridge_core::liveness::acquire_persistent_lock_in(
                dir.path(),
                &cache_volume_lock_id(vol)
            )
            .is_err(),
            "the cache-volume lock must stay held while the verify container runs"
        );

        let second = spawn_verify(dir.path(), vol, "B", false, log.clone());
        assert!(
            second.done.recv_timeout(WHILE_HELD_GRACE).is_err(),
            "a same-volume verify must not complete while another holds the cache volume"
        );

        first.release.send(()).unwrap();
        second
            .done
            .recv_timeout(DEADLINE)
            .expect("the queued verify must run once the holder releases");
        let a = first.handle.join().unwrap();
        let b = second.handle.join().unwrap();
        assert!(matches!(a, VerifyOutcome::Ran(_)) && matches!(b, VerifyOutcome::Ran(_)));

        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["A enter", "A exit", "B enter", "B exit"],
            "same-volume verify container runs must be strictly sequential"
        );
    }

    /// The lock must not over-serialize: different repos key different volumes and must run in parallel.
    #[test]
    fn different_cache_volumes_do_not_serialize() {
        let dir = tempfile::tempdir().unwrap();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let held = spawn_verify(
            dir.path(),
            "a2a-verify-cache-000000000000000b",
            "A",
            true,
            log.clone(),
        );
        held.entered
            .recv_timeout(DEADLINE)
            .expect("the first verify must reach its container run");

        let other = spawn_verify(
            dir.path(),
            "a2a-verify-cache-000000000000000c",
            "B",
            false,
            log.clone(),
        );
        other
            .done
            .recv_timeout(DEADLINE)
            .expect("a DIFFERENT cache volume must not wait on this one");

        held.release.send(()).unwrap();
        held.done.recv_timeout(DEADLINE).expect("holder completes");
        held.handle.join().unwrap();
        other.handle.join().unwrap();
        let log = log.lock().unwrap();
        assert_eq!(
            &log[..3],
            ["A enter", "B enter", "B exit"],
            "the second volume ran INSIDE the first's container run: {log:?}"
        );
    }

    /// A lock namespace that cannot be created must not strand the run: it degrades to an unserialized
    /// verify (loudly, on stderr) rather than refusing to verify at all.
    #[test]
    fn an_unusable_lock_namespace_still_runs_the_verify() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        let calls = std::cell::Cell::new(0_u32);
        let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
            calls.set(calls.get() + 1);
            Ok((0, "ok".into()))
        };
        let outcome = run_verify_serialized(
            &cfg(),
            Some(&profile(&[("test", true)], None)),
            &bridge_core::SessionCwd::parse("/repo/clone").unwrap(),
            "a2a-verify-cache-000000000000000d",
            &runner,
            4096,
            &bridge_core::attempt_activity::NoopAttemptRecorder,
            &blocker.join("locks"), // create_dir_all under a regular file always fails
        );
        assert!(matches!(outcome, VerifyOutcome::Ran(_)));
        assert_eq!(calls.get(), 1, "the verify still ran");
    }

    #[test]
    fn a_none_profile_takes_no_lock_and_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let namespace = dir.path().join("locks");
        let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
            panic!("runner must NOT be called when profile is None")
        };
        let outcome = run_verify_serialized(
            &cfg(),
            None,
            &bridge_core::SessionCwd::parse("/repo/clone").unwrap(),
            "a2a-verify-cache-000000000000000e",
            &runner,
            4096,
            &bridge_core::attempt_activity::NoopAttemptRecorder,
            &namespace,
        );
        assert!(matches!(outcome, VerifyOutcome::Skipped { .. }));
        assert!(
            !namespace.exists(),
            "a skipped verify must not even create the lock namespace"
        );
    }

    #[test]
    fn the_cache_volume_lock_id_cannot_collide_with_a_run_id() {
        let id = cache_volume_lock_id("a2a-verify-cache-0123456789abcdef");
        assert_eq!(id, "verify-a2a-verify-cache-0123456789abcdef");
        assert!(!crate::implement::task_id(4242, "abcd1234").starts_with(CACHE_VOLUME_LOCK_PREFIX));
    }

    #[test]
    fn the_wait_note_names_the_contended_cache_volume() {
        let note = cache_volume_wait_note("a2a-verify-cache-0123456789abcdef");
        assert!(note.contains("waiting for a concurrent verify of the same repository"));
        assert!(note.contains("a2a-verify-cache-0123456789abcdef"));
    }

    #[test]
    fn the_lock_dir_is_the_implement_root_namespace_else_the_lease_dir() {
        assert_eq!(
            cache_volume_lock_dir(std::path::Path::new(
                "/Users/w/code/.a2a-implement/impl-1-abcd"
            )),
            std::path::Path::new("/Users/w/code/.a2a-implement/.operation-locks"),
            "an implement run locks beside its own operation lock"
        );
        assert_eq!(
            cache_volume_lock_dir(std::path::Path::new("/Users/w/code/some-checkout")),
            bridge_core::liveness::lease_dir(),
            "a non-implement verify falls back to the host lease dir"
        );
    }

    #[test]
    fn acquire_cache_volume_lock_refuses_a_name_that_is_not_a_volume() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "..", "a/b", "-leading", "a b"] {
            let kind = acquire_cache_volume_lock(dir.path(), bad)
                .map(|_| ())
                .expect_err("must refuse a non-volume id")
                .kind();
            assert_eq!(kind, std::io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "a refused name must not create any lock file"
        );
        drop(acquire_cache_volume_lock(dir.path(), "a2a-verify-cache-0123456789abcdef").unwrap());
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
