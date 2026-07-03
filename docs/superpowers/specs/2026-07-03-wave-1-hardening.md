# Wave 1 — Runtime & CI Hardening (spec, v1)

**Status:** Draft, pending codex xhigh spec-review.
**Source:** `docs/2026-07-03-strategic-analysis.md` next-steps #1, #2, #8 (the S/M-effort code items).
**Branch:** `feat/wave-1-hardening`.
**Out of scope (later waves):** README rewrite / artifact purge / tier ADR (Wave 2, docs), CLI polish + doctor + A2A golden fixtures (Wave 3), release engineering M2 + eval harness M3, bin extraction (#9), Coordinator migration (#10).

## Why this wave first

All four items are small, independently testable, and de-risk everything after them: the
lock fix removes the one verified serve-wide serialization point before any concurrency
work builds on it; the SQLite pragmas remove fsync-gating before batch usage grows; the
CI pin closes a latent local/CI divergence before more contributors (or agents) build
against it.

## W1-A: SQLite pragmas (WAL, synchronous, busy_timeout)

**File:** `crates/bridge-store/src/sqlite.rs` — both connection-open constructors
(file-backed and in-memory; today the only pragma set is `foreign_keys`).

**Change:** at file-backed open, execute in order:
1. `PRAGMA journal_mode=WAL;` — query the returned mode; if the result is not `wal`
   (read-only FS, exotic mount), `tracing::warn!` and continue — never fail the open.
2. `PRAGMA synchronous=NORMAL;` — safe under WAL (durability at checkpoint, not per-commit).
3. `PRAGMA busy_timeout=5000;`

In-memory connections: set `busy_timeout` only (WAL is meaningless for `:memory:`; the
pragma silently reports `memory` — do not warn for in-memory).

**Invariants:**
- Existing on-disk DBs are silently upgraded (journal_mode persists per-database file) —
  this is one-way but harmless; document in the commit message, no migration needed.
- WAL introduces `-wal`/`-shm` sidecar files beside the store DB. The single-serve lock
  already prevents multi-process writers; sidecars are operationally inert but must not
  break `validate --repo-hygiene` or any path assumptions (check: nothing globs the store
  dir expecting exactly one file).

**Tests:**
- Unit: open a temp file-backed store, `query_row("PRAGMA journal_mode")` == `"wal"`,
  `synchronous` == `1` (NORMAL), `busy_timeout` == `5000`.
- Unit: in-memory store still opens and passes an existing smoke (no WAL assertion).
- Existing full store suite stays green (no behavioral change expected).

## W1-B: `checkout_turn_inner` lock scope (the verified serialization point)

**File:** `crates/bridge-coordinator/src/session_manager.rs` (`checkout_turn_inner`,
~lines 378–620).

**Problem (verified):** the `by_context` mutex guard is held across
`self.registry.resolve(&agent).await` (which lazy-spawns the agent process on first
resolve) on **both** the existing-handle path (~:395) and the fresh path (~:554), and
across `backend.configure_session(...).await` (~:574–579) on the fresh path. One slow
spawn blocks every context checkout serve-wide.

**Design — two changes, in order of decreasing safety:**

1. **Hoist resolve + fingerprint above the lock (pure win).**
   `registry.resolve()` reads `ArcSwap` registry state and never touches `by_context`;
   the lock has never protected it. Move `resolve` + `effective_config` +
   `SessionSpecFingerprint` construction to the top of `checkout_turn_inner`, before
   `self.by_context.lock().await`. Both paths then use the pre-computed `resolved`/`fp`.
   Semantics: the registry snapshot is taken marginally earlier; since the registry is
   independently mutable (hot-reload) this is the same race exposure as today, only
   narrower in time. NOTE: resolve is now paid even when the ctx turns out
   busy/retired — acceptable (resolve on a warm registry is a map lookup; the lazy
   spawn only fires for genuinely new agents, which is exactly the case we must not
   serialize).

2. **Fresh path: insert-claimed, configure outside the lock, settle on re-lock.**
   - Under the lock: after the fresh-path guards pass, mint the op + abort token,
     construct the handle in the **claimed** state (`SessionState::Running`, `op` set)
     with the resolved backend + backend_session, insert into the map, **drop the lock**.
   - Outside the lock: `configure_session(...).await`.
   - On success: return the `WarmTurn` (no re-lock needed if the handle was fully
     constructed at insert; verify nothing else must be written post-configure — if
     capabilities/usage fields are set post-configure today, set them at insert from
     `resolved.backend.capabilities()`, which is sync).
   - On failure: re-lock, remove the handle **only if its op still equals the minted op**
     (a force-clear/cancel may have raced and replaced it — never remove someone else's
     handle), return the error.
   - Concurrent checkout for the same ctx during configure now sees a claimed handle →
     `HandleBusy`. This matches today's observable behavior (today the second caller
     blocks on the mutex through the whole configure, then sees `Running` → `HandleBusy`)
     — it just stops *unrelated contexts* from also blocking.
   - Cancel/clear racing the configuring handle: both go through claim-state guards that
     reject non-Idle states or fire the turn-abort token; the minted abort token exists
     from insert, so `session cancel` during configure behaves like cancel-during-turn
     (accepted, token fired; the configure result is settled by the op-equality check).
     Spec-review should pressure-test this claim against `fire_lingering_turn_abort` and
     the generation guards.

**Explicit non-goals:** no new `SessionState` variant (reuse `Running` + op-equality
settle); no change to `reconcile`/`release`/`reset` (already drop the guard correctly);
no fairness/queueing changes.

**Tests:**
- Concurrency (the headline): fake backend whose `configure_session` awaits a
  `Notify`/gate. Start checkout for ctx A (blocks in configure, off-lock). Assert a
  checkout for ctx B (different agent, instant configure) completes while A is still
  gated. Pre-fix this deadlocks/times out; post-fix passes. Bound with a generous
  `tokio::time::timeout`, no sleeps.
- Same-ctx busy: while A is gated in configure, a second checkout for ctx A returns
  `HandleBusy` (not a hang, not a duplicate spawn).
- Failure settle: gated configure resolves to `Err` → handle removed, next checkout for
  that ctx succeeds fresh.
- Race settle guard: while A is gated, force-clear ctx A; configure then fails/succeeds —
  assert the settle does not clobber the post-clear state (op-equality branch).
- Existing session_manager suite green (the ~70%-test file is the safety net).

## W1-C: `spawn_blocking` / async process for blocking calls on live paths

**Files:** `crates/bridge-worktree/src/host_git.rs` (`add`, `remove`, `is_git_repo` —
sync `std::process::Command::output()` inside `async fn`), `crates/bridge-worktree/src/sweep.rs`
(same pattern at ~:10 if it runs post-boot; if it is strictly boot-time-before-serve,
leave it and note why).

**Change:** replace `std::process::Command` with `tokio::process::Command` +
`.output().await` (preferred over `spawn_blocking` — same semantics, no thread-pool
hop, kill-on-drop available). Preserve exact args, env, error mapping, and output
parsing. No trait signature changes (methods are already `async`).

**Tests:** existing bridge-worktree suite green; add one test asserting `add` does not
block the runtime: spawn `add` against a repo fixture concurrently with a
short-interval `tokio::time::Instant` tick task and assert tick cadence is maintained
(or, simpler and less flaky: just rely on type-level guarantee + existing suite —
spec-review to rule whether the cadence test earns its flake risk).

## W1-D: CI/toolchain/pin hygiene

**Files:** `.github/workflows/ci.yml`, root `Cargo.toml`, all 16 member `Cargo.toml`s,
`bin/a2a-bridge/Cargo.toml`.

**Changes:**
1. `ci.yml`: `dtolnay/rust-toolchain@stable` → explicit `toolchain: 1.94.0` (matching
   `rust-toolchain.toml`); add a one-line `rustc --version` echo step for the log.
2. Root `Cargo.toml` `[workspace.package]`: add `rust-version = "1.94"`; each member
   adds `rust-version.workspace = true` (mechanical, 17 manifests).
3. `bin/a2a-bridge/Cargo.toml`: replace the hand-duplicated
   `a2a = { package = "a2a-lf", version = "=0.3.0" }` in `[dependencies]` with
   `a2a.workspace = true` (the dev-dependency already does this).
4. **Deliberately NOT adding** a separate plain `cargo test` CI job: `cargo llvm-cov`
   already executes the full suite; a second run doubles CI time for no new signal.
   Recorded here so the strategic-analysis recommendation is answered, not ignored.

**Tests:** CI itself is the test; locally `cargo build -p a2a-bridge` + `cargo deny check`
confirm the pin swap and MSRV field parse.

## Definition of done (wave)

1. All four items implemented on `feat/wave-1-hardening`, TDD per task, one commit per task.
2. Gates green locally: `cargo fmt --check`, `cargo clippy --workspace --all-targets -j 1 -- -D warnings`,
   `cargo test --workspace -j 1`, `cargo deny check`, `a2a-bridge validate --repo-hygiene`.
3. **Live gate:** boot `serve` against a file-backed store; run one real warm turn
   (codex) end-to-end; then `sqlite3 <store> "PRAGMA journal_mode"` returns `wal` and
   the `-wal` sidecar exists; a second concurrent context checkout is not blocked by a
   cold spawn (observable via two `submit`s to different agents, the second returning
   while the first's agent is still initializing — best-effort observation, the unit
   concurrency test is the hard gate).
4. Whole-branch dual review (opus 4.8 + codex xhigh) clean or findings folded; merge to
   `main`; push.

## Risks

- **W1-B is the only risky item.** The settle-on-failure op-equality logic interacts
  with cancel-tokens (`fire_lingering_turn_abort`) and force-clear generation guards —
  the two most bug-dense subsystems in the repo's history. Mitigation: the four
  targeted tests above + task review by opus 4.8 + me + whole-branch codex xhigh with
  this interaction called out explicitly.
- WAL persistence is one-way per DB file (harmless; documented).
- MSRV field makes builds on <1.94 fail fast with a clear error — that is the point,
  but note it in CONTRIBUTING if anyone complains.
