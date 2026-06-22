# Slice 9 — Turn Channel + E2 permission — HANDOFF / Resume Doc

> Resume the LAST core orchestration slice. Architect phase DONE; spec dual-reviewed → **needs-respike**.
> Branch `feat/slice-9-turn-channel-e2` (base = `main` `e4e12f0`). Read top-to-bottom.

## Where this sits
- The prerequisite ([[cancel-tokens-shipped]]) is SHIPPED+MERGED+PUSHED (`main` merge `12e3816`). Slice 9 is
  UNBLOCKED and is the last core slice (then the Slice-10+ tail: B2 fan-out · E1 worktree · E6 retry · etc.).
- **Spec written + committed** (`907a8c6`): `docs/superpowers/specs/2026-06-22-slice-9-turn-channel-e2.md`.
  Two parts: **A) queued-inject** (mirrors `pending_seed`; low-risk, NO spike needed) + **B) E2 permission**
  (the spike-heavy half).
- **Dual spec-review DONE** (codex-xhigh `1b0ecd9` + Opus lens). **Verdict: needs-respike.** Findings in
  `/tmp/slice-9-spec-review.out` (transient — re-run the workflow to regenerate).

## The keystone: SPIKE-1 (do this FIRST — it gates the E2 half)
**E2 is NOT reachable under current configs.** Today the ACP handler auto-answers every `session/request_
permission` (`acp_backend.rs:1051`/`decide_permission:1227`); the default policy approves
(`bridge-policy/src/permission.rs:15`); dogfood codex runs `approval_policy="never"`. So a real interactive
permission NEVER fires live. **SPIKE-1 = empirically confirm a real permission arrives** with: codex `-c
approval_policy="on-request"` (or `"untrusted"`) + a tool prompt that requires approval (a write/exec the
sandbox would gate), OR claude-agent-acp permission mode `default`; + a bridge policy that returns `Defer`.
Log the raw `RequestPermissionRequest` (options/tool_call/raw_input) and PIN the event shape to real traffic.
If no config makes it fire, the E2 design/DoD must change — hence "respike before plan".

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

## NEXT (resume HERE)
1. **Run SPIKE-1** (empirical, the keystone). Author a `Defer` policy + an interactive-approval codex config;
   drive a permission-triggering turn; log the raw request; pin the shape. (De-risks the whole E2 half.)
2. **Fold the spike + the 13 findings into spec v2** (a `## v2` section, like cancel-tokens). Re-review.
3. **Then** plan → dual plan-review → 7 TDD tasks → whole-branch review → live-gate → merge.
   (Consider SPLITTING: ship Part A queued-inject first — low-risk, no spike — and gate Part B on SPIKE-1.)

## Proven loop + scaffolding (reuse)
- codex-HIGH implements (no commit) / Opus verifies+commits / codex-xhigh reviews / live-gate vs real codex.
- Spec-review tooling committed (`1b0ecd9`): `prompts/slice-9-spec-review.md` +
  `examples/a2a-bridge.slice-9-spec-review-codex.toml` (port 8123). Mirror for plan/whole-branch (ports 8124+).
