# Slice 7b — E9 watchdog — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (or executing-plans).
> **Spec (BINDING, read incl. the `## v2 … FIX-1..12` section FIRST):** `docs/superpowers/specs/2026-06-20-slice-7b-
> watchdog.md`. **Analysis:** `…/2026-06-20-slice-7b-watchdog-ANALYSIS.md`. **Model roles:** codex-HIGH implements;
> controller (Opus) verifies + commits + live-gates; codex-xhigh reviews.

**Goal:** a per-turn E9 watchdog in `AcpBackend::prompt_inner` that fires the existing graceful cancel on idle-timeout
(silence after first output) or hard wall-clock, surfacing a DISTINCT `AgentTimedOut` → A2A `Failed`. Opt-in per
agent; all ACP turns; the DRIVER owns the terminal decision (a `select!` arm) so a natural completion is never
mislabeled.

**Architecture:** the SDK handler bumps a per-turn `TurnWatch.last_activity` atomic (non-blocking); a `'static`
watchdog task observes it + the wall-clock and `notify`s a `watchdog_fired` signal on timeout; the driver's existing
`tokio::select!` gains a `watchdog_fired` arm that runs the same bounded cancel its `done_sender.closed()` arm
already does and emits `AgentTimedOut`. The watch rides the routing-registry value (`TurnRoute`); `watch=None` (no
config) is byte-identical to today.

**Tech stack:** Rust (`bridge-acp`, `bridge-core`, `bridge-container`, `bin/a2a-bridge`), tokio (`select!`, `Notify`,
`oneshot`, `Instant`, `sleep_until`). TDD; fmt+clippy clean; controller runs the suite (the `_dyld_start` flake);
coverage floors per `.github/workflows/ci.yml`.

---

## v2 — dual plan-review fixes folded (BINDING; SUPERSEDES contradicting task text)
Dual plan-reviewed (codex-xhigh `fix-then-implement`; Opus `needs-rework` — same findings, all precise compile-break
corrections; the MECHANISM [driver-arm race-freedom, `'static` task, oneshot teardown, unmodeled coverage] was
CONFIRMED sound). Read PFIX-A..M FIRST.
- **PFIX-A (BLOCKER — both) — `WatchdogConfig` lives in `bridge-core::domain`, NOT `bridge-acp`.** `bridge-core`
  cannot depend on `bridge-acp` (the edge is `bridge-acp → bridge-core`); `AgentEntry` (`domain.rs:115`) carries it.
  → define `pub struct WatchdogConfig { idle_timeout: Duration, hard_wall_clock: Duration }` in
  `crates/bridge-core/src/domain.rs` (next to `SandboxConfig`, `:65`); `bridge-acp`'s `AcpConfig.watchdog:
  Option<bridge_core::domain::WatchdogConfig>` IMPORTS it. (Fixes the File-Structure + Data-model placement.)
- **PFIX-B (BLOCKER — both) — adding `AgentEntry.watchdog` breaks ~31 struct-literal sites** (`AgentEntry` has NO
  `#[derive(Default)]`). Task 2 MUST `grep -rn "AgentEntry {" crates bin` and add `watchdog: None` to EVERY literal:
  `executor.rs:614`, `registry.rs:493/550/595/641`, `domain.rs:349/379/407`, `session_manager.rs:1353`,
  `tests/workflow_producer.rs:39`, `e2e_registry.rs:228/566/625`, `tests/common/mod.rs:23`, `server.rs:4053/7088`,
  `workflow_sink.rs:767`, `route.rs:109`, `config.rs:997`, `catalog_probe.rs:169`, `main.rs:4159/4258/4261` (+ any
  the grep finds). The tree must be green at the T2 commit.
- **PFIX-C (BLOCKER — both) — `to_state` does NOT exist; the method is `disposition() -> A2aDisposition`
  (`error.rs:105`).** Task 1 test: `assert_eq!(BridgeError::AgentTimedOut.disposition(), A2aDisposition::SetState(
  A2aState::Failed));` + assert it is NOT `SetState(Canceled)`. The `_ => SetState(Failed)` default arm (`:121`)
  auto-covers it (don't add a `Canceled` arm; `CancelTimeout` is the only one).
- **PFIX-D (MAJOR — both) — the ONE exhaustive `BridgeError` match that breaks is `table_key` (`resilient.rs:154`,
  no `_`).** Add an `AgentTimedOut` arm there; ALSO add an `(BridgeError::AgentTimedOut, Death::Fatal)` row to the
  `classify_death_table_is_exhaustive` Vec (`resilient.rs:183`). `disposition`/`client_message`/`classify_death`
  have `_` defaults → auto-covered (Fatal + Failed, which is FIX-6).
- **PFIX-E (MAJOR — both) — the `RequestPermissionRequest` handler does NOT capture the routing registry** (only
  `policy_handler`, `acp_backend.rs:1027`). → before `.on_receive_request(` (`:1021`), `let updates_perm =
  Arc::clone(&updates);` (the same `updates` bound at `:954`), capture it, and bump via `req.session_id` BEFORE the
  `cx.spawn` policy offload.
- **PFIX-F (MAJOR — Opus #6) — the watchdog `select!` arm DISCARDS the inner cancel outcome.** The `done_sender
  .closed()` arm (`:1974`) PROPAGATES its inner `prompt_fut` result (`Ok(resp)→Done`). The watchdog arm must NOT:
  hoist ONLY the cancel-notify + inner grace `select!` (for liveness/escalation), then UNCONDITIONALLY
  `timed_out_local = true; Err(())` — even if the agent honors cancel and returns `Done{cancelled}` within grace
  (the KEYSTONE: honored-cancel-after-timeout = `AgentTimedOut`, not a user cancel). They are NOT the same consumer.
- **PFIX-G (MAJOR — codex #7) — the disabled path (`watchdog=None`) must compile + spawn no task.** Don't make the
  `select!` arm unconditional. Use a future that's `Pending` when disabled: e.g. `let wd_fired = watchdog.as_ref()
  .map(|_| Arc::new(Notify::new())); … _ = async { match &wd_fired { Some(n) => n.notified().await, None =>
  std::future::pending().await } } => { … }` — so `None` never fires + spawns no watchdog task (byte-identical).
- **PFIX-H (MAJOR — codex #8) — `tokio::time::sleep_until` takes `tokio::time::Instant`, not `std::time::Instant`.**
  `TurnWatch.turn_start` is `std::time::Instant` (for the handler's `elapsed()`). In the watchdog task, convert the
  computed `std::Instant` deadline via `tokio::time::Instant::from_std(deadline_std)` (or compute with tokio
  instants). State the conversion in the sketch.
- **PFIX-I (MAJOR — codex #6) — `ContainerRwConfig` has a TEST literal at `lib.rs:810` (`cfg_with_mount`)** besides
  the prod site — add `watchdog: None` there too (and any other `ContainerRwConfig { … }` literal the grep finds).
- **PFIX-J (MAJOR — both) — FIX-10 needs an EXECUTABLE doc step + FIX-11 needs a TEST.** Add a step editing an
  operator doc (e.g. `docs/containerized-agents*.md` or the multi-agent example TOML comment) warning that watchdog
  escalation SIGKILLs sibling turns on a shared-process ACP backend → recommend container/per-turn isolation
  (FIX-10). Add a Task-5 test where the fake agent emits ONLY unmodeled updates (thought chunks) periodically with
  `idle_timeout` < total < `hard_wall_clock` → completes, NOT `AgentTimedOut` (FIX-11), and the Task-6 live-gate
  includes it.
- **PFIX-K (MAJOR — codex #4) — Task-2 TOML test needs `[server]`** (existing config tests require it) or it fails
  for the wrong reason before watchdog parsing runs.
- **PFIX-L (MINOR — both) — pin the small details:** (a) `la_instant = turn_start + Duration::from_millis(la.
  saturating_sub(1))` (the `+turn_start`/`-1` round-trip). (b) imports: `use std::sync::atomic::{AtomicU64,
  Ordering::Relaxed}; use std::time::{Duration, Instant}; use tokio::time::sleep_until;`. (c) build the
  `Option<Arc<TurnWatch>>` BEFORE `map.insert`, then insert `TurnRoute{tx, watch}` atomically. (d) `escalate
  _terminate` is take-once-idempotent (`:1612`) → the watchdog + the external `cancel()` grace-watcher escalating
  concurrently is safe.
- **PFIX-M (MINOR — Opus #9) — `main.rs:252` uses `..AcpConfig::default()`** → a forgotten `watchdog: entry.
  watchdog.clone()` SILENTLY defaults `None` (ACP agents miss the watchdog with no compile error). Add a config→
  AcpConfig assertion test, OR a code-review note; the container path (`lib.rs:249` + `main.rs:441`) is
  compiler-enforced.

---

## File Structure
- `crates/bridge-core/src/error.rs` — `BridgeError::AgentTimedOut` + `to_state`→Failed; exhaustive-match updates.
- `bin/a2a-bridge/src/resilient.rs` — `classify_death`: `AgentTimedOut`→Fatal (the `_` default).
- `crates/bridge-acp/src/acp_backend.rs` — `WatchdogConfig` + `AcpConfig.watchdog`; `TurnRoute`/`TurnWatch`; the
  handler bump; the watchdog task + the driver `select!` arm + the `AgentTimedOut` terminal.
- `bin/a2a-bridge/src/config.rs` — `WatchdogToml` parse + `into_snapshot`.
- `crates/bridge-core/src/domain.rs` — `AgentEntry.watchdog` field.
- `bin/a2a-bridge/src/main.rs` — `AcpConfig.watchdog` build (2 sites: ACP + container).
- `crates/bridge-container/src/lib.rs` — `ContainerRwConfig.watchdog` field + forward.

---

## Task 1: `BridgeError::AgentTimedOut` + classifiers (Fatal, → A2A Failed)

**Files:** `crates/bridge-core/src/error.rs`; `bin/a2a-bridge/src/resilient.rs`. Tests: both.

- [ ] **Step 1: Failing tests.**
```rust
// error.rs
#[test]
fn agent_timed_out_maps_to_failed_not_canceled() {
    use super::*;
    assert!(matches!(BridgeError::AgentTimedOut.to_state(), /* SetState(Failed) */ _));
    // assert it is NOT Canceled (CancelTimeout is) — compare the produced state
}
// resilient.rs
#[test]
fn agent_timed_out_is_fatal_not_retried() {
    assert!(matches!(classify_death(&BridgeError::AgentTimedOut), Death::Fatal));
}
```
- [ ] **Step 2: Run → FAIL** (variant missing). `cargo test -p bridge-core agent_timed_out; cargo test -p a2a-bridge agent_timed_out_is_fatal`
- [ ] **Step 3: Implement.** Add `AgentTimedOut` to `BridgeError` (a unit variant; mirror an existing unit variant's
  attrs). In `to_state`, ensure it lands on the `_ => SetState(S::Failed)` default (do NOT add a `Canceled` arm).
  In `classify_death` (`resilient.rs:18`), ensure it lands on the `_ => Fatal` default (do NOT add to the transient
  arm). **Find every EXHAUSTIVE `match` on `BridgeError`** (`grep -rn "BridgeError::" --include=*.rs | …`; helpers/
  tests with no `_` arm) and add an `AgentTimedOut` arm so the workspace compiles.
- [ ] **Step 4: Run → PASS** (+ `cargo test --workspace --no-run`). fmt; clippy.
- [ ] **Step 5: Commit.** `git commit -am "feat(core): BridgeError::AgentTimedOut -> Failed (fatal) (s7b FIX-6)"`

---

## Task 2: `WatchdogConfig` + the full config ripple (per-agent TOML → AcpConfig + ContainerRw)

**Files:** `crates/bridge-acp/src/acp_backend.rs` (`WatchdogConfig` + `AcpConfig.watchdog`), `bin/a2a-bridge/src/
config.rs` (`WatchdogToml` + `into_snapshot`), `crates/bridge-core/src/domain.rs` (`AgentEntry.watchdog`),
`bin/a2a-bridge/src/main.rs` (build, 2 sites), `crates/bridge-container/src/lib.rs` (`ContainerRwConfig`). Tests:
`config.rs`.

- [ ] **Step 1: Failing test** — `[agents.watchdog]` parses into the snapshot; absent → None; non-positive rejected.
```rust
#[test]
fn watchdog_toml_parses_per_agent() {
    let toml = r#"
        default = "c"
        [[agents]]
        id = "c"
        cmd = "codex-acp"
        [agents.watchdog]
        idle_timeout_secs = 30
        hard_wall_clock_secs = 600
    "#;
    let snap = parse_and_snapshot(toml).unwrap();
    let wd = snap.agent("c").watchdog.as_ref().unwrap();
    assert_eq!(wd.idle_timeout_secs, 30);
    // an agent without [agents.watchdog] → None
    // idle_timeout_secs = 0 → a validation error
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** (mirror `[agents.sandbox]` end-to-end):
  - `acp_backend.rs`: `pub struct WatchdogConfig { pub idle_timeout: Duration, pub hard_wall_clock: Duration }`
    (Clone, Debug); `AcpConfig` gains `pub watchdog: Option<WatchdogConfig>`; `Default` → `None`.
  - `config.rs`: a `WatchdogToml { idle_timeout_secs: u64, hard_wall_clock_secs: u64 }` sub-table on the agent TOML
    struct; validate both `> 0` in `into_snapshot` (error otherwise); convert to `WatchdogConfig` (secs→Duration).
  - `domain.rs`: `AgentEntry` gains `pub watchdog: Option<WatchdogConfig>` (or the secs pair) — mirror how
    `sandbox` rides the `AgentEntry`.
  - `main.rs`: at the `AcpConfig { … }` build site (`:252`) set `watchdog: entry.watchdog.clone()`; same at the
    container build site (`:441`).
  - `bridge-container/lib.rs`: `ContainerRwConfig` gains `watchdog: Option<WatchdogConfig>`; forward it into the
    inner `AcpConfig` it builds (`:249`).
- [ ] **Step 4: Run → PASS** (+ existing config tests; `cargo test --workspace --no-run`). fmt; clippy.
- [ ] **Step 5: Commit.** `git commit -am "feat(config): per-agent [agents.watchdog] -> AcpConfig + ContainerRw (s7b FIX-7)"`

---

## Task 3: `TurnRoute` / `TurnWatch` — the routing-registry value-type change (byte-identical, watch=None)

**Files:** `crates/bridge-acp/src/acp_backend.rs`. Tests: existing acp turn tests (the None path must be unchanged).

- [ ] **Step 1: Failing/again test** — first confirm the existing acp turn tests pass, then make the type change
  KEEP them passing (the change is structural; the assertion is "still green").
- [ ] **Step 2: Implement** the value-type change ONLY (no watchdog logic yet):
  - `struct TurnRoute { tx: UpdateSender, watch: Option<Arc<TurnWatch>> }` and `struct TurnWatch { turn_start:
    std::time::Instant, last_activity_ms: std::sync::atomic::AtomicU64 }` (import `AtomicU64`).
  - Change `UpdateRegistry` value to `TurnRoute`. Update the driver INSERT (`:1934` → `TurnRoute { tx, watch: None
    }` for now), the driver REMOVE (`:2003`, unchanged key), and the handler GET (`:999` → `route.tx`).
- [ ] **Step 3: Run → PASS** (the existing acp turn/corpus/usage tests — byte-identical None path). `cargo test -p
  bridge-acp`; `cargo test --workspace --no-run`. fmt; clippy.
- [ ] **Step 4: Commit.** `git commit -am "refactor(acp): routing registry value = TurnRoute{tx,watch} (s7b FIX-2)"`

---

## Task 4: The activity tap — watch creation in `prompt_inner` + the handler bump

**Files:** `crates/bridge-acp/src/acp_backend.rs`. Tests: a unit test of the bump (+ the watch-created-when-config).

- [ ] **Step 1: Failing test** — a `bump_activity` helper advances `last_activity_ms` (testable in isolation), and
  `prompt_inner` creates the watch when `config.watchdog.is_some()`.
```rust
#[test]
fn bump_activity_advances_last_activity() {
    let w = TurnWatch { turn_start: Instant::now(), last_activity_ms: AtomicU64::new(0) };
    std::thread::sleep(Duration::from_millis(2));
    bump_activity(&w);  // the same call the handler makes
    assert!(w.last_activity_ms.load(Relaxed) >= 1); // elapsed_ms + 1, never 0 once bumped
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.**
  - `fn bump_activity(w: &TurnWatch) { w.last_activity_ms.store(w.turn_start.elapsed().as_millis() as u64 + 1,
    Relaxed) }` (FIX-4: `as u64`, `+1` sentinel).
  - In `prompt_inner`, when `self.config.watchdog.is_some()` (AFTER `ensure_session` + the registry insert), build
    `Arc<TurnWatch>{ turn_start: Instant::now(), last_activity_ms: 0 }` and store it in the turn's `TurnRoute.watch`
    (insert `TurnRoute { tx, watch: Some(w.clone()) }`).
  - In the handler (`:977`): at the TOP of the closure, BEFORE `let te = map_session_update…`, a short
    `if let Ok(map) = updates.lock() { if let Some(r) = map.get(&notif.session_id) { if let Some(w) = &r.watch {
    bump_activity(w) } } }`. Add the SAME bump in the `RequestPermissionRequest` handler (`:1021`). (No watchdog
    task yet → a hung turn still hangs; the tap is inert until Task 5.)
- [ ] **Step 4: Run → PASS** (+ existing acp tests unchanged). `cargo test -p bridge-acp`; `--no-run`. fmt; clippy.
- [ ] **Step 5: Commit.** `git commit -am "feat(acp): per-turn activity tap (handler bump) (s7b FIX-3/4)"`

---

## Task 5: The watchdog task + the driver `select!` arm + the `AgentTimedOut` terminal (the keystone)

**Files:** `crates/bridge-acp/src/acp_backend.rs`. Tests: a fake-backend turn — hung→AgentTimedOut, active→completes,
no-config→unchanged.

- [ ] **Step 1: Failing tests** (use the existing fake-ACP test harness):
```rust
#[tokio::test]
async fn watchdog_cancels_a_hung_turn_as_timed_out() {
    // a fake backend that ACCEPTS the prompt then NEVER responds + emits nothing;
    // AcpConfig.watchdog = { idle: 10s, hard_wall_clock: 50ms }; drive prompt → the stream's
    // terminal item is Err(AgentTimedOut) within ~the wall-clock.
}
#[tokio::test]
async fn watchdog_does_not_trip_an_active_turn() {
    // a fake backend that emits a chunk every 20ms then Done; idle=100ms,wall=10s → completes (no timeout).
}
#[tokio::test]
async fn no_watchdog_config_is_byte_identical() {
    // watchdog=None → no task spawned; a hung turn behaves exactly as today (no AgentTimedOut).
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** (only when `config.watchdog.is_some()`):
  - Build `watchdog_fired: Arc<tokio::sync::Notify>` + a `(done_tx, done_rx) = oneshot::channel()`.
  - **Spawn the watchdog task** (`'static` — captures `Arc<TurnWatch>`, `watchdog_fired`, the two `Duration`s,
    `done_rx`): loop — derive `deadline = min(turn_start + hard_wall_clock, idle_deadline)` where `idle_deadline =
    if la != 0 { la_instant + idle_timeout } else { far_future }` (FIX-9); `tokio::select! { _ = sleep_until(
    deadline) => {}, _ = &mut done_rx => return }`. On wake: reload `la`; if `turn_start.elapsed() >=
    hard_wall_clock` OR (`la != 0` && `now.saturating_sub(la_instant) >= idle_timeout`) → `watchdog_fired.notify
    _one(); return;` else loop (re-derive). (FIX-5 saturating.)
  - **The driver `select!` arm** (`:1971`): add `_ = watchdog_fired.notified() => { <run the SAME bounded cancel as
    the `done_sender.closed()` arm: send CancelNotification(agent_id); inner select!{ &mut prompt_fut | kill.
    notified()→ | sleep(grace)→escalate_terminate }>; timed_out_local = true; Err(()) }`. Hoist the bounded-cancel
    body into a small closure/block reused by both arms.
  - **Teardown:** at the driver all-exit cleanup (`:2002`, after `map.remove`), `drop(done_tx)` (→ the watchdog's
    `done_rx` resolves → it exits). The `map.remove` already drops the `TurnRoute.watch`.
  - **The terminal (`:2010`):** with a `let mut timed_out_local = false;` before the `select!`, set it in the
    watchdog arm; in the terminal `match outcome { Err(()) => if timed_out_local { TurnEvent::Failed(BridgeError::
    AgentTimedOut) } else { <existing AgentCrashed> }, Ok(resp) => <existing Done> }`. (Ok(resp) is a natural
    completion → never AgentTimedOut.)
- [ ] **Step 4: Run → PASS** (the 3 new tests + ALL existing acp turn/cancel tests — the no-config path identical).
  `cargo test -p bridge-acp`; `cargo test --workspace --no-run`. fmt; `cargo clippy -p bridge-acp --all-targets
  -- -D warnings`.
- [ ] **Step 5: Commit.** `git commit -am "feat(acp): E9 watchdog task + driver select arm + AgentTimedOut (s7b FIX-1)"`

---

## Task 6: Gate + whole-branch review + live-gate + merge (controller)

- [ ] **Step 1: Full gate** (controller): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --
  -D warnings`; `cargo test --workspace --exclude bridge-container` (timeout-guarded). Coverage floors per ci.yml.
- [ ] **Step 2: Whole-branch review** — codex-xhigh + Opus on the whole `main...HEAD` diff (the cross-task net),
  iterate to clean. Focus: the driver-arm race-freedom (a natural completion never AgentTimedOut); the watchdog
  task teardown on EVERY exit (no leak); the handler bump non-blocking + unconditional; the no-config byte-identity;
  the blast-radius on a shared backend; the config ripple completeness.
- [ ] **Step 3: Live-gate** vs real codex: (a) a deliberately-hung turn (a prompt the agent accepts then stalls, or
  a tiny `hard_wall_clock`) → `AgentTimedOut` → A2A `Failed`; (b) a long-but-emitting tool turn (steady updates) →
  completes, NOT tripped; (c) no watchdog task leaks.
- [ ] **Step 4: Merge** to `main` once the whole-branch review is clean (controller commits).

---

## Self-Review (controller, against the spec)
- **FIX coverage:** FIX-1 (T5 driver arm + watchdog task) · FIX-2 (T3 TurnRoute) · FIX-3 (T4 handler bump) · FIX-4
  (T4 cast/sentinel) · FIX-5 (T5 saturating) · FIX-6 (T1 error/classifiers) · FIX-7 (T2 config ripple) · FIX-8 (T5
  oneshot teardown) · FIX-9 (T5 sleep-to-deadline) · FIX-10 (doc note) · FIX-11 (T6 live-gate unmodeled case) ·
  FIX-12 (T2 container composition).
- **Type consistency:** `TurnWatch`/`TurnRoute` (T3) used by the handler bump (T4) + the watchdog task (T5);
  `WatchdogConfig` (T2) read in `prompt_inner` (T4/T5); `AgentTimedOut` (T1) emitted by the driver terminal (T5).
- **Risk:** T5 is the keystone — the plan-review must confirm the driver-arm terminal is RACE-FREE (a natural
  `Ok(resp)` can't be relabeled), the bounded-cancel reuse is correct, and the watchdog task is torn down on every
  driver exit (no leak). T2's config ripple completeness (the 5+ sites) is the other risk.
