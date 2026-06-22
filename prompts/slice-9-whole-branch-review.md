You are doing a rigorous, adversarial WHOLE-BRANCH REVIEW (read-only) of the COMPLETED **Slice 9 — Turn Channel +
E2 interactive permission** feature branch of the a2a-bridge (a Rust A2A↔ACP bridge + orchestrator). All 11 tasks
are implemented + per-task tested + LIVE-GATED vs real codex (deny/approve/cancel-mid-permission + queued-inject
all PASS). Your job: find CROSS-TASK INTEGRATION BUGS, lifecycle/concurrency hazards, and correctness gaps that
the per-task tests + the happy-path live-gate MISSED — the kind that only appear when the whole branch is viewed
together. The prior slice's whole-branch review found ~19 such bugs across 7 rounds; be that thorough.

READ-ONLY: read the diff + the real code; do NOT edit/build/test.

## What the branch does
- **Queued-inject** (A): `SessionManager.pending_injects` + `inject()` (dedupe replace-in-place, 32/64KB caps,
  D5 Idle-or-Running) drained at the 3 checkout sites into `WarmTurn.injects`→`LocalDispatch.injects`; a shared
  `assemble_turn_parts(seed, injects, base: Vec<Part>)` used by `collect_turn` + BOTH A2A producers; clear-drops,
  compact-rejects-while-pending.
- **E2 interactive permission** (B): a `bridge_core::permission::PermissionRegistry` (gen+op-keyed, exact-once
  `resolve`, `resolve_context_cancelled` per-send Cancelled, drop-guard `reap`, `pending` for status) shared
  `Arc` into AcpBackend + SessionManager + InboundServer + Coordinator. `TurnMeta{ctx,gen,op}` threaded via a
  defaulted `AgentBackend::configure_turn` + a backend stash TAKEN at `prompt_inner` entry (before
  `ensure_session`) → `TurnRoute.turn_meta`; `ContainerRwBackend` forwards it. The ACP reverse handler:
  `policy.interactive_decide()` → `Decide`(byte-identical auto path) | `Defer`(register + `biased select!{rx,
  sleep(timeout)=>Deny}` → map `PermitDecision`→ACP outcome; no-registry/no-meta → default-deny). SessionManager
  `cancel_inner`/`release_inner`/`reset_session_inner` call `resolve_context_cancelled(ctx)` AFTER the
  is_claimed/Cancelling defer guards, before the backend await. `[server] permission_policy="defer"` +
  `permission_timeout_ms` select `DeferPolicy`. `session/status` lists `pendingPermissions`. `SessionInject`/
  `SessionPermit` (+ MCP tools) via `apply_permit` (Escalate = TRUE no-op: no resolve, no reap).

## The diff to review
Run: `git --no-pager diff main..HEAD` (11 feat/test commits). Also read the binding spec + plan:
`docs/superpowers/specs/2026-06-22-slice-9-turn-channel-e2.md` (`## v2`), `docs/superpowers/plans/2026-06-22-
slice-9-turn-channel-e2.md` (`## v2` + `## v3`).

{{input}}

GROUND every finding in real `file:line`. Pressure-test specifically:

1. **resolve_context_cancelled placement (the data-loss guard).** Re-verify in `session_manager.rs`: is the call
   on EVERY teardown path AFTER the claim guards and BEFORE the backend await? Is there ANY cancel/clear/release
   path (force vs non-force, release_all, reap/reconcile-expire, child contexts via `*_with_children`) that
   either (a) tears down a session WITHOUT resolving its pending permission (→ the parked handler hangs until
   timeout), or (b) resolves a pending permission of an IN-FLIGHT compact/reset claim (→ poisons summarize →
   EXPIRE → data loss)? Check `reap_idle`/lease-retire eviction paths too.
2. **Registry liveness / leaks.** Can a `PermKey` entry outlive its handler task on ANY path (normal finish,
   abandon, backend crash, peer-gone responder)? Is the drop-guard held across the ENTIRE await in the handler
   (every early return)? Does a generation bump (clear/reset/compact) leave a stale-gen entry that a later permit
   could resolve against the WRONG turn? Is `resolve` truly exact-once under concurrent permit+cancel+timeout?
3. **TurnMeta threading completeness + races.** Is `configure_turn`'s backend stash race-free given
   `ensure_session().await` runs before the turn lock? Two turns on the same session (reuse / rapid follow-ups) —
   can the take-at-entry read STALE meta or the WRONG turn's meta? Cold-bind / detached / legacy `session-{task}`
   with no meta — does the handler default-deny cleanly (no panic, no hang, no registry entry)? Does the
   container forward apply the meta to the correct inner for BOTH warm + cold container paths?
4. **assemble_turn_parts / inject integration.** Is the seed wrapper BYTE-IDENTICAL (dead-safe)? Do injects drain
   EXACTLY once across all 3 checkout sites + both producers (no double-inject, no drop)? Caps/dedupe correct
   under the lock? Does compact-reject vs clear-drop match D3? Any producer that bypasses the helper?
5. **apply_permit / Escalate / wire.** Is Escalate a TRUE no-op on EVERY surface (A2A + MCP)? Do
   `PermitDecision::Deny` and a policy `Err(PermissionDenied)` select the IDENTICAL reject option? Does
   `SessionPermit` reject stale gen/op (gen-safety)? Does the camelCase serde (`rename_all_fields`) round-trip
   AND match the CLI's emitted JSON (`optionId`)? Param parsing edge cases (missing fields, bad decision tag)?
6. **Dead-safe / byte-identity.** With `AutoPolicy` (the default), is EVERY pre-slice behavior unchanged —
   `decide()` impls, the `Decide` branch mapping, `PermissionRequest::with_id(...,false)`, no registry entry, no
   new event? Any place a non-Defer policy could accidentally reach the Defer path?
7. **Cross-crate consistency.** `PermissionRegistry` in bridge-core used by 4 crates — any Arc-sharing or
   dep-direction issue? Is the SAME registry instance threaded everywhere in serve/mcp (not two registries)?
   Does the InboundServer registry == the SessionManager registry == the backend registry (else status shows
   entries the handler can't resolve, or vice versa)?
8. **Concurrency.** cancel + permit + timeout racing the same key; inject while a turn is mid-checkout; two
   permits for different requests on the same turn; status read concurrent with resolve. Any deadlock (registry
   std::Mutex under the by_context tokio::Mutex), torn read, or lost wakeup?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` + a CONCRETE
fix. If you find NO blockers, say so explicitly and list the strongest residual risks. End with:
`WHOLE-BRANCH VERDICT: ship | fix-first`. Do NOT edit files.
