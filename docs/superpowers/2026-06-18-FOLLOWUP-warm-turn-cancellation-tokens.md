# Follow-up (tracked, deferred) — Warm-turn cancellation tokens

> **Status:** DEFERRED, tracked. Discovered by the whole-branch codex-xhigh review of Slice 3 (clear/reset),
> kept out of Slice 3 per an explicit **merge-as-is** decision (2026-06-18). This is the durable home for the
> follow-up so it is **not lost after the per-slice handoff docs are superseded.** Linked from the authoritative
> plan (`docs/superpowers/specs/2026-06-17-orchestration-slicing.md`) and the resume HANDOFF.
>
> **Sequencing rule:** land this **before any feature that relies on `force`/cancel under concurrency.** It is
> NOT a blocker for Slices 4 (compact — require-Idle, no `force`) or 5 (serve-CLI). Earliest natural slot:
> **post-MVP (after Slice 5)**, or sooner if a force/cancel-concurrency feature is pulled forward.

## What this is

Two **PRE-EXISTING** concurrency races in the warm-session lifecycle (present since Slices 0/1/2; the Slice-3
`force` clear + the generation guard make them *visible*, they did not introduce them). Both stem from the same
gap: **there is no per-turn cancellation mechanism** — the bridge cannot abort an in-flight producer/turn, and
the stale-write guard keys on a *task-derived* operation id rather than a unique nonce. Slice 3 shipped a sound
core (new-generation reset + `gen && op && Running` guard, live-gate-proven) but FIX-12's op-token is only a
*partial* close.

**Do NOT assume the cancel/force paths are race-free under concurrency until this lands.**

## The two races

### Race 1 — cancel→next-turn op collision (FIX-12 is partial)
`OperationId` is **task-derived** at the server edge (`op-{taskId}`, `"task-1"` fallback when `taskId` is
omitted — `server.rs:732 / 2321 / 3158`). The generation guard
(`finish_turn`/`record_usage` no-op unless `gen == handle.generation && op == Some(op) && state == Running`)
therefore fails to discriminate when two turns share an op:

- A `SessionCancel` followed by a same-context send with the **same or omitted** `taskId` reuses the op.
- The cancelled producer's late `finish_turn` / `record_usage` then still satisfies `gen && op && Running`
  (no generation bump on a plain cancel) and can idle/clobber the *new* turn.

The generation guard alone already covers the post-`reset` case (gen bumps); this is specifically the
**cancel / no-generation-bump** case.

**Fix:** mint a **UNIQUE per-checkout operation token** (a nonce) in `SessionManager`, independent of the client
`taskId`; the guard keys on the nonce. Thread it through `WarmTurn` / `WarmTurnGuard` exactly where the current
`op` flows.

### Race 2 — `clear --force` vs producer start
`checkout_turn` marks the handle `Running`, but the streaming/unary handlers `await store.put(...)` **before**
spawning the producer (`server.rs:749 / 2340`). In that window:

- A concurrent `SessionClear --force` claims (`Running`→`Resetting`) and **releases** the old bridge
  `SessionId`.
- The original handler then resumes and prompts the **released** session, which ACP **lazy re-mints**
  (`acp_backend.rs:2052` → `translator.rs:133`) — **resurrecting the force-cleared context** the operator just
  asked to wipe.

**Fix:** a **per-turn abort/cancellation token** owned by the `SessionManager`, cancelled under the reset claim
during `force`. The producer/translator `select!`s on it **before and while** entering `backend.prompt`, so a
force-clear aborts the in-flight turn instead of racing it. (Deferring `force` entirely also closes this —
`clear` would then strictly require `Idle`.)

## Recommended slice — "warm-turn cancellation tokens"

One foundational follow-up closes BOTH races and makes `force`-clear truly abortive:

1. **Manager-minted unique op nonce** per `checkout_turn` (closes Race 1). Independent of client `taskId`;
   guard keys on it.
2. **Per-turn abort token** owned by the manager, wired through the producer → translator → `backend.prompt`
   via `select!`, cancelled under the reset claim during `force` (closes Race 2; makes `force` abortive rather
   than racy).

### Code anchors (verify before implementing — these were current at 2026-06-18)
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `checkout_turn` (mint), `WarmTurn`, `finish_turn`,
  `record_usage`, `reset_session` (the `Resetting` claim + release path).
- `crates/bridge-a2a-inbound/src/server.rs:732 / 2321 / 3158` (op derivation), `:749 / 2340` (the
  `store.put`-before-spawn gap), `WarmTurnGuard`.
- `crates/bridge-acp/src/acp_backend.rs:2052` (lazy re-mint on prompt), `crates/bridge-core/src/translator.rs:133`
  (mint path entered from prompt).

### DoD / live-gate (when built)
- Race 1: a scripted cancel + same/omitted-taskId resend proves the cancelled producer's late completion does
  NOT touch the new turn's state (unit-gated on the manager; the live shape is cancel-then-immediately-resend).
- Race 2: a `clear --force` fired in the checkout→prompt window leaves the context **cleared** (recall=none),
  never resurrected — the in-flight producer is aborted, not allowed to re-mint.

## Related: claim-held-across-await robustness (added 2026-06-19, Slice-4 whole-branch review)

The Slice-4 whole-branch review surfaced two robustness patterns in the claim-held-across-await design that
Slice 4 FIXED for compact but which also (with a SMALLER window) exist in the SHIPPED `reset_session`/clear
path:

1. **Caller-future-drop strands a claim.** A handler that `await`s a claim-held op (`reset_session` /
   `compact_session`) directly will, if its future is dropped mid-op (client disconnect / a request-timeout
   layer), leave the handle stranded in the claim state forever (`reap_idle` skips claimed states; release/
   cancel only defer). Slice 4 fixed COMPACT by running `compact_session` on a DETACHED `tokio::spawn` task in
   `session_compact` (the task always drives to commit-or-EXPIRE). `reset_session` (via `session_clear`) still
   awaits directly — its window is tiny (local release+configure, ms) vs compact's full summarize turn, but the
   same spawn-detach would close it. **Follow-up: apply spawn-detach to `session_clear` too.**
2. **reap-vs-claim TOCTOU (already fixed for ALL claims).** `reap_idle` snapshotted Idle contexts, dropped the
   lock, then `release`d — a claim landing in that gap got defer-expired, killing it. Slice 4 fixed this
   GENERALLY (reap now re-validates + removes atomically under the lock, skipping any claimed handle), so it is
   already closed for reset/reconcile/compact. No further action.

These are robustness hardening (not the op-collision / force-clear races above); sequence with the cancellation-
token work or as a small standalone `session_clear` spawn-detach fix.

## Source / provenance
- Full detail + the merge-as-is decision: `docs/superpowers/specs/2026-06-18-slice-3-clear-reset.md`
  → **"## Deferred hardening (whole-branch review round 2)"**.
- Memory: `slice-3-clear-reset-shipped` (the DEFERRED HARDENING block).
- Resume HANDOFF: `docs/superpowers/2026-06-17-orchestration-HANDOFF.md` (Slice 3 TL;DR + backlog).
