# R2f owner design — Sol closure review 4

- **Verdict:** `R2F OWNER DESIGN: REVISE`
- **Date:** 2026-07-20 local / 2026-07-21 UTC
- **Execution:** operator-served host Codex, read-only, one new clean-room turn; not a retry or resume
- **Requested/catalog-advertised identity:** raw `gpt-5.6-sol`, `xhigh`, `read-only`
- **Operator release:** `3c02bf3f419da8bc`
- **Adapter/CLI:** `@agentclientprotocol/codex-acp` 1.1.2 / `@openai/codex` 0.144.1
- **Prompt artifact:** owner-private temporary file, mode `0600`, 6,291 bytes, SHA-256
  `62f6bc21646d502e5ca82407051f8daf4b1811cf812c8febfb20f1928f10e831`; deleted after this durable fold
- **Method:** no helper, edit, build, test, nested agent, nested provider/model turn, or production mutation by the
  reviewer; current source and the pinned/operator ACP implementation were inspected as needed and documentary gates
  were not treated as live proof

## Frozen boundary

`HEAD`, `origin/main`, and merge base were all `345941db91a7d898884bfe79e573433484ccafcc` on branch
`agent/r2f-owner-design`. The reviewer verified the same boundary before and after semantic inspection: exactly two
modified and seven untracked documentation paths, with no other changed path.

| Path | SHA-256 |
|---|---|
| `docs/reliability-execution-roadmap.md` | `f72fa4340865a7e786e83e989b1afcfe99a29319c2e00fe6a058abdbbd91eae5` |
| `docs/superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md` | `d92b5f51d6274e6abcdb65aab120bf69f967473f82d7da29991b816879ff700a` |
| `docs/superpowers/plans/2026-07-20-r2g-stable-ingress.md` | `e045470ac7af477fa16f8e8c81ae188016a6350e132a1532f2225874ebc45704` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-review-1.md` | `2623e40ea9f90c63262e596d73de19c745233f45760e3355a58b451c6dd463c6` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md` | `b30334196a52589cbfc37083ad196ffeafb67bb558ac1d4cdba3906741fef01c` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md` | `a81ffc6b7047440b6d514d5aa8569e60292cf00e7663d563ce4ee2401ab1c0d2` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-3.md` | `ff1b3c45721fa7a13722d4eb2e5ed3185038e5b7128df51bb8ffb7fdeb1dd0dc` |
| `docs/superpowers/specs/2026-07-20-r2f-owner-design.md` | `0f1fe5554b9f0136f699525d1780767b68cee22e539a4b82f4715f7c541ee776` |
| `docs/superpowers/spikes/2026-07-20-r2f-short-bound-validation.md` | `2ce34a150d45e303f4162d425fe1aecf324cccf5f21c314c595d7a6c6b89b484` |

The reviewer read `AGENTS.md`, the complete operator skill and routed trusted-host/read-only references, every frozen
file, and relevant workflow, unary, observation/journal, ACP, registry, process, worktree/sweep, capacity, SQLite,
and operator source.

## Inherited adjudication

1. `FIXED` — direct unary must reserve its minimal core row in the one selected ledger before effects; initial open
   or reservation failure is a typed refusal, while only optional workflow summaries and later unary enrichment
   remain fail-open.
2. `FIXED` — the caller mints and pre-network prints validated high-entropy execution/attempt ids; missing, invalid,
   and colliding ids refuse before effects, with no `task-1` substitution or duplicate-locator replay.
3. `PARTIAL` — the durable state model correctly separates producer terminal, final-message presence, and process
   liveness and maps known producer completion plus absent final to `protocol_incomplete_final`; the current ACP seam
   cannot authoritatively supply the first two facts.
4. `FIXED` — the roadmap/design/plan/current-handoff cursor agrees literally.
5. `FIXED` — recovered `close_prepared` retains still-unspent ordinal 0.
6. `FIXED` — shared process/container escalation joins one generation-scoped resource-action flight; ordinary
   session close cannot signal it.
7. `FIXED` — preservation publication precedes cancellation/process effects and protects run-end and boot sweeps.
8. `FIXED` — exactly-one telemetry-ledger selection and bounded no-fallback failure behavior covers every surface.
9. `FIXED` — truthful session-capacity sources and create/close/generation-exit claim lifetimes are closed.

## WRONG finding

### High — current ACP observations cannot distinguish null-final producer completion from a live-process turn failure

Constructible state: a direct-unary prompt is durably admitted and accepted; the agent emits commentary chunks; the
underlying Codex producer emits `task_complete` with `last_agent_message: null`; and the ACP process remains live.
The same bridge-visible text-plus-error-plus-live-process shape is constructible when accepted work emits commentary
and then suffers a genuine per-turn SDK error.

Current `Update` exposes undifferentiated text and terminal stop reason only. `bridge-acp` discards ACP message id and
metadata. The pinned ACP schema has streamed `AgentMessageChunk` and `PromptResponse.stop_reason`, but no standard
producer-terminal or final-message-presence field. The operator codex-acp does internally observe Codex
`turn/completed` and phase-tagged assistant response items, yet it does not project those facts through ACP; its
prompt response metadata contains quota only.

Incorrect result: treating any prior text as final misclassifies commentary-plus-null-final as success; mapping the
terminal SDK error normally repeats false `AgentCrashed`; inferring completion from text, error, and live process
misclassifies the genuine per-turn-error alternative. Those states are observationally indistinguishable at the
current bridge seam.

Required correction: R2f0b must define a negotiated, versioned adapter/wire terminal-evidence extension that carries
producer disposition and explicit nonempty-final presence independently, bound to the exact turn/attempt. It must
also define missing, unsupported, malformed, and conflicting evidence without fabricating either value. The Codex
lane then needs adapter-conformance evidence before the incident contract can close.

## SMELL findings

None.

## Verification boundary

- This was a source/design review only. No build, test, provider turn, ACP session, container, takeover, or live
  operator behavior was executed by the reviewer.
- Prior reports were used only for inherited adjudication. The short-bound spike remained documentary evidence and
  the owner-approved 31/30-second and 6/5-second bounds were not reopened.
- All R2f source slices, provider-free matrices, fail-first/edge tests, full workspace/release/hygiene gates, adapter
  conformance, separately authorized takeover, and provider-specific #24/live null-final evidence remain unexecuted.

R2F OWNER DESIGN: REVISE
