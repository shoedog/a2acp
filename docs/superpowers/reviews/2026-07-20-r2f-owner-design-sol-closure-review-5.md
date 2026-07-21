# R2f owner design — Sol closure review 5

- **Verdict:** `R2F OWNER DESIGN: APPROVE`
- **Date:** 2026-07-20 local / 2026-07-21 UTC
- **Execution:** operator-served host Codex, read-only, one new clean-room turn; not a retry or resume
- **Requested/catalog-advertised identity:** raw `gpt-5.6-sol`, `xhigh`, `read-only`
- **Operator release:** `3c02bf3f419da8bc`
- **Adapter/CLI:** `@agentclientprotocol/codex-acp` 1.1.2 / `@openai/codex` 0.144.1
- **Prompt artifact:** owner-private temporary file, mode `0600`, 7,134 bytes, SHA-256
  `b6cc3a224e55ce49db7e0c58512ea807b67d0ebb670d1cf62c2b690c737f5fda`; deleted after this durable fold
- **Method:** no helper, edit, build, test, nested agent, nested provider/model turn, or production mutation by the
  reviewer; current source, pinned ACP crates/schema, and installed operator codex-acp were inspected as needed and
  documentary gates were not treated as live proof

## Frozen boundary

`HEAD`, local `origin/main`, and merge base were all `345941db91a7d898884bfe79e573433484ccafcc` on branch
`agent/r2f-owner-design`. The reviewer verified the same boundary before and after semantic inspection: exactly two
modified and eight untracked documentation paths, with no other changed path.

| Path | SHA-256 |
|---|---|
| `docs/reliability-execution-roadmap.md` | `375b2e42af8a71e05ce7cf45fc594d3ba5a068d40f799bea19cc88317454d3e8` |
| `docs/superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md` | `2ca1ca2ed97a2b3c6f1b7cbcd49a7eaa7c6457712816579f7d6a0bd67a75ee19` |
| `docs/superpowers/plans/2026-07-20-r2g-stable-ingress.md` | `e045470ac7af477fa16f8e8c81ae188016a6350e132a1532f2225874ebc45704` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-review-1.md` | `2623e40ea9f90c63262e596d73de19c745233f45760e3355a58b451c6dd463c6` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md` | `b30334196a52589cbfc37083ad196ffeafb67bb558ac1d4cdba3906741fef01c` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md` | `a81ffc6b7047440b6d514d5aa8569e60292cf00e7663d563ce4ee2401ab1c0d2` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-3.md` | `ff1b3c45721fa7a13722d4eb2e5ed3185038e5b7128df51bb8ffb7fdeb1dd0dc` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-closure-review-4.md` | `6eba86ec91ff747c4e2d3a776ccd75e0fc5f25a013dc23ede569ab373d1f4d12` |
| `docs/superpowers/specs/2026-07-20-r2f-owner-design.md` | `a2f8ef919dcee52c53d67198b93aef2ac19f40b5d3448be850e2a9c767695cde` |
| `docs/superpowers/spikes/2026-07-20-r2f-short-bound-validation.md` | `2ce34a150d45e303f4162d425fe1aecf324cccf5f21c314c595d7a6c6b89b484` |

The reviewer read the complete steering, operator skill and routed trusted-host/read-only references, every frozen
file, relevant workflow/unary/observation/journal/ACP/lifecycle/registry/process/worktree/capacity/SQLite/operator
source, pinned `agent-client-protocol` 1.0.1/schema 1.1.0, and installed codex-acp 1.1.2/Codex 0.144.1.

## Inherited adjudication

1. `FIXED` — `a2a_bridge.turn_evidence.v1` is a concrete negotiated wire contract: initialize advertises the exact
   version, prompt `_meta` carries opaque attempt correlation, and an ordered envelope binds generation, session,
   adapter-native turn, and attempt before prompt resolution or rejection. ACP prompt/capability `_meta` and its
   catch-all vendor-notification seam make the contract implementable.
2. `FIXED` — Codex producer disposition comes from native turn-terminal evidence; final `nonempty` comes only from a
   same-turn nonempty `final_answer` item or equivalent native terminal field; `absent` requires authoritative
   completion and ordered drain. Commentary, message ids, stop reason, generic errors, and process liveness cannot
   synthesize either fact. Installed codex-acp already has the required native terminal, phase, turn-id, and drain
   source seams even though it does not yet project the versioned extension.
3. `FIXED` — duplicate-identical, unsupported, advertised-but-missing, malformed, late, mismatched, conflicting,
   reordered, RPC-error, and transport-loss states are typed and conservative. Evidence applies durably before
   terminal publication; known `completed + absent` becomes `protocol_incomplete_final` even when the RPC rejects;
   no adverse case fabricates success, `AgentCrashed`, or retry.
4. `FIXED` — the proof boundary is honest: a fake proves only the state machine, unsupported adapters retain typed
   unknowns, and the Codex incident cannot close until the selected adapter advertises v1 and passes captured or
   separately authorized live conformance. Current codex-acp does not advertise v1, so no live closure is claimed.
5. `FIXED` — all closure-review-4 regression families remain closed: mandatory direct-unary core reservation;
   caller-minted/pre-network-visible ids; literal cursor agreement; unspent close ordinal 0; one shared-generation
   resource-action flight; crash-safe preservation/sweep exclusion; exactly-one ledger; and truthful capacity claims.

## New findings

No new `WRONG` or `SMELL` findings.

## Verification boundary

- The short-bound spike was accepted only for its stated documentary/local-mechanism scope.
- No build, test, provider turn, ACP session, production mutation, or live adapter conformance ran.
- Remaining work is implementation/release evidence: slices `0a` through `4`; fail-first and edge matrices; selected
  adapter modification/pin and captured or separately authorized Codex conformance; full format/Clippy/build/hygiene/
  workspace gates; authorized takeover/live lanes; provider-specific #24 disposition; and R2g stable ingress.

R2F OWNER DESIGN: APPROVE
