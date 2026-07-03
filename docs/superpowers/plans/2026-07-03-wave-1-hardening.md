# Wave 1 — Runtime & CI Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship strategic-analysis next-steps #1/#2/#8: SQLite pragmas, the checkout lock-scope fix, async process calls in bridge-worktree, CI/toolchain pins.

**Architecture:** Four independent tasks on `feat/wave-1-hardening`, disjoint file sets, one commit per task. Task 4 (W1-B) is the only stateful design change and follows the v2 spec exactly (`docs/superpowers/specs/2026-07-03-wave-1-hardening.md` — read it first; it is the contract, folded from a codex xhigh review).

**Tech stack:** Rust 1.94.0 (pinned), tokio, rusqlite (bundled).

## Global constraints

- TDD: failing test → minimal impl → green → commit. One commit per task, staged by explicit path (`git add <your files only>`); if `index.lock` contention occurs, retry up to 3×.
- Gates that must stay green per task: `cargo fmt --check`, `cargo clippy -p <touched crates> --all-targets -j 1 -- -D warnings`, `cargo test -p <touched crates> -j 1`.
- NEVER run full-workspace `--all-targets` builds/tests (OOM on this machine); per-crate `-j 1` only. The orchestrator runs the workspace gate once at the end.
- Match surrounding code style; comments only for constraints the code can't show.

---

### Task 1: W1-A SQLite pragmas

**Files:** Modify `crates/bridge-store/src/sqlite.rs` (both constructors). Tests inline (`#[cfg(test)]` in the same file, following its existing pattern).
**Contract:** spec §W1-A. File-backed open: `journal_mode=WAL` (query result; `tracing::warn!` if not `"wal"`, never fail), `synchronous=NORMAL`, `busy_timeout=5000`. In-memory: `busy_timeout` only. Tests: pragma readback (journal_mode==wal, synchronous==1, busy_timeout==5000) on a tempfile store; in-memory smoke unchanged; full `-p bridge-store` suite green. Do not assert `-wal` sidecar at open.

### Task 2: W1-C async process in bridge-worktree

**Files:** Modify `crates/bridge-worktree/src/host_git.rs` (`run_git` :21–28 → `tokio::process::Command`; `std::thread::sleep` :98 → `tokio::time::sleep`); add a rationale comment to `crates/bridge-worktree/src/sweep.rs` (stays sync: `Drop`-guard context, startup/run-end only — spec §W1-C).
**Contract:** identical args/env/error-mapping/output-parsing; no signature changes. `-p bridge-worktree` suite green. No new cadence test (spec ruled it out as flake risk).

### Task 3: W1-D CI/toolchain/pin hygiene

**Files:** Modify `.github/workflows/ci.yml` (toolchain `stable` → `1.94.0` + a `rustc --version` log step), root `Cargo.toml` (`rust-version = "1.94"` in `[workspace.package]`), all 16 member manifests (add `rust-version.workspace = true` — 15 `crates/*/Cargo.toml` + `bin/a2a-bridge/Cargo.toml`), `bin/a2a-bridge/Cargo.toml` (`[dependencies]` `a2a = { package = "a2a-lf", version = "=0.3.0" }` → `a2a.workspace = true`).
**Contract:** spec §W1-D. Verify: `cargo metadata --no-deps` parses; `cargo build -p a2a-bridge -j 1` green; `cargo deny check licenses` green. Deliberately NO plain-cargo-test CI job (spec records why).

### Task 4: W1-B Configuring claim state + lock-scope fix

**Files:** Modify `crates/bridge-coordinator/src/session_manager.rs` only. Tests inline in its existing `#[cfg(test)]` module.
**Contract:** spec §W1-B (v2) is the authoritative design — implement it exactly: new `SessionState::Configuring` (claimed; in `is_claimed`; `status()` string `"configuring"`; `inject` rejects; cancel/release/reset treat as claimed with deferred expiry, NOT the `Running if force` path); optimistic re-check (lock #1 precedence checks → off-lock resolve → lock #2 re-validate → fresh path inserts `Configuring` with claim id, no `turn_abort` → off-lock `configure_session` → lock #3 settle by exact claim on success AND failure, including the claim-gone branches). The six tests named in spec §W1-B **Tests** are required, gated-configure fake backend, `tokio::time::timeout` bounds, no sleeps. `-p bridge-coordinator -j 1` suite green.

---

**Orchestrator-owned (not tasks):** per-task review (opus 4.8 + controller), workspace-wide gate run, live gate per spec DoD, whole-branch dual review (opus 4.8 + codex gpt-5.5 xhigh), merge + push.
