# Resume for `a2a-bridge implement` — Design Spec

**Date:** 2026-06-08
**Status:** Approved (brainstorm). Plan + ADR-0026 to follow.
**Builds on:** ADR-0024 (warm loop session), ADR-0023 (review→tweak loop), ADR-0025 (concurrent runs —
flock lease + labels), ADR-0011 (W3b crash-resume — *referenced for contrast, NOT reused*).
**Cross-checked by:** the bridge's own clean-room `design` workflow (containerized codex *executability*
lens + claude *structure* lens + synth), run against this repo. Both lenses + the controller's brainstorm
converged on the spine below; the codex lens supplied the load-bearing `drain_turn` correction.

---

## Goal

Let a long `a2a-bridge implement` run survive a mid-loop death — the ~1h Anthropic prompt-cache / OAuth
horizon, or any crash — by **resuming** instead of losing the work: transparently in-process for transient
deaths, and via an operator `implement --resume <id>` for anything that falls through.

## Context & problem

`implement` clones a repo into a quarantine (`allowed_cwd_root/.a2a-implement/<task_id>`), runs a **warm**
containerized agent (one container + one ACP session across the edit + all fix turns), host-commits the
agent-staged index on a task branch, then runs a bounded tweak loop
(verify → review → fix → `git commit --amend`) until APPROVE+PASS or `[implement].max_attempts`, and hands
off the branch. A long run can die mid-loop:

- the warm ACP session dies at the ~1h prompt-cache TTL / OAuth-token expiry (activity-based); or
- any crash — agent failure, container death, machine reboot, `kill -9`.

Today the run aborts and the in-flight work is stranded with no way to continue.

## What survives a death vs. what's lost (verified against the code)

| State | Survives? | Where |
|---|---|---|
| Quarantine **clone** | ✅ on disk | bridge never deletes it (only the operator hand-off does) |
| Git **commits** (first + each amend) | ✅ on disk | real commits on the task branch |
| Loop **attempt counter** + last verify/review verdicts | ❌ in-memory | stack locals in `run_tweak_loop` |
| Warm **ACP session** | ❌ | strictly process-lifetime; `retire_warm` cancels session + reaps container |

Two correctness facts: **verify** and **review** are safely **re-runnable** over the committed tree (review
is LLM-driven, so output is *not* deterministic — which is exactly why we re-derive verdicts on resume
rather than trust a persisted one); and `implement` is **bespoke off the workflow executor**, so it does
*not* use the server-side TaskStore/ADR-0011 checkpoint system.

## The load-bearing finding: `drain_turn` erases the death

`drain_turn` (`bin/a2a-bridge/src/main.rs:613`) consumes the turn stream, **logs and discards any
mid-stream `Err`** (`:627`), and returns a bare `bool`. The realistic horizon death happens *while
streaming*, so it surfaces as `completed=false` with the error **erased**. Consequence: a retry seam wrapped
around `prompt()` alone is blind to the common death. **The retry seam must sit at the turn level** — a
richer `drain_turn → TurnOutcome { completed, last_err }` feeding a `TurnRunner` port.

## Architecture — two composable layers

- **Layer 2 — manual cross-process resume** (`implement --resume <id>`): the surviving clone + its
  committed branch are the source of truth; a fresh warm session re-enters the bounded loop at the persisted
  attempt. Independent of Layer 1.
- **Layer 1 — in-process auto-retry**: on a *transient* turn death, respawn a fresh warm session on the same
  clone and retry the turn, capped. Needs **no** checkpoint write (same attempt/sha, in-process).

**Composition:** Layer 1 absorbs transient deaths invisibly; when its budget is exhausted or the death is
fatal, the run aborts leaving a checkpoint that Layer 2 can resume. The checkpoint is what bridges L1's
in-process recovery to L2's cross-process recovery.

## Convergent, code-verified decisions

1. **Checkpoint lives in `CLONE/.git/a2a-bridge/`.** Survives the loop's per-iteration
   `git reset --hard HEAD && git clean -fdq` (`tweak.rs:173`) and can never be staged into the hand-off
   commit. Mirrors the existing `.git/A2A_COMMIT_MSG` pattern.
2. **Persist only the correctness-minimal set** (`attempt` + `sha` + metadata). verify/review verdicts are
   **always re-run** on resume — fresh verdicts over the committed tree beat trusting a half-written one.
3. **`run_tweak_loop` gains `start_attempt: u32` + an injected `&mut dyn CheckpointSink`** (separate from
   `TweakEffects` — no filesystem I/O in the existing seam). `classify`'s `attempt >= max_attempts` bound
   (`tweak.rs:85`) then spans resumes automatically.
4. **Two record points → crash-exact `max_attempts` across resumes.** Both `InProgress`:
   - **Entry**, once before `loop {`: `record { attempt: start_attempt, sha }` (covers a crash during the
     first verify).
   - **Post-amend**, right after `attempt += 1; sha = s` (`tweak.rs:241`): `record { attempt, sha }`.

   A crash during attempt *N* leaves the record at *N* and the tree at start-of-*N* → resume restarts *N*
   against that exact tree; `max_attempts` is honored exactly (not +1). The **terminal** status is written
   by `main` after the loop returns; if `main` crashes first, the checkpoint stays `InProgress` and resume
   re-runs verify/review on the converged tree → succeeds immediately → idempotent.
5. **Freeze `loop_max_attempts` in the checkpoint.** Config clamps `max_attempts` to a hard max
   (`IMPLEMENT_HARD_MAX`, `config.rs`); a resume must honor the *frozen* budget, not re-derive it from a
   possibly-edited config. Operational settings (agents, containers, verify/review) still resolve from the
   passed `--config`.
6. **`classify_death` — retry deaths, not refusals.** `BridgeError` is a closed enum (`error.rs`):
   ```rust
   enum Death { Transient, Fatal }
   fn classify_death(e: &BridgeError) -> Death {
       use BridgeError::*;
       match e {
           AgentCrashed { .. } | AgentOverloaded | SessionNotFound | CancelTimeout | FrameError => Transient,
           _ => Fatal, // AuthRequired / AgentNotAuthenticated / … never respawn-loop; new variants default safe
       }
   }
   ```
   A clean non-completion with `last_err == None` is the agent **refusing**, not dying → do **not** respawn.
7. **Concurrency reuses ADR-0025.** A resumed run mints a fresh `instance_id`/lease (`acquire_lease_in`)
   and calls `recover_orphans`; no new lock mechanism. A per-resume **takeover lease** under
   `clone/.git/a2a-bridge/locks` guards against two processes resuming the same clone.

## Resolved owner decisions

1. **Build order:** all four slices (both layers in scope), order 1→4 below. Slices 1–2 ship resume
   entirely before the turn-API change in 3–4.
2. **Discovery:** **direct dir resolution** — `resume_id == task_id == dirname` (the clone dir is already
   named by the unique `task_id`). An `…/.a2a-implement/runs/<id>.json` index for prefix-match UX is a
   cheap retroactive add, deferred.
3. **Respawn budget:** a **config field `[implement].max_session_respawns`** (default 3, same clamp
   discipline as `max_attempts`), so long runs are tunable without a rebuild.

## Components & file boundaries

| File | Change |
|---|---|
| `bin/a2a-bridge/src/implement_resume.rs` | **NEW** — checkpoint schema, atomic load/save, discovery/resolve, validation, HEAD reconciliation, lease acquisition. |
| `bin/a2a-bridge/src/resilient.rs` | **NEW** — `ResilientWarm` (`TurnRunner` impl), `classify_death`. |
| `bin/a2a-bridge/src/tweak.rs` | `run_tweak_loop` gains `start_attempt: u32` + `ckpt: &mut dyn CheckpointSink`; two `ckpt.record(…)` lines. |
| `bin/a2a-bridge/src/main.rs` | `drain_turn → TurnOutcome`; `TurnRunner` port; `ProdEffects` depends on `&dyn TurnRunner`; CLI mode split; prod `CheckpointSink`; the `--resume` command arm; terminal-status write. |
| `bin/a2a-bridge/src/implement.rs` | checkpoint state helpers beside the `A2A_COMMIT_MSG` helpers; subject recompute via `git log -1 --format=%s`. |
| `crates/bridge-core/src/config.rs` (or the `[implement]` config struct) | add `max_session_respawns` (default 3, clamp ≤ a sane max). |

## Key interfaces / types

**CLI — mode split** (the current parser requires a positional task + `--repo`, so `--resume` cannot be a
mere optional field):

```rust
enum ImplementMode {
    Fresh { task: String, repo: PathBuf, base_ref: Option<String>, workflow: String },
    Resume { resume_id: String },
}
struct ImplementArgs { mode: ImplementMode, config: PathBuf }
```
```
a2a-bridge implement <task> --repo <path> [--base-ref <ref>] [--workflow <id>] [--config <path>]
a2a-bridge implement --resume <id> [--config <path>]
```

**Checkpoint schema** (`clone/.git/a2a-bridge/implement-checkpoint.json`, written atomically — temp + rename):

```rust
struct ImplementCheckpoint {
    schema_version: u32,
    resume_id: String,        // == task_id (pid+nonce-unique, implement.rs:94)
    task_id: String,
    task_brief: String,

    source_repo: PathBuf,
    clone_path: PathBuf,
    config_path: PathBuf,

    branch: String,
    base_ref: Option<String>,
    base_commit: String,
    current_commit: Option<String>,   // tip of the single commit
    original_message: Option<String>, // for `--amend --no-edit` subject preservation

    edit_workflow: String,
    fix_workflow: String,
    loop_max_attempts: u32,    // FROZEN from the original [implement] config
    attempt_next: u32,         // the attempt to (re)start at

    phase: ImplementPhase,     // Cloned | EditStarted | FirstCommitCreated | InLoop | Approved | LoopStopped
    created_at_ms: i64, updated_at_ms: i64,
    // optional telemetry only (not load-bearing): last_verify, last_review
}
```

**Turn seam** (Layer 1):

```rust
struct TurnOutcome { completed: bool, last_err: Option<BridgeError> } // .completed preserves the old bool
async fn drain_turn(stream) -> TurnOutcome;

#[async_trait] trait TurnRunner: Send + Sync {
    async fn run_turn(&self, session: &SessionId, parts: Vec<Part>) -> bool; // completed?
}
```

`ProdEffects` depends on `&dyn TurnRunner` (not `&dyn AgentBackend`); the edit turn and `fix` both route
through `run_turn`. **The review path is untouched** (executor-backed, not warm). `ResilientWarm` holds the
swappable container behind interior mutability so the `&dyn TurnRunner` borrow stays stable across respawns.

**Checkpoint sink** (separate injected trait — the pure, unit-testable seam):

```rust
struct Progress { attempt: u32, sha: String, status: Status } // InProgress | terminal
trait CheckpointSink { fn record(&mut self, p: Progress); }
```

## Control flow

**Fresh run:** parse + **freeze** config → clone under `.a2a-implement/<task_id>` → write `Cloned` →
before edit write `EditStarted` → after `host_commit` write `FirstCommitCreated`
(`current_commit`, `original_message`, `attempt_next = 1`) → `run_tweak_loop(start_attempt = 1, ckpt)` →
on return `main` writes terminal `Approved`/`LoopStopped` → **print the existing hand-off text
byte-for-byte** → best-effort `retire` (exactly as today).

**Manual resume:** load config → resolve `<id>` → `clone_path` → read the checkpoint →
`acquire_lease_in(clone/.git/a2a-bridge/locks, "implement-resume")` (takeover gate) → validate (clone +
branch exist, phase not handed-off, `attempt_next <= loop_max_attempts`) → **reconcile HEAD**: if
`HEAD == current_commit` continue; else if a single commit over `base_commit` (an amend may have just
landed) accept the tip; else fail loud with manual-recovery instructions (never amend unless
`HEAD == current_commit`) → **refuse a dirty worktree by default** (don't let the loop's reset/clean
silently discard a half-finished fix) → spawn a fresh warm session (wrapped in `ResilientWarm`), session id
`implement-{task_id}` (+ `-r<n>` suffix for cross-process stale-session hygiene) →
`run_tweak_loop(start_attempt = attempt_next, ckpt)` → hand-off → retire. The fresh session is told
explicitly: *"Prior session state is unavailable; the repository tree and committed diff are
authoritative."* `build_fix_input` (`tweak.rs:216`) is already self-sufficient, so continuing against the
committed tree is an ordinary loop iteration.

**Auto-retry (`ResilientWarm::run_turn`):** `prompt` → `drain_turn` → `TurnOutcome`. If `completed` →
`true`. Else if `last_err = Some(e)`, `classify_death(e) == Transient`, and budget > 0:
`retire` dead backend → `reset_worktree_to_head` (discard the dead turn's scratch) → rebuild the warm
container → reconfigure the session → `budget -= 1` → backoff → retry the same `parts`. Fatal / clean
non-completion (`last_err == None`) / budget exhausted → `false` (edit aborts; fix → `FixIncomplete`),
leaving a resumable checkpoint.

## Risks & mitigations

- **Widening `drain_turn`'s return type touches its existing tests** (`main.rs:2337`). Mechanical; do it in
  slice 3 (a pure refactor) so it lands without behavior change. Until then, do **not** try to recover the
  death from log strings — the typed `last_err` is the only sound signal.
- **HEAD reconciliation ambiguity** after a crash *between* amend and post-amend record: the "single commit
  over base, accept tip" rule handles it; otherwise fail loud. Never amend unless
  `HEAD == checkpoint.current_commit`.
- **Container test seam is `new_with_hooks` (`lib.rs:117`), not `new_warm_with_hooks`** (the latter does not
  exist) — `ResilientWarm` tests inject a fake stream via `new_with_hooks` + the `ContainerSpawn` trait.
- **Lease hygiene:** resume mints a fresh lease + `recover_orphans`, consistent with ADR-0025's manual-reap
  discipline.

## Testing strategy

The bridge's pattern: pure/injectable cores unit-tested; docker paths live-gated.

- **`CheckpointSink`** — a `Vec`-recorder fake; assert crash-exact attempt accounting across the two record
  points + the `start_attempt` re-entry (the keystone test: a crash at attempt N resumes at N, not N+1).
- **`classify_death`** — exhaustive table test over every `BridgeError` variant (closed enum → the test
  fails to compile if a new variant is added without a deliberate classification).
- **`ResilientWarm`** — inject a fake turn stream (transient death then success) via `new_with_hooks`;
  assert respawn + budget decrement + eventual completion, and that a *fatal* death / refusal does **not**
  respawn.
- **Checkpoint load/save + HEAD reconciliation** — pure functions over a temp git repo (real `git` calls,
  no docker): round-trip, dirty-worktree refusal, the three HEAD-reconcile branches.
- **Live gate** — a real `implement --resume` after a forced mid-loop kill (operator-run, peers idle):
  resume re-enters at the persisted attempt and converges; an injected transient death triggers an
  in-process respawn that completes the same invocation.

## Build order (smallest shippable slices)

1. **Checkpoint write-only** — schema + atomic save; `start_attempt` param (default 1) + the two `record`
   points + prod sink + phase writes. *No behavior change* — the checkpoint just appears on disk.
2. **Manual `--resume`** — CLI mode split, resolve/validate/reconcile, lease takeover, fresh warm + re-enter
   loop. **Layer 2 complete** — the operator-facing win, needs none of the turn-API change.
3. **`drain_turn → TurnOutcome` + `TurnRunner` port** — pure refactor; `ProdEffects` routes through it.
4. **`ResilientWarm` auto-retry** — `classify_death` + respawn/budget/backoff + `max_session_respawns`
   config. **Layer 1 complete.**

## Out of scope / deferred

- A run **index** for prefix-match `--resume` UX (direct dir resolution ships first).
- Persisting verify/review **verdicts** for correctness (kept optional-telemetry only).
- Reviving the *original* ACP conversation context (impossible — the session is gone; a fresh session
  continues from the committed tree).
- Resuming a `run-workflow` / `serve` task via this mechanism (this slice is `implement`-only).

## ADR

This increment gets **ADR-0026** (resume for `implement`); the concurrency bits reuse ADR-0025 (no new
concurrency ADR).
