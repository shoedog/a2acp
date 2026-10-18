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
    pub cache: Option<String>,
    pub results: Vec<VerifyResult>,
    pub passed: bool,
}

/// PURE. The verdict passes iff every GATE command succeeded (non-gate commands are reported only).
pub fn aggregate(results: Vec<VerifyResult>) -> VerifyVerdict {
    let passed = results.iter().all(|r| !r.gate || r.ok);
    VerifyVerdict {
        results,
        passed,
        cache: None,
    }
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
/// Non-gate failures are reported in the hand-off but never re-prompted. Infrastructure failures keep
/// their details even when configured commands were not reached; they do not imply code-repair work.
pub fn failure_digest(v: &VerifyVerdict, max_bytes: usize) -> String {
    let failed: Vec<&VerifyResult> = v
        .results
        .iter()
        .filter(|r| {
            r.gate
                && !r.ok
                && (matches!(r.reach, VerifyReach::Reached { .. }) || r.failure_class.is_some())
        })
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
        } else {
            s.push_str(" (not reached");
            if let Some(class) = r.failure_class {
                s.push_str(&format!("; {}", failure_class_label(class)));
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
    let mut marks: Vec<String> = v.results.iter().map(result_mark).collect();
    marks.extend(v.cache.clone());
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

/// All implement roots share the OS-account verify-cache namespace. Cache locks are coordination state;
/// callers must never unlink them, including after an interrupted verify.
pub fn cache_volume_lock_dir(_clone: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    bridge_core::liveness::verify_cache_lock_dir()
}

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

// ─── F-1 item 2: the warm-cache lock (the S5 defect one volume over) ──────────────────────────
//
// `warm_lsp_deps_step` (bin main.rs) writes the per-repo warm LSP dep volume with NO lock — the exact
// same "shared volume, unserialized container run" shape S5 closed for verify, on a DIFFERENT cache
// volume (`[[languages]].warm_cache`, not `[verify].cache`). This section mirrors S5's lock verbatim —
// same wait-not-fail contract and lock primitives, with the existing lease namespace and
// contended-note + degradation-arm semantics — under a DISTINCT `warm-` prefix. That prefix is ordinary
// NAMESPACE SEPARATION for two independent, legitimately-concurrent lock families (a verify run and a
// warm-deps fetch are expected to proceed at the same time when they hold DIFFERENT volumes) — it is
// NOT a safety mechanism for a shared volume. If `warm_cache` and `[verify].cache` ever produced the
// SAME volume name, these two non-communicating locks would NOT serialize a concurrent verify + warm-
// deps fetch against it — that IS the corruption case, which is exactly why config.rs's F-1 item 3
// rejects the alias at parse time. The actual guard against that corruption is the config-level
// rejection, not this prefix.

/// Distinct from [`CACHE_VOLUME_LOCK_PREFIX`] ("verify-"): a warm-cache lock id can never collide with a
/// verify-cache lock id even for the same volume name, and (like `CACHE_VOLUME_LOCK_PREFIX`) can never
/// collide with the run-operation-lock namespace ("impl-<pid>-<nonce>").
pub const WARM_CACHE_LOCK_PREFIX: &str = "warm-";

/// PURE. The lock id serializing warm-deps fetches that share one cache volume (file: `warm-<volume>.lock`).
pub fn warm_cache_lock_id(cache_vol: &str) -> String {
    format!("{WARM_CACHE_LOCK_PREFIX}{cache_vol}")
}

/// PURE. The operator note printed once when a warm-deps fetch is about to wait for another fetch of the
/// same repository. Pure so its content is asserted by a test rather than by scraping stderr.
pub fn warm_cache_wait_note(cache_vol: &str) -> String {
    format!(
        "[implement] lsp warm-deps: waiting for a concurrent warm-deps fetch of the same repository \
         (cache volume {cache_vol}) to finish..."
    )
}

/// Take the exclusive per-cache-volume warm-deps lock, WAITING (not failing) for any concurrent
/// same-volume fetch — mirrors [`acquire_cache_volume_lock`]'s wait-not-fail contract (a warm fetch
/// legitimately runs for a while, so a `WouldBlock` refusal would strand real work), same validation,
/// under the distinct `warm-` namespace.
pub fn acquire_warm_cache_lock(
    lock_dir: &std::path::Path,
    cache_vol: &str,
) -> std::io::Result<bridge_core::liveness::PersistentLockGuard> {
    if !is_volume_name(cache_vol) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("cache volume name {cache_vol:?} is not a usable lock id"),
        ));
    }
    let note = || eprintln!("{}", warm_cache_wait_note(cache_vol));
    bridge_core::liveness::acquire_persistent_lock_blocking_in(
        lock_dir,
        &warm_cache_lock_id(cache_vol),
        &note,
    )
}

/// Run ONE warm-deps fetch command (the injected `runner`, mirroring [`Runner`]) under the exclusive
/// per-volume warm-cache lock, WAITING for a concurrent same-volume fetch rather than corrupting the
/// shared cache. A lock that cannot be taken (unwritable namespace, no working `flock`, a non-volume-
/// shaped name) currently degrades to an unserialized warm fetch. Verify cache access instead fails
/// closed; warm-cache policy is outside this maintenance authority.
pub fn run_warm_fetch_serialized(
    cache_vol: &str,
    runner: &Runner,
    lock_dir: &std::path::Path,
    program: &str,
    argv: &[String],
) -> std::io::Result<(i32, String)> {
    let _cache_volume = match acquire_warm_cache_lock(lock_dir, cache_vol) {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!(
                "[implement] lsp warm-deps: could not take the cache-volume lock in {} ({e}) — running \
                 UNSERIALIZED; a concurrent warm-deps fetch of the same repository could corrupt the \
                 shared {cache_vol} cache",
                lock_dir.display()
            );
            None
        }
    };
    runner(program, argv)
}

/// A command runner: given `(program, argv)`, run it and return `(exit_code, combined_output)`. The real
/// impl spawns Docker; tests inject a stub. The exit code is the CONTAINER's — unforgeable by in-container
/// agent code.
pub type Runner<'a> = dyn Fn(&str, &[String]) -> std::io::Result<(i32, String)> + 'a;

/// Run every configured command as its own container (sharing the per-repo cache volume), reading each
/// container's exit code. Every configured population is reached even after an earlier command failure,
/// so one repair round receives the complete bounded defect set. Pure given an injected `runner`.
/// Returns `VerifyOutcome::Skipped` when `profile` is `None` (`--lang none`) without spawning any container.
///
/// `run`/`owner` (F-1 item 1) name + label EVERY spawned container exactly like implement's other managed
/// containers (`a2a_name` + `RunHandle::labels`), so the ADR-0021 label reaper, `containers list|reap`,
/// and (via the STABLE `owner` — see `verify_sweep_target` in bin main.rs) the NEXT process's
/// `recover_orphans` can all find a verify container. `recover_orphans`, not `RunEndGuard`, is the
/// SIGKILL-covering path: `RunEndGuard` is a `Drop` guard that runs on clean exit or a panic unwind, never
/// under SIGKILL. Each command's ordinal is the name's tail (1-based) so sequential commands never
/// collide on a container name.
#[allow(clippy::too_many_arguments)] // 2 more than before F-1 item 1: the naming/labeling identity
pub fn run_verify_recorded(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
    recorder: &dyn bridge_core::attempt_activity::AttemptRecorder,
    run: &bridge_core::run_identity::RunHandle,
    owner: &str,
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
        let name =
            bridge_core::sandbox::a2a_name("verify", owner, &run.instance_id, &ordinal.to_string());
        let labels = run
            .labels(
                "verify",
                "oneshot",
                &c.name,
                owner,
                Some(clone.as_str()),
                Some(clone.as_str()),
            )
            .to_arg_pairs();
        let (prog, argv) = bridge_core::sandbox::compose_verify(
            cfg.runtime.as_deref(),
            image,
            &cfg.egress,
            clone,
            &binding,
            &c.cmd,
            &name,
            &labels,
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

/// Classify the entire Rust run before any container work. Only visible command text is covered;
/// a wrapper that changes its target internally is outside the isolation guarantee.
pub fn rust_cache_mode(commands: &[bridge_core::profile::VerifyCommand]) -> Result<String, String> {
    let mut isolated = None;
    for command in commands {
        // Retain only the shell words before an unquoted bare `--`. Quotes and escapes are
        // conservative non-plain syntax, but still parsed here to locate that boundary correctly.
        let mut words = Vec::new();
        let mut word = String::new();
        let mut quote = None;
        let mut escaped = false;
        let mut quoted = false;
        let mut syntax = false;
        for ch in command.cmd.chars().chain(std::iter::once(' ')) {
            if escaped {
                word.push(ch);
                escaped = false;
            } else if ch == '\\' && quote != Some('\'') {
                escaped = true;
                syntax = true;
                quoted = true;
            } else if quote == Some(ch) {
                quote = None;
            } else if quote.is_some() {
                word.push(ch);
            } else if ch == '\'' || ch == '"' {
                quote = Some(ch);
                quoted = true;
                syntax = true;
            } else if ch.is_whitespace() {
                if word == "--" && !quoted {
                    break;
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                syntax |= ch == '\n' || ch == '\r';
                quoted = false;
            } else {
                syntax |= ";&|$`><(){}*?[".contains(ch);
                word.push(ch);
            }
        }
        let option = |w: &str, flag: &str| w == flag || w.starts_with(&format!("{flag}="));
        let visible = words.join(" ");
        let target_override = (visible.contains("--config") && visible.contains("target-dir"))
            || words
                .iter()
                .any(|w| w.contains("--target-dir") || w.contains("CARGO_TARGET_DIR="))
            || words.iter().enumerate().any(|(i, w)| {
                option(w, "--config")
                    && (w.contains("target-dir")
                        || words.get(i + 1).is_some_and(|v| v.contains("target-dir")))
            });
        if target_override {
            return Err(format!("verify command {:?} ({}) refused: visible target override bypasses bridge cache control", command.name, command.cmd));
        }
        let plain = words.first().is_some_and(|w| w == "cargo")
            && !syntax
            && quote.is_none()
            && !escaped
            && !words.get(1).is_some_and(|w| w.starts_with('+'))
            && !words.iter().any(|w| {
                option(w, "--config") || option(w, "--manifest-path") || w.starts_with("-Z")
            });
        if !plain && isolated.is_none() {
            isolated = Some(format!(
                "cache: isolated (non-plain command: {})",
                command.name
            ));
        }
    }
    Ok(isolated.unwrap_or_else(|| "cache: shared (plain commands)".into()))
}

/// Retained-size limit for the complete per-repository volume, including Cargo home and legacy
/// targets. This is deliberately not a peak-build quota.
pub const VERIFY_CACHE_LIMIT_BYTES: u64 = 20 * 1024 * 1024 * 1024;

fn cache_failure_result(message: String) -> Vec<VerifyResult> {
    vec![VerifyResult {
        name: "cache-safety".into(),
        gate: true,
        ok: false,
        reach: VerifyReach::NotReached,
        failure_class: Some(VerifyFailureClass::Runner),
        output: message,
    }]
}

pub fn cache_failure(message: String) -> VerifyOutcome {
    VerifyOutcome::Ran(aggregate(cache_failure_result(message)))
}

fn checked_run(runner: &Runner, program: &str, argv: &[String]) -> std::io::Result<String> {
    let (status, output) = runner(program, argv)?;
    if status != 0 {
        return Err(std::io::Error::other(format!(
            "{program} exited {status}: {output}"
        )));
    }
    Ok(output)
}

/// The caller holds the cache lock across this probe and every cache operation. Query ALL running
/// consumers, including unmanaged containers and orphaned verifies whose bridge process died.
pub fn cache_consumers_idle(runtime: &str, volume: &str, runner: &Runner) -> std::io::Result<()> {
    if !is_volume_name(volume) {
        return Err(std::io::Error::other("invalid cache volume"));
    }
    let out = checked_run(
        runner,
        runtime,
        &[
            "ps".into(),
            "--filter".into(),
            format!("volume={volume}"),
            "--format".into(),
            "{{.ID}}".into(),
        ],
    )?;
    if !out.trim().is_empty() {
        return Err(std::io::Error::other(format!(
            "cache {volume}: consumers present; maintenance/verify deferred"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheMeasurement {
    pub before: u64,
    pub after: u64,
    pub proposed: u64,
    #[serde(default)]
    pub logical_bytes: Option<u64>,
    #[serde(default)]
    pub disk_bytes: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Measurement and trimming use the same volume root, regardless of the profile's mount path or
/// CARGO_TARGET_DIR. Coarse oldest-first top-level eviction includes historical lane directories;
/// a large target can be removed whole. Cargo home is retained preferentially. No symlink is followed.
/// Measurement failures are I/O errors. Partial removals retain measured before/after bytes with an
/// explicit error in the result; callers must inspect it before reporting success. Accounting uses
/// max(logical, allocated) per entry and includes allocated directory blocks.
pub const CACHE_MAINTENANCE: &str = r#"
import os, sys, stat, json, shutil
root, limit, preview = sys.argv[1], int(sys.argv[2]), sys.argv[3] == 'preview'
def measure(path):
    m = os.lstat(path)
    disk = m.st_blocks * 512
    if stat.S_ISDIR(m.st_mode):
        children = [measure(os.path.join(path, x)) for x in os.listdir(path)]
        return (sum(x[0] for x in children), disk + sum(x[1] for x in children), disk + sum(x[2] for x in children))
    if stat.S_ISREG(m.st_mode) or stat.S_ISLNK(m.st_mode):
        return (m.st_size, disk, max(m.st_size, disk))
    raise RuntimeError('unsupported cache entry: ' + path)
entries = [(os.path.join(root, x), os.lstat(os.path.join(root, x))) for x in os.listdir(root)]
measured = [(p, m, measure(p)[2]) for p, m in entries]
logical, disk, before = measure(root)
remaining = before
proposed = 0
error = None
for p, m, n in sorted(measured, key=lambda e: (not os.path.basename(e[0]).startswith('.a2a-isolated-'), os.path.basename(e[0]) == 'cargo', e[1].st_mtime_ns, e[0])):
    if remaining <= limit and not os.path.basename(p).startswith('.a2a-isolated-'):
        continue
    proposed += n
    remaining -= n
    if not preview:
        try:
            if stat.S_ISDIR(m.st_mode):
                shutil.rmtree(p)
            else:
                os.unlink(p)
        except OSError as e:
            error = str(e)
            break
after = measure(root)[2]
if not preview and after > limit:
    error = (error or '') + '; deferred excess bytes=' + str(after - limit)
print(json.dumps(dict(before=before, after=after, proposed=proposed, logical_bytes=logical, disk_bytes=disk, error=error)))
"#;

/// Caller must own the cache lock and establish idle consumers, including for preview. `preview`
/// mounts the payload read-only. The helper image must provide Python 3; a missing tool fails closed.
pub fn maintain_cache(
    cfg: &VerifyConfig,
    volume: &str,
    runner: &Runner,
    preview: bool,
) -> std::io::Result<CacheMeasurement> {
    if !is_volume_name(volume) {
        return Err(std::io::Error::other("invalid cache volume"));
    }
    let argv = vec![
        "run".into(),
        "--rm".into(),
        "--network=none".into(),
        "--mount".into(),
        format!(
            "type=volume,src={volume},dst=/cache,volume-nocopy{}",
            if preview { ",readonly" } else { "" }
        ),
        "--entrypoint".into(),
        "python3".into(),
        cfg.image.clone(),
        "-c".into(),
        CACHE_MAINTENANCE.into(),
        "/cache".into(),
        VERIFY_CACHE_LIMIT_BYTES.to_string(),
        if preview {
            "preview".into()
        } else {
            "apply".into()
        },
    ];
    let out =
        checked_run(runner, cfg.runtime.as_deref().unwrap_or("docker"), &argv).map_err(|e| {
            std::io::Error::other(format!(
                "maintenance image {} must provide Python 3: {e}",
                cfg.image
            ))
        })?;
    let mut measurement: CacheMeasurement = serde_json::from_str(out.trim())
        .map_err(|e| std::io::Error::other(format!("cache measurement unavailable: {e}")))?;
    if preview && measurement.before != measurement.after {
        return Err(std::io::Error::other("inconsistent cache measurement"));
    }
    if !preview && measurement.after > VERIFY_CACHE_LIMIT_BYTES && measurement.error.is_none() {
        measurement.error = Some(format!(
            "deferred excess bytes={}",
            measurement.after - VERIFY_CACHE_LIMIT_BYTES
        ));
    }
    Ok(measurement)
}

fn trim_cache(cfg: &VerifyConfig, volume: &str, runner: &Runner) -> std::io::Result<()> {
    let measurement = maintain_cache(cfg, volume, runner, false)?;
    match measurement.error {
        Some(e) => Err(std::io::Error::other(format!(
            "cache maintenance: {e}; retained bytes={}",
            measurement.after
        ))),
        None => Ok(()),
    }
}

/// Cargo supports relocated workspaces and trusts source mtimes, so every local/path package must be
/// cleaned before the first command, even for unchanged bases. Metadata covers non-member path deps
/// and build scripts as well as workspace members. Cargo itself owns artifact naming and deletion.
/// Registry/git dependencies are immutable and remain reusable. Only plain commands use this path.
pub const INVALIDATE_LOCAL: &str = r#"python3 - <<'A2A_INVALIDATE'
import json, os, pathlib, subprocess
manifests = {'Cargo.toml'} if pathlib.Path('Cargo.toml').is_file() else set()
if not manifests:
    raise RuntimeError('no Cargo manifest in supported verification scope')
for manifest in sorted(manifests):
    # Use the configured egress/offline policy: a cold verify Cargo home must be allowed to fetch
    # immutable sources. All features include optional non-member path dependencies before any build.
    resolved = subprocess.run(['cargo', 'metadata', '--format-version=1', '--locked', '--all-features', '--manifest-path', manifest], stdout=subprocess.PIPE)
    if resolved.returncode:
        raise SystemExit(resolved.returncode)
    metadata = json.loads(resolved.stdout)
    local = [p['id'] for p in metadata['packages'] if p['source'] is None]
    target = pathlib.Path(metadata['target_directory']).resolve()
    mount = pathlib.Path(os.environ.get('A2A_VERIFY_CACHE_ROOT', '/cache')).resolve()
    coverage = 'in-volume; included in retained-size bound' if target.is_relative_to(mount) else 'EXTERNAL; outside retained-size bound and cross-volume locking'
    print('verify cache: effective target=' + str(target) + '; ' + coverage, flush=True)
    roots = [(None, target)]
    if target.exists():
        for p in target.iterdir():
            if p.is_dir() and any((d / '.fingerprint').is_dir() for d in p.iterdir() if d.is_dir()):
                roots.append((p.name, p))
    for triple, root in roots:
        profiles = [p.name for p in root.iterdir() if (p / '.fingerprint').is_dir()] if root.exists() else []
        for profile in profiles:
            args = ['cargo', 'clean', '--offline', '--locked', '--manifest-path', manifest, '--profile', 'dev' if profile == 'debug' else profile]
            # Cleaning needs only the output directory, not the historical compilation settings.
            # Supply an inheritance solely for cleanup so a removed/renamed custom profile remains
            # cleanable without deleting immutable dependency artifacts or modifying the checkout.
            if profile not in ('debug', 'release'):
                args += ['--config', 'profile.' + json.dumps(profile) + '.inherits="dev"']
            for package in local:
                args += ['-p', package]
            if triple:
                args += ['--target', triple]
            if local:
                code = subprocess.call(args)
                if code:
                    raise SystemExit(code)
A2A_INVALIDATE"#;

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
/// Lock failure refuses shared-cache verification. Contention waits; an unavailable lock is never
/// evidence that no other verifier or maintenance operation is using the volume.
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
    run: &bridge_core::run_identity::RunHandle,
    owner: &str,
) -> VerifyOutcome {
    if profile.is_none() {
        // No profile ⇒ no container ⇒ nothing to serialize; never create a lock namespace for a skip.
        return run_verify_recorded(
            cfg, profile, clone, cache_vol, runner, max_bytes, recorder, run, owner,
        );
    }
    let profile = profile.expect("checked above");
    let cache_mode = if profile.id == "rust" {
        match rust_cache_mode(&profile.verify_commands) {
            Ok(mode) => Some(mode),
            Err(error) => return cache_failure(error),
        }
    } else {
        None
    };
    let fail = |message| {
        let mut verdict = aggregate(cache_failure_result(message));
        verdict.cache = cache_mode.clone();
        VerifyOutcome::Ran(verdict)
    };
    let _cache_volume = match acquire_cache_volume_lock(lock_dir, cache_vol) {
        Ok(guard) => guard,
        Err(e) => return fail(format!("cache lock: {e}")),
    };
    let isolated = cache_mode
        .as_ref()
        .is_some_and(|mode| mode.starts_with("cache: isolated"));
    let binding = profile.cache_binding(bridge_core::profile::CacheCtx::Verify, "", cache_vol);
    let mount = binding
        .mounts
        .first()
        .and_then(|m| m.split(':').nth(1))
        .unwrap_or("/cache");
    let target = if isolated {
        let id: String = run
            .instance_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("{mount}/.a2a-isolated-{id}")
    } else {
        format!("{mount}/target")
    };
    let bound_profile = if profile.id == "rust" {
        profile.with_verify_target_dir(target)
    } else {
        profile.clone()
    };
    let profile = &bound_profile;
    let mut effective = cfg.clone();
    effective.image = profile.image.as_deref().unwrap_or(&cfg.image).to_string();
    let cfg = &effective;
    let runtime = cfg.runtime.as_deref().unwrap_or("docker");
    if let Err(e) = cache_consumers_idle(runtime, cache_vol, runner) {
        return fail(e.to_string());
    }
    // Trim leftovers before use as well as after: SIGKILL can skip the latter boundary.
    if let Err(e) = trim_cache(cfg, cache_vol, runner) {
        return fail(e.to_string());
    }
    let invalidation_error = if profile.id == "rust" && !isolated {
        let binding = profile.cache_binding(bridge_core::profile::CacheCtx::Verify, "", cache_vol);
        let mount = binding
            .mounts
            .first()
            .and_then(|m| m.split(':').nth(1))
            .unwrap_or("/cache");
        let invalidate = format!(
            "A2A_VERIFY_CACHE_ROOT='{}' {INVALIDATE_LOCAL}",
            mount.replace('\'', "'\"'\"'")
        );
        let (prog, argv) = bridge_core::sandbox::compose_verify(
            cfg.runtime.as_deref(),
            profile.image.as_deref().unwrap_or(&cfg.image),
            &cfg.egress,
            clone,
            &binding,
            &invalidate,
            &bridge_core::sandbox::a2a_name("verify", owner, &run.instance_id, "invalidate"),
            &run.labels(
                "verify",
                "oneshot",
                "invalidate",
                owner,
                Some(clone.as_str()),
                Some(clone.as_str()),
            )
            .to_arg_pairs(),
        );
        match runner(&prog, &argv) {
            Ok((0, out)) => {
                eprintln!("{out}");
                None
            }
            Ok((status, out)) => Some(VerifyResult {
                name: "cache-preparation".into(),
                gate: true,
                ok: false,
                reach: VerifyReach::NotReached,
                // Cargo's source/lock errors remain repairable. Runtime/tool failures require the
                // operator and must not spend empty code-fix turns.
                failure_class: classify_verify_failure(status, &out, status != 101),
                output: truncate_output(&format!("local artifact invalidation: {out}"), max_bytes),
            }),
            Err(e) => cache_failure_result(format!("local artifact invalidation: {e}")).pop(),
        }
    } else {
        None
    };
    let mut outcome = match invalidation_error {
        Some(result) => VerifyOutcome::Ran(aggregate(vec![result])),
        None => run_verify_recorded(
            cfg,
            Some(profile),
            clone,
            cache_vol,
            runner,
            max_bytes,
            recorder,
            run,
            owner,
        ),
    };
    if let VerifyOutcome::Ran(verdict) = &mut outcome {
        verdict.cache = cache_mode;
    }
    // A failed command still reaches maintenance. A surviving container (e.g. a runner disconnect)
    // prevents it; the next use also refuses until that consumer has stopped.
    let maintenance = cache_consumers_idle(runtime, cache_vol, runner)
        .and_then(|()| trim_cache(cfg, cache_vol, runner));
    match maintenance {
        Ok(()) => outcome,
        Err(e) => match outcome {
            VerifyOutcome::Ran(mut verdict) => {
                verdict.results.extend(cache_failure_result(e.to_string()));
                let mut result = aggregate(verdict.results);
                result.cache = verdict.cache;
                VerifyOutcome::Ran(result)
            }
            _ => cache_failure(e.to_string()),
        },
    }
}

#[allow(clippy::too_many_arguments)] // 2 more than before F-1 item 1: the naming/labeling identity
pub fn run_verify(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
    run: &bridge_core::run_identity::RunHandle,
    owner: &str,
) -> VerifyOutcome {
    run_verify_recorded(
        cfg,
        profile,
        clone,
        cache_vol,
        runner,
        max_bytes,
        &bridge_core::attempt_activity::NoopAttemptRecorder,
        run,
        owner,
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
    fn rust_cache_classification_and_override_refusal_precede_container_work() {
        for cmd in [
            "cargo test",
            "cargo clippy -- -D warnings",
            "cargo test -p app",
            "cargo test -- --skip app*",
            "cargo test -- --skip process::tests::drop_group_kills_descendants",
        ] {
            let mut p = profile(&[("test", true)], None);
            p.verify_commands[0].cmd = cmd.into();
            assert_eq!(
                rust_cache_mode(&p.verify_commands).unwrap(),
                "cache: shared (plain commands)"
            );
        }
        for cmd in [
            "make test",
            "test.sh",
            "cargo test && cargo build",
            "VAR=value cargo test",
            "cargo build --config net.offline=true",
            "cargo -Zunstable-options test",
            "cargo test --manifest-path other/Cargo.toml",
            "cargo +stable test",
            "cargo test -p app*",
            "cargo test -p app?",
            "cargo test -p [ab]pp",
        ] {
            let mut p = profile(&[("test", true)], None);
            p.verify_commands[0].cmd = cmd.into();
            assert_eq!(
                rust_cache_mode(&p.verify_commands).unwrap(),
                "cache: isolated (non-plain command: test)"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        for cmd in [
            "cargo test --target-dir /else",
            "cargo test --target-dir=/else",
            "CARGO_TARGET_DIR=/else cargo test",
            "env CARGO_TARGET_DIR=/else cargo test",
            r#"cargo test --config 'build.target-dir="/else"'"#,
            "cargo test --config=build.target-dir=other",
            "sh -c 'cargo test --target-dir /else'",
            "sh -c 'cargo test --config build.target-dir=other'",
        ] {
            let mut p = profile(&[("wrapper", true), ("test", true)], None);
            p.verify_commands[0].cmd = "make test".into();
            p.verify_commands[1].cmd = cmd.into();
            let outcome = run_verify_serialized(
                &cfg(),
                Some(&p),
                &bridge_core::SessionCwd::parse("/repo").unwrap(),
                "cache",
                &|_, _| panic!("override must refuse before ANY container work"),
                1000,
                &bridge_core::attempt_activity::NoopAttemptRecorder,
                dir.path(),
                &test_run(),
                "owner",
            );
            let v = unwrap_ran(outcome);
            assert!(!v.passed);
            assert!(v.results[0].output.contains(cmd), "{v:?}");
            assert!(v.results[0].output.contains("visible target override"));
        }
    }

    #[test]
    fn toolchain_selected_cargo_runs_isolated_without_preparation() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = profile(&[("test", true)], None);
        p.verify_commands[0].cmd = "cargo +stable test".into();
        let calls = std::cell::RefCell::new(Vec::new());
        let runner = |_program: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            calls.borrow_mut().push(argv.to_vec());
            if argv.iter().any(|argument| argument == CACHE_MAINTENANCE) {
                return Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()));
            }
            assert!(
                !argv.last().is_some_and(|command| {
                    command.contains("cargo metadata") || command.contains("cargo clean")
                }),
                "toolchain-selected cargo must not run shared-cache preparation: {argv:?}"
            );
            Ok((0, String::new()))
        };
        let verdict = unwrap_ran(run_verify_serialized(
            &cfg(),
            Some(&p),
            &bridge_core::SessionCwd::parse("/repo").unwrap(),
            "cache",
            &runner,
            1000,
            &bridge_core::attempt_activity::NoopAttemptRecorder,
            dir.path(),
            &test_run(),
            "owner",
        ));
        assert!(verdict.passed, "{verdict:?}");
        assert_eq!(
            verdict.cache.as_deref(),
            Some("cache: isolated (non-plain command: test)")
        );
        assert!(calls.borrow().iter().any(|argv| argv
            .last()
            .is_some_and(|command| command.contains("cargo +stable test"))));
    }

    #[test]
    fn isolated_targets_are_fresh_swept_and_removed_even_on_command_failure() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(cache.join("cargo")).unwrap();
        std::fs::write(cache.join("cargo/keep"), "shared sources").unwrap();
        let wrapper = dir.path().join("check");
        std::fs::write(&wrapper, "#!/bin/sh\nset -eu\ncase \"$CARGO_TARGET_DIR\" in */.a2a-isolated-*) ;; *) exit 9;; esac\ntest -z \"$(ls -A \"$CARGO_TARGET_DIR\")\"\necho artifact > \"$CARGO_TARGET_DIR/artifact\"\n").unwrap();
        std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        for cmd in ["./check", "true && ./check", "./check && false"] {
            std::fs::create_dir_all(cache.join(".a2a-isolated-interrupted")).unwrap();
            std::fs::write(cache.join(".a2a-isolated-interrupted/stale"), "old").unwrap();
            let p = bridge_core::profile::LanguageProfile::from_parts(
                "rust".into(),
                "".into(),
                "warm".into(),
                "/cargo".into(),
                cache.display().to_string(),
                vec![],
                vec![],
                vec![],
                vec![
                    (
                        "CARGO_HOME".into(),
                        cache.join("cargo").display().to_string(),
                    ),
                    (
                        "CARGO_TARGET_DIR".into(),
                        cache.join("old-target").display().to_string(),
                    ),
                ],
                None,
                vec![bridge_core::profile::VerifyCommand {
                    name: "test".into(),
                    cmd: cmd.into(),
                    gate: true,
                }],
            );
            let runner = |_: &str, args: &[String]| -> std::io::Result<(i32, String)> {
                if args[0] == "ps" {
                    return Ok((0, String::new()));
                }
                let out = if args.iter().any(|a| a == CACHE_MAINTENANCE) {
                    std::process::Command::new("python3")
                        .args(["-c", CACHE_MAINTENANCE])
                        .arg(&cache)
                        .args(["99999999", "apply"])
                        .output()?
                } else {
                    assert!(
                        !cache.join(".a2a-isolated-interrupted").exists(),
                        "leftovers must be swept before execution"
                    );
                    assert!(!args.last().unwrap().contains(INVALIDATE_LOCAL));
                    std::process::Command::new("sh")
                        .args(["-c", args.last().unwrap()])
                        .output()?
                };
                Ok((
                    out.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out.stdout).to_string()
                        + &String::from_utf8_lossy(&out.stderr),
                ))
            };
            let v = unwrap_ran(run_verify_serialized(
                &cfg(),
                Some(&p),
                &bridge_core::SessionCwd::parse(&dir.path().display().to_string()).unwrap(),
                "cache",
                &runner,
                2000,
                &bridge_core::attempt_activity::NoopAttemptRecorder,
                &dir.path().join("locks"),
                &test_run(),
                "owner",
            ));
            assert_eq!(v.passed, !cmd.ends_with("false"), "{v:?}");
            assert!(verdict_line(&v).contains("cache: isolated (non-plain command: test)"));
            assert_eq!(
                std::fs::read_dir(&cache).unwrap().count(),
                1,
                "isolated targets must be removed after verification"
            );
            assert_eq!(
                std::fs::read_to_string(cache.join("cargo/keep")).unwrap(),
                "shared sources"
            );
        }
    }

    #[test]
    fn maintenance_preview_and_apply_disable_volume_copy_up() {
        for preview in [true, false] {
            let runner = |_: &str, args: &[String]| {
                let mount = args.windows(2).find(|pair| pair[0] == "--mount").unwrap()[1]
                    .split(',')
                    .collect::<Vec<_>>();
                assert!(mount.contains(&"volume-nocopy"), "{args:?}");
                assert_eq!(mount.contains(&"readonly"), preview);
                Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()))
            };
            maintain_cache(&cfg(), "cache", &runner, preview).unwrap();
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

    /// F-1 item 1: a stable `RunHandle` fixture for tests that assert on the composed verify
    /// container's name/labels (the `a2a.run` value the run-scoped reaper keys on).
    fn test_run() -> bridge_core::run_identity::RunHandle {
        bridge_core::run_identity::RunHandle {
            instance_id: "run-77".into(),
            host: "host-x".into(),
            lease: "/tmp/run-77.lock".into(),
            start: "1700000000".into(),
        }
    }

    #[test]
    fn run_verify_recorded_names_and_labels_each_container_for_the_run_scoped_reaper() {
        // F-1 item 1 (S5 follow-up): an unnamed/unlabeled verify container is invisible to the
        // ADR-0021 label reaper, `containers list|reap`, `RunEndGuard`'s END sweep (clean-exit/panic
        // path), and recover_orphans's crash recovery (the SIGKILL-covering path — RunEndGuard is
        // Drop-based and never runs under SIGKILL; recover_orphans keys on `a2a.owner`/`a2a.managed`,
        // asserted separately by the `verify_sweep_target`/`recovery_owners` tests in bin main.rs). This
        // test proves the `a2a.run` label RunEndGuard's `by_run_filter_argv` and `containers`' `--run`
        // filter key on is present on every container.
        let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
        let seen: std::cell::RefCell<Vec<Vec<String>>> = std::cell::RefCell::new(Vec::new());
        let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
            seen.borrow_mut().push(argv.to_vec());
            Ok((0, "ok".into()))
        };
        let run = test_run();
        let _ = run_verify_recorded(
            &cfg(),
            Some(&profile(&[("fmt", true), ("test", true)], None)),
            &clone,
            "cache-x",
            &runner,
            4096,
            &bridge_core::attempt_activity::NoopAttemptRecorder,
            &run,
            "owner9",
        );
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        // each command gets its OWN uniquely-named container (ordinal-suffixed) carrying owner+run.
        assert!(seen[0]
            .windows(2)
            .any(|w| w[0] == "--name" && w[1] == "a2a-verify-owner9-run-77-1"));
        assert!(seen[1]
            .windows(2)
            .any(|w| w[0] == "--name" && w[1] == "a2a-verify-owner9-run-77-2"));
        // the run label is on EVERY command's container — what RunEndGuard's by_run_filter_argv
        // (`label=a2a.run=<id>`) and the ADR-0021 label reaper both key on.
        for argv in seen.iter() {
            assert!(argv.windows(2).any(|w| w == ["--label", "a2a.run=run-77"]));
            assert!(argv.windows(2).any(|w| w == ["--label", "a2a.managed=1"]));
        }
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
            &test_run(),
            "owner9",
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
            &test_run(),
            "owner9",
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
            &test_run(),
            "owner9",
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
            &test_run(),
            "owner9",
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
            &test_run(),
            "owner9",
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
        let outcome = run_verify(
            &cfg(),
            None,
            &clone,
            "cache-x",
            &runner,
            4096,
            &test_run(),
            "owner9",
        );
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
                if _argv.first().is_some_and(|a| a == "ps") {
                    return Ok((0, String::new()));
                }
                if _argv.last().is_some_and(|a| a.contains(INVALIDATE_LOCAL)) {
                    return Ok((0, String::new()));
                }
                if _argv.iter().any(|a| a == CACHE_MAINTENANCE) {
                    return Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()));
                }
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
                &test_run(),
                "owner9",
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

    /// An unavailable lock namespace must refuse shared-cache execution.
    #[test]
    fn an_unusable_lock_namespace_refuses_the_verify() {
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
            &test_run(),
            "owner9",
        );
        assert!(matches!(outcome, VerifyOutcome::Ran(_)));
        assert_eq!(calls.get(), 0, "unsafe verification must not run");
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
            &test_run(),
            "owner9",
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

    // ---- F-1 item 2: the warm-cache lock (the S5 defect one volume over) --------------------
    //
    // `warm_lsp_deps_step` (bin main.rs) writes the per-repo warm LSP dep volume with NO lock — the
    // exact same "SHARED volume, unserialized container run" shape S5 fixed for verify, just on a
    // DIFFERENT cache volume. `run_warm_fetch_serialized` mirrors `run_verify_serialized`'s wait-not-fail
    // contract and retains the existing lease namespace/fallback rules, under a
    // DISTINCT `warm-` prefix — ordinary namespace separation between two independent lock families, NOT
    // a safety net for a shared volume: if `warm_cache` and `[verify].cache` ever named the SAME volume,
    // these non-communicating locks would fail to serialize a concurrent verify + warm-deps fetch against
    // it. The actual guard against that is config.rs's F-1 item 3 parse-time alias rejection.

    /// FAIL-FIRST for the warm-cache defect: with NO lock, two concurrent warm fetches against the same
    /// volume would run their containers interleaved, so the ordering assertion below would fail. The
    /// lock makes the two container runs strictly sequential — mirrors S5's
    /// `same_cache_volume_verify_runs_never_interleave` exactly, one call site over.
    #[test]
    fn same_warm_cache_volume_fetches_never_interleave() {
        struct ConcurrentFetch {
            entered: std::sync::mpsc::Receiver<()>,
            release: std::sync::mpsc::Sender<()>,
            done: std::sync::mpsc::Receiver<()>,
            handle: std::thread::JoinHandle<std::io::Result<(i32, String)>>,
        }
        fn spawn_fetch(
            lock_dir: &std::path::Path,
            cache_vol: &str,
            label: &str,
            park: bool,
            log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        ) -> ConcurrentFetch {
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
                let outcome =
                    run_warm_fetch_serialized(&cache_vol, &runner, &lock_dir, "docker", &[]);
                done_tx.send(()).unwrap();
                outcome
            });
            ConcurrentFetch {
                entered,
                release,
                done,
                handle,
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let vol = "a2a-impl-lsp-cache-000000000000000f";
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let first = spawn_fetch(dir.path(), vol, "A", true, log.clone());
        first
            .entered
            .recv_timeout(DEADLINE)
            .expect("the first fetch must reach its container run");

        // Exact: the guard is held ACROSS the container run, not merely taken and dropped around it.
        assert!(
            bridge_core::liveness::acquire_persistent_lock_in(dir.path(), &warm_cache_lock_id(vol))
                .is_err(),
            "the warm-cache lock must stay held while the fetch container runs"
        );

        let second = spawn_fetch(dir.path(), vol, "B", false, log.clone());
        assert!(
            second.done.recv_timeout(WHILE_HELD_GRACE).is_err(),
            "a same-volume warm fetch must not complete while another holds the cache volume"
        );

        first.release.send(()).unwrap();
        second
            .done
            .recv_timeout(DEADLINE)
            .expect("the queued fetch must run once the holder releases");
        let a = first.handle.join().unwrap();
        let b = second.handle.join().unwrap();
        assert!(a.is_ok() && b.is_ok());

        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["A enter", "A exit", "B enter", "B exit"],
            "same-volume warm fetch container runs must be strictly sequential"
        );
    }

    /// The warm lock and the verify lock live in SEPARATE namespaces (`warm-`/`verify-` prefixes): a held
    /// verify lock for a volume name does NOT block a warm-fetch lock request for the SAME name. This is
    /// namespace hygiene, NOT a corruption guard — if `warm_cache` and `[verify].cache` ever produced the
    /// SAME volume, these two locks would run right past each other while the verify container and the
    /// warm-fetch container both write the ONE shared Docker volume, unserialized. The actual guard
    /// against that is config.rs's F-1 item 3 parse-time alias rejection
    /// (`RegistryConfig::parse`, tested by `parse_rejects_a_warm_cache_that_aliases_verify_cache`), which
    /// keeps `warm_cache` and `[verify].cache` from ever naming the same volume in the first place.
    #[test]
    fn warm_and_verify_locks_use_separate_lock_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let vol = "a2a-shared-name-0000000000000010";
        let _verify_guard = acquire_cache_volume_lock(dir.path(), vol).unwrap();
        // A warm lock request for the SAME name succeeds even while the verify lock is held — proving the
        // namespaces are separate, NOT proving this is safe if the volume were actually shared.
        drop(acquire_warm_cache_lock(dir.path(), vol).unwrap());
    }

    #[test]
    fn the_warm_cache_lock_id_uses_a_distinct_prefix_from_verify() {
        let id = warm_cache_lock_id("a2a-impl-lsp-cache-0123456789abcdef");
        assert_eq!(id, "warm-a2a-impl-lsp-cache-0123456789abcdef");
        assert_ne!(
            warm_cache_lock_id("same-volume"),
            cache_volume_lock_id("same-volume")
        );
        assert!(!crate::implement::task_id(4242, "abcd1234").starts_with(WARM_CACHE_LOCK_PREFIX));
    }

    #[test]
    fn the_warm_wait_note_names_the_contended_cache_volume() {
        let note = warm_cache_wait_note("a2a-impl-lsp-cache-0123456789abcdef");
        assert!(note.contains("waiting for a concurrent warm-deps fetch of the same repository"));
        assert!(note.contains("a2a-impl-lsp-cache-0123456789abcdef"));
    }

    #[test]
    fn acquire_warm_cache_lock_refuses_a_name_that_is_not_a_volume() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "..", "a/b", "-leading", "a b"] {
            let kind = acquire_warm_cache_lock(dir.path(), bad)
                .map(|_| ())
                .expect_err("must refuse a non-volume id")
                .kind();
            assert_eq!(kind, std::io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "a refused name must not create any lock file"
        );
    }

    /// A lock namespace that cannot be created must not strand the fetch: it degrades to an unserialized
    /// run (loudly, on stderr) rather than refusing to fetch at all — mirrors
    /// `an_unusable_lock_namespace_still_runs_the_verify`.
    #[test]
    fn an_unusable_warm_lock_namespace_still_runs_the_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        let calls = std::cell::Cell::new(0_u32);
        let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
            calls.set(calls.get() + 1);
            Ok((0, "ok".into()))
        };
        let outcome = run_warm_fetch_serialized(
            "a2a-impl-lsp-cache-0000000000000011",
            &runner,
            &blocker.join("locks"), // create_dir_all under a regular file always fails
            "docker",
            &[],
        );
        assert!(outcome.is_ok());
        assert_eq!(calls.get(), 1, "the fetch still ran");
    }

    #[test]
    fn the_wait_note_names_the_contended_cache_volume() {
        let note = cache_volume_wait_note("a2a-verify-cache-0123456789abcdef");
        assert!(note.contains("waiting for a concurrent verify of the same repository"));
        assert!(note.contains("a2a-verify-cache-0123456789abcdef"));
    }

    #[test]
    fn the_lock_dir_is_shared_across_implement_roots() {
        assert_eq!(
            cache_volume_lock_dir(std::path::Path::new("/a/.a2a-implement/run")).unwrap(),
            cache_volume_lock_dir(std::path::Path::new("/b/.a2a-implement/run")).unwrap()
        );
        assert_eq!(
            cache_volume_lock_dir(std::path::Path::new("/a")).unwrap(),
            bridge_core::liveness::verify_cache_lock_dir().unwrap()
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
            &test_run(),
            "owner9",
        );
        let _ = run_verify(
            &cfg(),
            Some(&profile(&[("test", true)], None)),
            &clone,
            "cache-x",
            &runner,
            4096,
            &test_run(),
            "owner9",
        );

        let seen = seen.borrow();
        assert!(seen[0].iter().any(|a| a == "override:img"));
        assert!(seen[1].iter().any(|a| a == "img"));
    }
    /// FAIL-FIRST against 15f183f2 using the same run_verify_serialized API and injected runner:
    /// Cargo reports local Fresh then rustc E0433 with the shared target; an empty target succeeds.
    /// Cargo fixtures use path dependencies only. A registry-shaped sentinel verifies that package
    /// cleaning leaves unrelated compiled artifacts intact; registry selection is also injected below.
    #[test]
    fn stale_local_artifacts_are_invalidated_without_clearing_unrelated_artifacts() {
        use std::process::Command;
        for distinct in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let setup = r#"
import pathlib, subprocess, json, os, sys, shutil
r=pathlib.Path(sys.argv[1]); p=r/'a'
(p/'immutable/src').mkdir(parents=True)
(p/'immutable/Cargo.toml').write_text('[package]\nname="immutable"\nversion="1.0.0"\nedition="2021"\n')
(p/'immutable/src/lib.rs').write_text('pub fn value()->u32 { 7 }')
(p/'Cargo.toml').write_text('[workspace]\nmembers=["local", "app"]\nexclude=["immutable"]\nresolver="2"\n[profile.custom]\ninherits="release"\n')
for name in ['local','app']:
    (p/name/'src').mkdir(parents=True)
    (p/name/'Cargo.toml').write_text('[package]\nname="'+name+'"\nversion="0.1.0"\nedition="2021"\n'+ ('[dependencies]\nimmutable={path="../immutable"}\n' if name=='local' else '[dependencies]\nlocal={path="../local"}\n'))
(p/'local/src/lib.rs').write_text('pub fn old()->u32 {immutable::value()}')
(p/'app/src/main.rs').write_text('fn main() {println!("{}",local::new::works());}')
(p/'local/build.rs').write_text('fn main() {println!("cargo:rerun-if-changed=input"); let s=std::fs::read_to_string("input").unwrap(); println!("cargo:rustc-env=INPUT={}",s.trim());}')
(p/'local/input').write_text('A')
env=dict(os.environ,CARGO_TARGET_DIR=str(r/'cache/target'),CARGO_INCREMENTAL='0',CARGO_TERM_COLOR='never')
subprocess.run(['cargo','build','--offline','-p','local'],cwd=p,env=env,check=True)
subprocess.run(['cargo','build','--offline','-p','local','--profile','custom'],cwd=p,env=env,check=True)
if sys.argv[2] == 'distinct':
    shutil.copytree(p,r/'b'); p=r/'b'
(p/'local/src/lib.rs').write_text('pub mod new { pub fn works()->u32 { assert_eq!(env!("INPUT"),"B"); immutable::value() } }')
(p/'local/input').write_text('B')
for f in (p/'local').rglob('*'):
    if f.is_file(): os.utime(f,(1500000000,1500000000))
x=subprocess.run(['cargo','run','--offline','-p','app','-v'],cwd=p,env=env,capture_output=True,text=True)
assert x.returncode != 0 and 'could not find `new` in `local`' in x.stderr, x.stderr
assert 'Fresh local' in x.stderr, x.stderr
(r/'stale-evidence').write_text(x.stderr)
empty=dict(env,CARGO_TARGET_DIR=str(r/'empty'))
x=subprocess.run(['cargo','run','--offline','-p','app'],cwd=p,env=empty,capture_output=True,text=True)
assert x.returncode == 0 and x.stdout.strip() == '7', (x.stdout,x.stderr)
"#;
            let setup_out = Command::new("python3")
                .args(["-c", setup])
                .arg(temp.path())
                .arg(if distinct { "distinct" } else { "common" })
                .output()
                .unwrap();
            assert!(
                setup_out.status.success(),
                "{}",
                String::from_utf8_lossy(&setup_out.stderr)
            );
            let target = temp.path().join("cache/target");
            let immutable = target.join("debug/deps/libregistry_fixture-aabb.rlib");
            std::fs::write(&immutable, "compiled registry sentinel").unwrap();
            let before = std::fs::metadata(&immutable).unwrap().modified().unwrap();
            let root = temp.path().join(if distinct { "b" } else { "a" });
            let mut p = profile(
                &[
                    ("run --offline -p app -v", true),
                    ("run --offline -p app --profile custom -v", true),
                ],
                None,
            );
            // Map the whole volume to a disposable host directory; both commands stay shared.
            p = bridge_core::profile::LanguageProfile::from_parts(
                p.id,
                p.fetch_cmd,
                p.warm_cache_base,
                "/cargo".into(),
                temp.path().join("cache").display().to_string(),
                vec![],
                vec![],
                vec![],
                vec![
                    ("CARGO_TARGET_DIR".into(), target.display().to_string()),
                    ("CARGO_INCREMENTAL".into(), "0".into()),
                    ("CARGO_NET_OFFLINE".into(), "true".into()),
                ],
                None,
                p.verify_commands,
            );
            let output = std::cell::RefCell::new(String::new());
            let runner = |_prog: &str, args: &[String]| -> std::io::Result<(i32, String)> {
                if args.first().is_some_and(|a| a == "ps") {
                    return Ok((0, String::new()));
                }
                // On base there is no maintenance/invalidation; this branch is never reached.
                if args.iter().any(|a| a.contains("remaining = before")) {
                    return Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()));
                }
                // CI exports CARGO_TERM_COLOR=always; the assertions match plain Cargo status text.
                let out = Command::new("sh")
                    .args(["-c", args.last().unwrap()])
                    .env("CARGO_TERM_COLOR", "never")
                    .output()?;
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                output.borrow_mut().push_str(&text);
                Ok((out.status.code().unwrap_or(-1), text))
            };
            let result = run_verify_serialized(
                &cfg(),
                Some(&p),
                &bridge_core::SessionCwd::parse(&root.display().to_string()).unwrap(),
                "a2a-verify-cache-0123456789abcdef",
                &runner,
                16000,
                &bridge_core::attempt_activity::NoopAttemptRecorder,
                &temp.path().join("locks"),
                &test_run(),
                "owner9",
            );
            let verdict = unwrap_ran(result);
            assert!(verdict.passed, "{distinct}: {verdict:?}");
            assert!(verdict_line(&verdict).contains("cache: shared (plain commands)"));
            let output = output.borrow();
            assert_eq!(
                std::fs::read_to_string(&immutable).unwrap(),
                "compiled registry sentinel"
            );
            assert!(output.contains("Compiling local"), "{output}");
            assert!(output.lines().any(|line| line.trim() == "7"), "{output}");
            assert_eq!(
                before,
                std::fs::metadata(immutable).unwrap().modified().unwrap()
            );
        }
    }

    #[test]
    fn failed_verify_still_trims_and_unknown_consumers_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let maintenance = std::cell::Cell::new(0);
        let runner = |_p: &str, args: &[String]| -> std::io::Result<(i32, String)> {
            if args.first().is_some_and(|a| a == "ps") {
                return Ok((0, String::new()));
            }
            if args.iter().any(|a| a == CACHE_MAINTENANCE) {
                maintenance.set(maintenance.get() + 1);
                return Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()));
            }
            if args.last().is_some_and(|a| a.contains(INVALIDATE_LOCAL)) {
                return Ok((0, String::new()));
            }
            Ok((1, "command failed".into()))
        };
        let result = run_verify_serialized(
            &cfg(),
            Some(&profile(&[("test", true)], None)),
            &bridge_core::SessionCwd::parse("/repo").unwrap(),
            "cache-0123456789abcdef",
            &runner,
            1000,
            &bridge_core::attempt_activity::NoopAttemptRecorder,
            dir.path(),
            &test_run(),
            "owner9",
        );
        assert!(!unwrap_ran(result).passed);
        assert_eq!(maintenance.get(), 2);
        for response in [
            Ok((0, "running-id".into())),
            Ok((1, "unknown".into())),
            Err(std::io::Error::other("unavailable")),
        ] {
            let response = std::cell::RefCell::new(Some(response));
            let runner = |_: &str, _: &[String]| response.borrow_mut().take().unwrap();
            assert!(cache_consumers_idle("docker", "cache", &runner).is_err());
        }
    }

    #[test]
    fn cache_measurement_and_deletion_errors_are_not_success() {
        for output in [
            "not json",
            r#"{"before":1,"after":999999999999,"proposed":0}"#,
        ] {
            let runner = |_: &str, _: &[String]| Ok((0, output.into()));
            assert!(trim_cache(&cfg(), "cache", &runner).is_err());
        }
        let runner = |_: &str, _: &[String]| Ok((1, "permission denied".into()));
        assert!(trim_cache(&cfg(), "cache", &runner).is_err());
    }

    #[test]
    fn trim_bounds_legacy_and_custom_targets_without_touching_preview_payloads() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["legacy-lane", "custom-target", "cargo"] {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("payload"), [0_u8; 32]).unwrap();
        }
        // Symlink to unrelated storage is measured as a link and never followed.
        let protected = tempfile::tempdir().unwrap();
        std::fs::write(protected.path().join("keep"), "protected").unwrap();
        std::os::unix::fs::symlink(protected.path(), dir.path().join("link")).unwrap();
        let run = |mode: &str, limit: &str| {
            let out = std::process::Command::new("python3")
                .args(["-c", CACHE_MAINTENANCE])
                .arg(dir.path())
                .args([limit, mode])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            serde_json::from_slice::<CacheMeasurement>(&out.stdout).unwrap()
        };
        let below = run("apply", "100000");
        assert_eq!(below.before, below.after);
        assert_eq!(below.proposed, 0);
        // Derive the limit from the measured size: directory block accounting differs across
        // filesystems (APFS directories occupy no blocks; ext4/overlay directories take 4 KiB).
        let limit = (below.before - 64).to_string();
        let preview = run("preview", &limit);
        assert_eq!(preview.before, preview.after);
        assert!(preview.proposed >= 64);
        for name in ["legacy-lane", "custom-target", "cargo"] {
            assert_eq!(
                std::fs::read(dir.path().join(name).join("payload")).unwrap(),
                [0_u8; 32]
            );
        }
        let applied = run("apply", &limit);
        assert_eq!(applied.before, preview.before);
        assert!(applied.after <= below.before - 64);
        assert_eq!(
            std::fs::read_to_string(protected.path().join("keep")).unwrap(),
            "protected"
        );
    }
    #[test]
    fn invalidation_selects_all_path_packages_and_leaves_registry_and_git_packages() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("custom-target");
        for profile in ["debug", "custom", "triple/release"] {
            std::fs::create_dir_all(target.join(profile).join(".fingerprint")).unwrap();
        }
        let metadata = serde_json::json!({
            "target_directory": target,
            "packages": [
                {"id": "local@0.1.0", "source": null},
                {"id": "nonmember-path@0.1.0", "source": null},
                {"id": "registry-package@1.0.0", "source": "registry+fixture"},
                {"id": "git-package@1.0.0", "source": "git+fixture"}
            ]
        });
        std::fs::write(dir.path().join("Cargo.toml"), "fixture").unwrap();
        let cargo = dir.path().join("cargo");
        std::fs::write(
            &cargo,
            format!(
                r#"#!/usr/bin/env python3
import sys, json
if sys.argv[1] == 'metadata':
    assert '--offline' not in sys.argv, 'cold immutable source cache cannot resolve offline'
    assert '--all-features' in sys.argv and '--manifest-path' in sys.argv
    print({metadata:?})
else:
    with open({trace:?}, 'a') as f: f.write(json.dumps(sys.argv[1:]) + '\n')
"#,
                metadata = metadata.to_string(),
                trace = dir.path().join("trace").display().to_string()
            ),
        )
        .unwrap();
        std::fs::set_permissions(cargo, std::fs::Permissions::from_mode(0o700)).unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", INVALIDATE_LOCAL])
            .current_dir(dir.path())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("A2A_VERIFY_CACHE_ROOT", dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("in-volume"));
        let trace = std::fs::read_to_string(dir.path().join("trace")).unwrap();
        assert_eq!(trace.lines().count(), 3, "{trace}");
        for line in trace.lines() {
            assert!(line.contains("local@0.1.0") && line.contains("nonmember-path@0.1.0"));
            assert!(!line.contains("registry-package") && !line.contains("git-package"));
        }
        assert!(trace.contains("custom") && trace.contains("triple") && trace.contains("release"));
    }
    #[test]
    fn preparation_cleans_removed_profiles_and_optional_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let optional = dir.path().join("optional");
        for p in [&root, &optional] {
            std::fs::create_dir_all(p.join("src")).unwrap();
        }
        let manifest = "[package]\nname='cache_scope'\nversion='0.1.0'\nedition='2021'\n[dependencies]\noptional_local={path='../optional',optional=true}\n";
        std::fs::write(
            root.join("Cargo.toml"),
            format!("{manifest}[profile.retired]\ninherits='dev'\n"),
        )
        .unwrap();
        std::fs::write(
            optional.join("Cargo.toml"),
            "[package]\nname='optional_local'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        std::fs::write(optional.join("src/lib.rs"), "pub fn old() {}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn old() {}\n").unwrap();
        let target = dir.path().join("target");
        let cargo = |cwd: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new("cargo")
                .args(args)
                .current_dir(cwd)
                .env("CARGO_TARGET_DIR", &target)
                .env("CARGO_HOME", dir.path().join("cold-home"))
                .env("CARGO_NET_OFFLINE", "true")
                .env("CARGO_INCREMENTAL", "0")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        cargo(&root, &["build", "--offline", "--all-features"]);
        cargo(
            &root,
            &[
                "build",
                "--offline",
                "--all-features",
                "--profile",
                "retired",
            ],
        );
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(optional.join("src/lib.rs"), "pub fn added() {}\n").unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "#[cfg(feature=\"optional_local\")] pub fn use_new() { optional_local::added(); }\n",
        )
        .unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", INVALIDATE_LOCAL])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_HOME", dir.path().join("cold-home"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        for profile in ["debug", "retired"] {
            for entry in std::fs::read_dir(target.join(profile).join("deps")).unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                assert!(!name.ends_with(".rlib"), "local artifact survived: {name}");
            }
        }
        cargo(&root, &["build", "--offline", "--all-features"]);
    }

    #[test]
    fn serialized_helpers_honor_image_override_and_surface_infrastructure_details() {
        let dir = tempfile::tempdir().unwrap();
        for override_image in [None, Some("override:img")] {
            let p = profile(&[("test", true)], override_image);
            let expected = override_image.unwrap_or("img");
            let runner = |_: &str, args: &[String]| -> std::io::Result<(i32, String)> {
                if args.first().is_some_and(|a| a == "ps") {
                    return Ok((0, String::new()));
                }
                assert!(args.iter().any(|a| a == expected), "{args:?}");
                if override_image.is_some() {
                    assert!(!args.iter().any(|a| a == "img"));
                }
                if args.iter().any(|a| a == CACHE_MAINTENANCE) {
                    return Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()));
                }
                Ok((0, String::new()))
            };
            let result = run_verify_serialized(
                &cfg(),
                Some(&p),
                &bridge_core::SessionCwd::parse("/repo").unwrap(),
                "cache",
                &runner,
                1000,
                &bridge_core::attempt_activity::NoopAttemptRecorder,
                dir.path(),
                &test_run(),
                "owner",
            );
            assert!(unwrap_ran(result).passed);
        }
        let runner = |_: &str, args: &[String]| -> std::io::Result<(i32, String)> {
            if args.first().is_some_and(|a| a == "ps") {
                return Ok((0, String::new()));
            }
            if args.iter().any(|a| a == CACHE_MAINTENANCE) {
                return Ok((0, r#"{"before":0,"after":0,"proposed":0}"#.into()));
            }
            Ok((
                101,
                "Cargo.lock needs to be updated but --locked was passed".into(),
            ))
        };
        let failed = unwrap_ran(run_verify_serialized(
            &cfg(),
            Some(&profile(&[("test", true)], None)),
            &bridge_core::SessionCwd::parse("/repo").unwrap(),
            "cache",
            &runner,
            1000,
            &bridge_core::attempt_activity::NoopAttemptRecorder,
            dir.path(),
            &test_run(),
            "owner",
        ));
        assert_eq!(
            failed.results[0].failure_class,
            Some(VerifyFailureClass::LockSync)
        );
        assert!(failure_digest(&failed, 1000).contains("Cargo.lock needs to be updated"));
        let failed = unwrap_ran(cache_failure("operator lock unavailable".into()));
        assert!(failure_digest(&failed, 1000).contains("operator lock unavailable"));
        assert!(failure_digest(&failed, 1000).contains("not reached; runner"));
        let mut results = vec![r("build", true, true)];
        results.extend(cache_failure_result("post-verify trim failed".into()));
        assert!(failure_digest(&aggregate(results), 1000).contains("post-verify trim failed"));
    }
}
