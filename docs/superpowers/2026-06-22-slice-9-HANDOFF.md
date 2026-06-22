# Slice 9 — Turn Channel + E2 permission — HANDOFF / Resume Doc

> Resume the LAST core orchestration slice. **STATUS: architect DONE (spec v2, SPIKE-1 ✅) + PLAN v3 DONE
> (dual-review folded, scope confirmed) → READY-TO-IMPLEMENT.** Branch `feat/slice-9-turn-channel-e2`
> (base = `main` `e4e12f0`). Read top-to-bottom. Docs-only so far — NO production code written yet.

## ⏯️ RESUME POINT: the plan is locked — START IMPLEMENTING (v2.T1)
- **Plan = `docs/superpowers/plans/2026-06-22-slice-9-turn-channel-e2.md`**: read the **`## v2`** (BINDING) +
  **`## v3`** (BINDING, supersedes v2 where it conflicts) sections — they are the implementation spec.
  History: draft (`5a04b6f`) → v2 fold of codex-1 plan-review + verification (`1e5e5a6`) → v3 reconciliation of
  the Opus architecture lens + codex-2 2nd pass (`ac96379`) → scope confirm (warm-only).
- **Both reviews (Opus lens + codex-xhigh 2nd pass) = needs-revision → ALL findings folded into v3.** The five
  v2 decisions stand; no re-architecture. Transient review outputs: `/tmp/slice-9-plan-review-v2.out` (codex-2);
  the Opus lens is captured in the conversation + v3.
- **SCOPE CONFIRMED: WARM-ONLY interactive permit.** Detached-node interactive permit + push/SSE visibility +
  per-agent Defer = TRACKED deferrals (see v3 "Deferred — TRACKED"). Detached nodes still get `configure_turn`
  so a Defer there TIMES-OUT to default-deny (no hang).
- **NEXT: implement task-by-task** v2.T1 → T2 → T3 → T4 → T5 → T5b → T6 → T7 → T8 → T9 → T10. Proven loop:
  codex-HIGH writes (no commit) / Opus verifies in the clean host env + commits / codex-xhigh whole-branch
  review / live-gate vs DIRECT codex (`approval_policy="untrusted" sandbox_mode="read-only"` + `[server]
  permission_policy="defer"`, port 8125). Reuse a `cancel-tokens-impl`-style codex-HIGH config (danger-full-
  access, NO commit). Stage ONLY each task's files (worktree has many unrelated untracked `examples/*.toml` /
  `prompts/*.md` + `M examples/a2a-bridge.slicing-analysis.toml` — never fold them).
- **The single most critical impl detail** (Opus, verified): `resolve_context_cancelled` fires AFTER the
  `is_claimed`/`Cancelling` guards in `cancel_inner`/`release_inner`/`reset_session_inner` (NOT at the top) —
  else cancelling a `Compacting`/`Resetting` ctx poisons its own summarize permission → EXPIRE → data loss.

## Where this sits
- The prerequisite ([[cancel-tokens-shipped]]) is SHIPPED+MERGED+PUSHED (`main` merge `12e3816`). Slice 9 is
  UNBLOCKED and is the last core slice (then the Slice-10+ tail: B2 fan-out · E1 worktree · E6 retry · etc.).
- **Spec written + committed** (`907a8c6`): `docs/superpowers/specs/2026-06-22-slice-9-turn-channel-e2.md`.
  Two parts: **A) queued-inject** (mirrors `pending_seed`; low-risk, NO spike needed) + **B) E2 permission**
  (the spike-heavy half).
- **Dual spec-review DONE** (codex-xhigh `1b0ecd9` + Opus lens) → was needs-respike; **SPIKE-1 ran + all
  findings folded into spec v2** (`98c85f5`, the `## v2` section — BINDING). Spec is ready-to-plan.

## The keystone: SPIKE-1 — ✅ DONE (E2 FEASIBLE; shape pinned)
**Confirmed empirically** (temporary `eprintln` probe in the `acp_backend.rs` permission handler, REVERTED):
codex-acp with **`-c approval_policy="untrusted"` `-c sandbox_mode="read-only"`** + a turn attempting a
sandbox-blocked **write** ("create /tmp/x.txt") emits a real `session/request_permission`. (`="never"`
auto-runs → no ask; that is why dogfood never saw it.) Shape captured (`/tmp/ct-lg/spike1_shape.txt`):
`tool_call: ToolCallUpdate{tool_call_id, kind:Execute, title:<cmd>, raw_input:{command,cwd,…}}` + `options[]`
using STANDARD ACP kinds (`approved`/AllowOnce, `approved-execpolicy-amendment`/AllowAlways, `abort`/RejectOnce)
→ the existing `decide_permission` mapping works UNCHANGED; `Modify`=select-offered-option validated. Folded
into **spec v2** (`## v2` section, BINDING, `98c85f5`). Live-gate config pinned (untrusted+read-only + a Defer
policy + a write-prompt).

## Spec-review findings to FOLD into spec v2 (after the spike)
BLOCKERS:
1. **SPIKE-1** (above) — pin the harness before implementation.
2. **Cancel-resolves must be IMMEDIATE**, not via `turn_kill` (which only fires after the grace timeout, and
   `cancel_inner` doesn't fire the warm abort on keep-warm cancel). `SessionCancel`/release/clear/reset must call
   `PermissionRegistry::resolve_context(ctx, Cancelled)` directly; `turn_kill` stays a backstop only.
3. **Gen-unsafe keying** — key the pending oneshot by `{context_id, generation, op, request_id}` (mirror the
   `finish_turn` gen+op guard, `session_manager.rs:581`); `SessionPermit` must echo gen/op and reject stale.
4. **Route lacks context** — the reverse handler only has `req.session_id`; routes carry only `tx`+`watch`
   (`acp_backend.rs:1060/1986`). Thread bridge ctx/gen/op into the route/registry AT CHECKOUT — don't parse it
   back out of formatted session ids.
MAJORS:
5. Registry **atomic remove-on-resolve + drop-guard** on EVERY exit (decision/timeout/cancel/responder-fail/
   task-drop) + sweep on handle release/finish.
6. **Inject must thread through the A2A producers too** — the streaming + unary Local producers compose their
   OWN parts from `LocalDispatch.seed` (`server.rs:1376/2311`); `LocalDispatch` has `seed` but no injects.
   Add injects to `WarmTurn`+`LocalDispatch` and use ONE helper `seed→prepend→input→append` in Coordinator
   `collect_turn` AND both A2A producers. (Same producer-multiplicity lesson as cancel-tokens.)
7. **`frame_from_orch` panics** on any `OrchEventKind` outside plan/tool-call/update (`detached.rs:398`); the new
   `PermissionRequest` event must get a real frame + `task watch` rendering OR be explicitly journal-only (skip
   frame conversion). Don't let the detached sink panic.
8. **Byte-identical dead-safe** — constrain the trait migration: default policies can't `Defer`; the auto branch
   responds BEFORE event/register; define how API/translator treat `Defer`. **Opus refinement:** add a
   DEFAULTED optional trait method (`fn decide_interactive(..) -> Option<..> { None }`) so the **14 existing
   `PolicyEngine` impls need ZERO changes** (dead-safe by construction, not by audit) — do NOT change
   `decide()`'s return type.
9. **Distinct cancel resolution value** — the oneshot needs an internal `PermissionResolution::{Decision,
   Cancelled}` (don't overload Deny).
10. **Producer-join deferral** — track the residual single-slot re-mint race EXPLICITLY (don't claim it away);
    cancel-resolves-permission is independent of it (resolves the oneshot registry), but say so precisely.
MINORS: 11. cap inject queue count+bytes; dedupe = replace-in-place (FIFO). 12. D3 split: clear DROPS injects,
compact PRESERVES-after-seed OR rejects-while-pending (compact already rejects a pending seed,
`session_manager.rs:1010`). 13. Decisions: D1=`PolicyOutcome::Defer` (separate, defaulted); D2=dedicated
`InjectParams`/`PermitParams` (OpParams is prompt-shaped, requires input); D4=`Escalate` non-functional in-slice
(must not consume the pending sender); D5=allow inject-while-Running (queue for next checkout).

## Phase status
- Architect: ✅ spec v2 (`98c85f5`), SPIKE-1 ✅.  · Plan: ✅ written (`5a04b6f`).  · Plan-review: ⏳ IN FLIGHT
  (see the RESUME POINT at the top — read `/tmp/slice-9-plan-review.out`, add an Opus lens, fold → plan v2).
- Then: TDD-implement per task → whole-branch dual-lens review → live-gate → merge. (User chose the FULL slice —
  inject + permission together — not the split.) Live-gate shape: inject lands next turn; a real permission
  surfaces + routed Deny blocks / Approve allows; cancel mid-permission ends promptly. Config = untrusted +
  read-only codex + a `Defer` policy.

## Proven loop + scaffolding (reuse)
- codex-HIGH implements (no commit) / Opus verifies in the clean host env + commits / codex-xhigh reviews /
  live-gate vs real codex. (codex sandbox hits the `_dyld_start` flake → controller re-verifies.)
- Scaffolding committed: spec-review (`1b0ecd9`, port 8123) + plan-review (port 8124,
  `prompts/slice-9-plan-review.md` + `examples/a2a-bridge.slice-9-plan-review-codex.toml`). For implementation,
  reuse a `cancel-tokens-impl`-style codex-HIGH config (danger-full-access, NO commit); whole-branch review next
  free port. Stage ONLY each task's files (the worktree has many unrelated untracked `examples/*.toml` /
  `prompts/*.md` + the pre-existing `M examples/a2a-bridge.slicing-analysis.toml` — never fold them).
