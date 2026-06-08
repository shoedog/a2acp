# ADR-0026 — Resume for `a2a-bridge implement`

**Date:** 2026-06-08
**Status:** Accepted

**Builds on:** ADR-0024 (warm loop session), ADR-0023 (review→tweak loop), ADR-0025 (concurrent runs —
flock lease + labels), ADR-0011 (W3b crash-resume — *referenced for contrast, NOT reused*).

**Spec/plan:** `docs/superpowers/specs/2026-06-08-implement-resume-design.md`,
`docs/superpowers/plans/2026-06-08-implement-resume.md`.

---

## Context

`a2a-bridge implement` clones a repo into a quarantine, runs a **warm** containerized agent (one container +
one ACP session across the edit + all fix turns), host-commits the agent-staged index, then runs a bounded
tweak loop (verify → review → fix → `git commit --amend`) until APPROVE+PASS or `[implement].max_attempts`,
and hands off the branch. A long run can die mid-loop — the ~1h Anthropic prompt-cache / OAuth-token horizon
(activity-based), or any crash (agent failure, container death, reboot, `kill -9`). Before this increment,
that aborted the run and stranded the in-flight work with no way to continue.

What survives a death (verified): the quarantine **clone** (the bridge never deletes it) and all git
**commits** (the first commit + each amend). What is lost (in-memory only): the loop's attempt counter +
the last verify/review verdicts; and the warm ACP session (strictly process-lifetime). `verify`/`review` are
safely **re-runnable** over the committed tree (review is LLM-driven, so its *output* isn't deterministic —
which is exactly why we re-derive verdicts on resume rather than trust a persisted one). `implement` runs
**off the workflow executor**, so it does not use the server-side TaskStore/ADR-0011 checkpoint system.

## Decision

A bespoke persistence layer around the warm-session loop, in **two composable layers**.

- **Checkpoint** (`implement_resume::ImplementCheckpoint`, JSON) lives in **`CLONE/.git/a2a-bridge/`** — it
  survives the loop's per-iteration `git reset --hard && git clean -fdq` (which resets the WORKTREE, not
  `.git/`) and can never be staged into the hand-off commit (mirrors `.git/A2A_COMMIT_MSG`). It persists the
  correctness-minimal set: `attempt_next` + `current_commit` (+ metadata); verify/review verdicts are
  re-run, never trusted. Written atomically (temp + rename).
- **`run_tweak_loop` gains `start_attempt` + an injected `CheckpointSink`** (separate from `TweakEffects`).
  Two record points — loop **entry** and **post-amend** — give **crash-exact `max_attempts` across
  resumes** (a crash during attempt *N* leaves the record at *N* and the tree at start-of-*N* → resume
  restarts *N*, not *N+1*). The terminal phase (`Approved`/`LoopStopped`) is written by the caller after the
  loop returns; a crash before that leaves `InLoop`, and resume re-runs verify/review on the converged tree
  → succeeds immediately (idempotent).
- **`loop_max_attempts` is FROZEN in the checkpoint** — a resume honors the original budget, not a
  possibly-edited config (operational settings — agents, containers, verify/review — still resolve from the
  passed `--config`).
- **Layer 2 — manual `implement --resume <id>`.** CLI mode split (`ImplementMode::{Fresh, Resume}`). Direct
  dir resolution (`resume_id == task_id == dirname` under `allowed_cwd_root/.a2a-implement/`, traversal-
  rejected). A **takeover lease** (`acquire_lease_in(clone/.git/a2a-bridge/locks, "implement-resume")`)
  guards against two concurrent resumes. **HEAD reconciliation**: `HEAD == current_commit` → resume there;
  else a single commit over base (an amend may have just landed) → accept the tip; else fail loud. A **dirty
  worktree is refused** (the loop's reset would discard a half-finished fix), and the **branch must match**
  the checkpoint. A fresh warm session re-enters `run_tweak_loop` at `attempt_next`; `build_fix_input` is
  already self-sufficient, so a fresh session continuing against the committed tree is an ordinary iteration.
- **Layer 1 — in-process auto-retry.** `drain_turn` was widened to `TurnOutcome { completed, last_err }` —
  it **captures the in-stream death** the old bool drain erased (the realistic horizon death dies *while
  streaming*). A `TurnRunner` port (moved to `turn.rs`) abstracts a turn; `ResilientWarm` implements it: on
  a **transient** death (`classify_death` → `Transient` for `AgentCrashed`/`AgentOverloaded`/
  `SessionNotFound`/`CancelTimeout`/`FrameError`; `_` → `Fatal`) with budget remaining, it **resets the
  worktree to HEAD** (an injected `reset_worktree` closure — discards the dead turn's scratch), rebuilds a
  fresh warm container+session (`WarmRebuild`), reconfigures the session, and retries the same parts.
  "Retry deaths, not refusals" — a clean non-completion (`last_err == None`) is the agent *refusing* and is
  not respawned. The budget is **`[implement].max_session_respawns`** (default 3, clamped to
  `RESPAWN_HARD_MAX = 20`).
- **Composition:** Layer 1 absorbs transient deaths invisibly; on budget-exhaustion or a fatal death the run
  aborts leaving the checkpoint that Layer 2 resumes. Layer 1 needs no checkpoint write (same attempt/sha,
  in-process).

The fresh and resume paths share a single extracted `run_warm_loop` helper (warm setup → loop → hand-off →
retire), so they can't diverge.

## Consequences

- A stranded `implement` run is resumable: transparently for transient deaths, and via `implement --resume
  <id>` for the rest. The hand-off stays byte-shape-identical.
- **Pure cores unit-tested** (the bridge's pattern): the `CheckpointSink` recorder (crash-exact attempt
  accounting + `start_attempt` re-entry), `classify_death` (exhaustive table over the closed `BridgeError`),
  `resolve_clone`/`validate_resumable`/`reconcile_head` (over a temp git repo), and `ResilientWarm`
  (fake-stream: one respawn → completes; fatal/refusal/exhausted-budget → no respawn; respawn resets the
  worktree). The docker wiring is live-gated.
- **Respawn budget is RUN-WIDE** (shared by the edit turn + all fix turns), not per-turn — a deliberate
  choice: a run that keeps dying is more likely systemically wedged than transiently unlucky, so a single
  pooled budget bounds total respawn cost. (Flagged by the build's own review.)
- **Self-hosted dogfood:** this feature was implemented BY `a2a-bridge implement` (codex), slice by slice,
  with the bridge's own verify+review+tweak loop gating each. Slice 4's review correctly **REJECTED** a
  first attempt (a respawn that replayed a turn *without* resetting the worktree → hybrid-commit risk) and
  the tweak loop fixed it (the injected `reset_worktree` closure) before APPROVE — a real validation of the
  loop. (Slice 1's run OOM-died under concurrent load mid-review; its already-correct diff was adopted and
  gated by hand. ADR-0025's crash-recovery cleanly reaped the OOM'd run's orphans.)

## Deferred

- A run **index** for prefix-match `--resume` UX (direct dir resolution ships first).
- Persisting verify/review **verdicts** for correctness (kept optional-telemetry only).
- Reviving the *original* ACP conversation context (impossible — a fresh session continues from the
  committed tree).
- Resuming a `run-workflow` / `serve` task via this mechanism (this slice is `implement`-only).
- **Stale `.git/A2A_COMMIT_MSG` on an EDIT-turn respawn** (MINOR, build-review-flagged): the `reset_worktree`
  closure runs `git reset --hard && clean -fdq`, which does not touch `.git/` — so a dead edit turn's
  `A2A_COMMIT_MSG` could persist into the retried turn's commit message. Low impact (same task, same intent);
  fix = also `rm -f .git/A2A_COMMIT_MSG` in the respawn reset.
