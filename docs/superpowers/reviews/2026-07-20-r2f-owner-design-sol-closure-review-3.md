# R2f owner design — Sol closure review 3

- **Verdict:** `R2F OWNER DESIGN: REVISE`
- **Date:** 2026-07-20 local / 2026-07-21 UTC
- **Execution:** operator-served host Codex, read-only, one new clean-room turn; not a retry or resume
- **Requested/catalog-advertised identity:** raw `gpt-5.6-sol`, `xhigh`, `read-only`
- **Operator release:** `3c02bf3f419da8bc`
- **Adapter/CLI:** `@agentclientprotocol/codex-acp` 1.1.2 / `@openai/codex` 0.144.1
- **Prompt artifact:** owner-private temporary file, mode `0600`, SHA-256
  `c5a8c2349250b744623bfa9eda45c193eaa3971cfdb589015bcb83349e2c1c50`; deleted after this durable fold
- **Method:** no helper, edit, build, test, nested agent, nested provider/model turn, or production mutation by the
  reviewer; current source was inspected as needed and documentary gates were not treated as live proof

## Frozen boundary

`HEAD`, `origin/main`, and merge base were all `345941db91a7d898884bfe79e573433484ccafcc` on branch
`agent/r2f-owner-design`. The reviewer verified the same boundary both before and after semantic inspection: exactly
two modified and six untracked documentation paths, with no other changed path.

| Path | SHA-256 |
|---|---|
| `docs/reliability-execution-roadmap.md` | `b35ff5d692a40cb662a0fcfc677dad0ae080296b3ae5fbaa47a3992c28604761` |
| `docs/superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md` | `b1066e7abb5f025ab88aecb573cb8de44d8f43e5a07a0e1379da7f2aaf3c90c9` |
| `docs/superpowers/plans/2026-07-20-r2g-stable-ingress.md` | `e045470ac7af477fa16f8e8c81ae188016a6350e132a1532f2225874ebc45704` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-review-1.md` | `2623e40ea9f90c63262e596d73de19c745233f45760e3355a58b451c6dd463c6` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md` | `b30334196a52589cbfc37083ad196ffeafb67bb558ac1d4cdba3906741fef01c` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md` | `a81ffc6b7047440b6d514d5aa8569e60292cf00e7663d563ce4ee2401ab1c0d2` |
| `docs/superpowers/specs/2026-07-20-r2f-owner-design.md` | `a1a6aa1c30647f7ee2003828b200cfa81cbb39304d13e4071fa83655d596e2fc` |
| `docs/superpowers/spikes/2026-07-20-r2f-short-bound-validation.md` | `2ce34a150d45e303f4162d425fe1aecf324cccf5f21c314c595d7a6c6b89b484` |

The reviewer read `AGENTS.md`, the complete operator skill and routed trusted-host/read-only references, every frozen
file, and the relevant current unary, journal, ACP, registry, process, worktree, capacity, and SQLite source.

## Inherited adjudication

1. `FIXED` — the roadmap/design/plan/current-handoff cursor agrees literally.
2. `FIXED` — recovered `close_prepared` preserves still-unspent ordinal 0.
3. `FIXED` — a multiplexed process/shared container has one generation-scoped resource-action flight, while
   ordinary session close cannot signal it.
4. `FIXED` — preservation intent/claim publication precedes cancellation/process effect and protects both run-end
   and boot sweeps through resume or explicit disposition.
5. `FIXED` — considered alone, exactly-one telemetry-ledger selection and bounded no-fallback failures cover every
   surface and named initial-open failure.
6. `FIXED` — truthful advertised/configured/minimum/unknown capacity sources and create/close/generation-exit claim
   lifetimes are closed.
7. `PARTIAL` — producer completion, nonempty final-message presence, process liveness, sticky acceptance,
   `protocol_incomplete_final`, and no-retry behavior are closed; mandatory durable pre-prompt direct-unary evidence
   and its caller-visible identity channel were not.

## WRONG finding

### High — fail-open telemetry can repeat the null-final incident without durable evidence

Concrete state: `serve` validly uses an in-memory primary task store, the selected platform workflow ledger cannot
open because of permission, lock, migration, corruption, or I/O failure, and D6 permits primary execution to proceed
with no ledger row. A direct unary prompt is then accepted, emits progress, and ends with null-final
`task_complete`.

Incorrect result: the attempt has no durable attempt/turn evidence, violating the incident-derived contract and
recreating the accepted-work evidence loss. Refusing the prompt instead would contradict the unqualified D6
fail-open rule.

Required correction: give direct unary an explicit precedence rule. Reserve mandatory minimal safety evidence before
prompt effects or refuse pre-effect when that reservation is unavailable; keep optional summary enrichment separate
and reconcile D6, R2f0a, and the verification contract literally.

## SMELL finding

### Medium — pre-prompt caller-visible unary identity channel is unspecified

R2f0a requires execution/attempt ids before effects, while the current synchronous `submit` waits for the completed
JSON-RPC response and the server may substitute `task-1`. The design did not choose client-side mint-and-print,
immediate accepted-task response with reattachment, or another wire-visible mechanism.

## Verification boundary

- This was a source/design review only. No build, test, spike, provider, ACP session, container, takeover, or live
  operator behavior was executed by the reviewer.
- The 31/30-second and 6/5-second spike remained documentary evidence rather than an independently rerun gate.
- Close/idempotency, capacity, generation-wide action, process identity, preservation, telemetry-open failure,
  transport-loss, and null-final contracts remain implementation obligations.
- Full formatting, Clippy, release, hygiene, dependency-policy, and serial-workspace gates remain implementation
  closure work.

R2F OWNER DESIGN: REVISE
