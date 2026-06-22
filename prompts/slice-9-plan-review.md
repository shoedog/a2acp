You are doing a rigorous, adversarial PLAN REVIEW (read-only) of the **Slice 9 — Turn Channel + E2 permission**
implementation plan for the a2a-bridge (a Rust A2A↔ACP bridge + orchestrator). The plan must be executable by an
engineer with zero context: exact files, real types, TDD steps, no placeholders. Your job: find decomposition
errors, missing/incorrect code, type inconsistencies, untestable steps, wrong file:line anchors, and any
SPEC-FIX (SF-1..9) the plan fails to realize — BEFORE implementation. READ-ONLY: read the plan + spec + the real
code; do NOT edit/build/test.

The plan: `docs/superpowers/plans/2026-06-22-slice-9-turn-channel-e2.md`. The BINDING spec (its `## v2` section,
SPIKE-1 RESOLVED + SF-1..9 + D1-D5): `docs/superpowers/specs/2026-06-22-slice-9-turn-channel-e2.md`. SPIKE-1 was
confirmed: codex `approval_policy="untrusted"` + `sandbox_mode="read-only"` + a sandbox-blocked write emits a
real `session/request_permission` (shape in the spec v2). Foundation: [[cancel-tokens-shipped]] (merged).

{{input}}

GROUND every finding in real code with `file:line`. Pressure-test:

1. **SF coverage.** Does each task realize its SF? Specifically: SF-1 — is cancel-resolves IMMEDIATE (the
   handler `select!`s on the registry rx + a timeout, NOT `turn_kill`; and `SessionCancel`/release/clear/reset
   call `resolve_context` synchronously)? Verify the plan's P1 (does the A2A `SessionCancel`/`SessionClear` path
   at `server.rs:2952` go through a Coordinator wrapper that can resolve, or call `SessionManager` directly?) —
   is the plan's wiring actually correct, or is the registry unreachable from the real cancel/clear handlers?
   SF-2 gen-keyed `PermKey` — is the generation/op available everywhere it is keyed? SF-5 — does the plan touch
   ALL THREE producer part-assembly sites (Coordinator `collect_turn:215` AND the streaming
   `spawn_local_producer` AND the unary Local `:2311`), and is `LocalDispatch.seed` actually the seam they use?
   SF-7 — is the defaulted `interactive_decide` truly zero-change for the 14 PolicyEngine impls?

2. **Task 4 risk (the least-pinned).** Is the bridge `ContextId` reliably available at ACP route-registration
   time (`acp_backend.rs:1986` + the prompt path)? Trace where the route map is populated and whether the
   producer knows `{ctx, gen, op}` there. If NOT, the plan understates Task 4 — say exactly what threads it.

3. **Type/signature consistency across tasks.** `PermissionDecision` (the match-site sweep — does Task 1 list
   EVERY `PermissionDecision::Approve` match site that breaks, e.g. `acp_backend.rs:1271`?); `PermissionResolution`
   vs `PermissionDecision` (the oneshot carries Resolution, the registry resolve takes Resolution, `resolve_
   context` only ever Cancelled — is that consistent + does `resolve_context` need Clone?); `assemble_turn_parts`
   signature stable; `PermKey` fields. Any function/type used before defined?

4. **Registry exact-once + drop-guard correctness.** Does `register` return a guard that reaps on EVERY handler
   exit? Is `resolve`/`resolve_context` an atomic take-under-one-lock (no double-send, no send-after-take)? Does
   a stale-generation `resolve` correctly no-op? Compare to the cancel-tokens oneshot rigor — any leak/orphan.

5. **Detached panic (SF-6).** Is `frame_from_orch` (`detached.rs:398`) the right + only place to skip the new
   event, or does the flush path (`:88/:131`) ALSO need an arm? Will the new event reach the detached sink at
   all (it's published per-turn via the RichEventSink) — is journal-only correct?

6. **TDD step soundness.** Are the failing tests REAL (compile against the proposed types, assert the behavior,
   fail before impl)? Any test that would pass for the wrong reason? Is the `manager()` test helper / the
   `finish_turn_by_ctx` helper the plan references real (or does it need adding)? Are the live-gate steps
   actually reachable (the inject codeword recall; the permit deny/approve observable effect; cancel-mid-
   permission)?

7. **Scope / ordering / missing.** Is the 8-task order right (deps)? Anything IN that should be OUT (or missing)?
   Is the producer-join residual (SF-8) correctly NOT a task? Are P1-P4 the right open items, and do you have
   concrete answers for any?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` or task anchor +
a CONCRETE fix. Give a concrete answer/recommendation for each of P1-P4. End with: `PLAN VERDICT: ready-to-
implement | needs-revision`. Do NOT edit files.
