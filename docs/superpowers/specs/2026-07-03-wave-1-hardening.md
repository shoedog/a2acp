# Wave 1 — Runtime & CI Hardening (spec, v2)

**Status:** APPROVED for implementation — v1 reviewed by codex gpt-5.5 xhigh (verdict: W1-B needs-rework, W1-A/C/D findings); all review changes folded below. Review artifact: scratchpad `wave1-spec-review-out.md`.
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
  `synchronous` == `1` (NORMAL), `busy_timeout` == `5000`. Assert the `-wal` sidecar
  only AFTER a write (it is not created at open; per review) — or leave sidecar
  assertion to the live gate.
- Unit: in-memory store still opens and passes an existing smoke (no WAL assertion).
- Existing full store suite stays green (no behavioral change expected).

## W1-B: `checkout_turn_inner` lock scope (the verified serialization point)

**File:** `crates/bridge-coordinator/src/session_manager.rs` (`checkout_turn_inner`, :379–616).

**Problem (verified):** the `by_context` mutex guard is held across
`self.registry.resolve(&agent).await` (which lazy-spawns the agent process on first
resolve) on **both** the existing-handle path (:395) and the fresh path (:554), and
across `backend.configure_session(...).await` (:574–579) on the fresh path. One slow
spawn blocks every context checkout serve-wide.

**Design (v2 — per codex xhigh review):**

1. **New claim state `SessionState::Configuring`** (session_manager.rs:23–37). It is a
   claimed state like `Reconciling`/`Resetting`/`Compacting`: included in `is_claimed`
   (:39–48), surfaced by `status()` (:660–682) as `"configuring"`, REJECTS
   `inject` (which today accepts only `Running`, :210–237), and is treated by
   `cancel`/`release`/`reset` like the other claimed states (defer/expire semantics,
   NOT real-turn cancellation). Force-clear during `Configuring` follows the
   deferred-expiry protocol below — it must NOT take the `Running if force` path
   (:960–1003), which would race `release_session(old_id)` against the in-flight
   `configure_session(old_id)`.

2. **Optimistic re-check instead of blind hoisting** (preserves today's busy/retired
   error precedence, :388–393):
   - Lock #1: if handle exists → run the existing busy/retired checks FIRST (precedence
     preserved). If Idle-with-matching-potential or absent → record what we saw, drop
     the lock.
   - Off-lock: `registry.resolve(&agent).await`, compute `effective_config` +
     `SessionSpecFingerprint`.
   - Lock #2: re-validate. If a handle now exists where none did (or state changed),
     re-run the same guards against current state (may now return `HandleBusy` /
     `SessionExpired` — correct). Existing-handle path (fingerprint diff → fast turn
     mint, or reconcile/reseed flow) proceeds under lock #2 exactly as today
     (:395–552 minus the resolve, which is pre-computed). The reconcile path already
     drops the guard before its awaits (:473) — unchanged.
   - Fresh path under lock #2: run the fresh-path guards, then insert a handle in
     `Configuring` with a minted **claim id** (reuse `mint_turn_op()` output as the
     claim token, stored in `h.op`) — but do NOT install `turn_abort` and do NOT mark
     `Running` (per review findings (a)/(c)/(e): capabilities may be set at insert
     (`capabilities()` is sync, ports.rs:90–92); `op` here means "claim", not "turn").
     Drop the lock.
   - Off-lock: `configure_session(&backend_session, &spec).await`.
   - Lock #3 (settle — ALWAYS, success and failure): find the handle and validate the
     EXACT claim: same handle identity, `state == Configuring`, `h.op == claim`.
     - Claim intact + configure Ok → transition to `Running`, mint the real turn
       (`turn_abort` token installed NOW), take `pending_seed`/`pending_injects`,
       return the `WarmTurn`.
     - Claim intact + configure Err → remove the handle, return the error.
     - Claim gone/replaced (cancel/release/force-clear ran the deferred-expiry
       protocol) → configure Ok: release the just-configured backend session
       best-effort (`release_session`), return `SessionExpired`; configure Err: return
       the error (nothing to clean).
   - **Deferred expiry protocol:** cancel/release/clear arriving while `Configuring`
     mark the claim expired (same mechanism the other claimed states use — the claim
     check at settle detects it). They must NOT call backend `release_session` for the
     configuring session themselves (the settle owns that), and must NOT fire a turn
     abort token (none exists — this is exactly what avoids widening the documented
     single-slot `turn_abort` hazard, :81–92).

3. **Observable-behavior notes:** same-ctx checkout during configure → `HandleBusy`
   (today it blocks on the mutex then sees the handle; on configure-FAILURE today it
   would then proceed fresh — v2 returns `HandleBusy` during the window instead; this
   narrow difference is accepted and documented). Different-ctx checkout no longer
   waits — the point of the fix.

**Tests (all in session_manager tests, gated-configure fake backend):**
- Different-ctx liveness: ctx A gated in configure; checkout ctx B completes while A
  is gated (bounded by `tokio::time::timeout`, no sleeps).
- Same-ctx busy: second checkout for ctx A during configure → `HandleBusy`, exactly
  one `configure_session` call observed.
- Failure settle: gated configure → `Err`; handle removed; next checkout succeeds fresh.
- Cancel-during-configure: `session cancel` while `Configuring` → no panic, no
  real-turn cancel path taken, no abort-token fired; on configure Ok the settle
  returns `SessionExpired` and the backend session is released; no `WarmTurn` escapes
  for a cancelled claim.
- Force-clear-during-configure: same shape via `reset_session(force=true)` — settle
  detects the replaced claim; no `release_session` vs `configure_session` race
  (assert call ordering via the fake backend's log).
- Status visibility: `status()` during the window reports `configuring`.
- Existing suite green (the ~70%-test file is the safety net).

**Explicit non-goals:** no fairness/queueing; no changes to `reconcile`/`release`/
`reset` beyond teaching their claimed-state matches about `Configuring`; no warm-pool.

**v2.1 (post branch-review):** whole-branch review (codex xhigh) found one MAJOR: lock
#1 observed an existing Idle handle for the ctx but did not *record* that fact before
dropping the lock and resolving off-lock. If a concurrent `release` removed the handle
from `by_context` and its off-lock `backend.release_session("ctx-{ctx}-g0")` was still
in flight when lock #2 re-checked, lock #2 saw NO handle and fell into the fresh path —
re-minting the same deterministic backend session id and racing
`configure_session(g0)` against the still-in-flight `release_session(g0)`. Fixed:
lock #1 now records `saw_handle`; if lock #2 finds the handle gone but `saw_handle` was
true, it returns `SessionExpired` instead of falling into the fresh path (the
state-changed-but-present cases are unaffected — those already re-run the existing
guards). Regression-tested (deterministic interleaving via a `cfg(test)`-only pause
hook between the off-lock resolve and lock #2, mirroring the file's existing
`block_next_*`/`wait_for_*` fake-backend gate style): a warm-idle handle is released
while a racing checkout is parked in that window; asserts the checkout returns
`SessionExpired` and `configure_session` is never called a second time while the
release is still in flight.

**Known pre-existing (out of scope):** a checkout that starts entirely *after* a
release has already removed the handle (i.e. lock #1 itself observes no handle) can
still fresh-mint `g0` against that release's still-in-flight `release_session(g0)` —
this narrower window exists on `main` today and is unchanged by the v2.1 fix above.
Candidate fix: hold an `Expiring` tombstone in `by_context` until `release_session`
completes (rather than removing the entry up front), so a racing checkout sees
`HandleBusy`/waits instead of finding an empty slot; deferred to a future wave.
Separately (opus review note): `checkout_child_turn` still holds the `children` lock
across a cold child's `configure_session` — improved by this wave's lock-scope work on
the parent path but not itself restructured; also deferred to a future wave.

## W1-C: `spawn_blocking` / async process for blocking calls on live paths

**Files:** `crates/bridge-worktree/src/host_git.rs` (`add`, `remove`, `is_git_repo` —
sync `std::process::Command::output()` via `run_git` :21–28 inside `async fn` :81–116,
plus a `std::thread::sleep` retry backoff at :98 that also parks the runtime thread).

**`sweep.rs` ruling (per review):** it is NOT boot-only — `WorktreeRunEndGuard::drop`
runs it synchronously at run-end (:105–119; guards installed at three main.rs sites).
A `Drop` impl cannot await, so converting sweep to async is a redesign, not hygiene.
**Decision: sweep.rs stays sync this wave**, with a code comment stating the rationale
(Drop-guard context; startup/run-end only, not a per-turn path).

**Change (host_git.rs only):** replace `std::process::Command` with
`tokio::process::Command` + `.output().await`; replace the `std::thread::sleep`
backoff at :98 with `tokio::time::sleep(...).await`. Preserve exact args, env, error
mapping, and output parsing. No trait signature changes (methods are already `async`).

**Tests:** existing bridge-worktree suite green; add one test asserting `add` does not
block the runtime: spawn `add` against a repo fixture concurrently with a
short-interval `tokio::time::Instant` tick task and assert tick cadence is maintained
(or, simpler and less flaky: just rely on type-level guarantee + existing suite —
spec-review to rule whether the cadence test earns its flake risk).

## W1-D: CI/toolchain/pin hygiene

**Files:** `.github/workflows/ci.yml`, root `Cargo.toml`, and all 16 member manifests
(15 `crates/*/Cargo.toml` + `bin/a2a-bridge/Cargo.toml`).

**Changes:**
1. `ci.yml`: `dtolnay/rust-toolchain@stable` → explicit `toolchain: 1.94.0` (matching
   `rust-toolchain.toml`); add a one-line `rustc --version` echo step for the log.
2. Root `Cargo.toml` `[workspace.package]`: add `rust-version = "1.94"`; **every one
   of the 16 member manifests** adds `rust-version.workspace = true` (inheritance is
   explicit per package — workspace metadata alone enforces nothing; per review).
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
