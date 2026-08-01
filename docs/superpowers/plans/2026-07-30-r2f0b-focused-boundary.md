# R2f0b focused implementation boundary — progress and terminal evidence

- **Status:** IN REVIEW — implementation candidate complete; deterministic/native verification and review pending
- **Frozen base:** `1a8cfc0020c0979b7a11724a7a39536dce41a680`
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Owner design:** [`../specs/2026-07-20-r2f-owner-design.md`](../specs/2026-07-20-r2f-owner-design.md)
- **Parent plan:** [`2026-07-11-r2f-phase-aware-liveness.md`](2026-07-11-r2f-phase-aware-liveness.md)

This document froze the source-implementation boundary before R2f0b source edits began; it remains the
review boundary for the IN REVIEW implementation candidate. It narrows, but does not replace, the approved owner
design and parent plan.

## Dogfood evidence

The exact frozen base and candidate were analyzed through the bridge by host Codex `gpt-5.6-sol`/`xhigh` under
the agent-native read-only sandbox. The completed analysis artifact was 44,929 bytes with SHA-256
`d498464aa054aee864947b804845c175c3fc3aac51ccf0c6ecf0d63c3cd77a0d` and ended
`R2F0B ANALYSIS: READY`.

One clean-room bridge design workflow then used independent Codex `gpt-5.6-sol`/`xhigh` and Claude `opus`/`xhigh`
lenses, refined each lens only against its own draft, and synthesized them. The completed synthesis was 33,438
bytes with SHA-256 `22806c52e89cf0716dc30c1cf9acbb35e36302de76c917feb1e44de2980a96db`.

The exact preflight bound host `@agentclientprotocol/codex-acp=1.1.7` to nested Codex `0.145.0`, and host
`@agentclientprotocol/claude-agent-acp=0.63.0` to Agent SDK `0.3.220` plus bundled Claude Code `2.1.220`. This is
workflow design evidence, not compatibility evidence. No live incident reproduction, quota exhaustion,
production-operator action, release, deployment, or baseline promotion occurred.

## Frozen decisions

1. **History attachment.** Prefer a compact ordered activity prefix plus saturating low-cardinality tally only if
   implementation proves the already charged 1,024-byte reclaimable attachment remains conservative, including its
   fixed schema/encoding overhead. Otherwise persist the tally-only fallback. Do not raise the slot charge, global
   cap, or row count in this slice.
2. **Clean stream EOF.** A direct-unary stream ending without `Done` or a structured error is a missing-terminal
   failure, never `TASK_STATE_COMPLETED`. This is an intentional correction to the currently blessed false-success
   behavior and requires a fail-first regression plus an explicit terminal negative control.
3. **Truthful missing evidence.** When v1 was negotiated but no envelope arrived, persist version `v1`, source
   `none`, completeness false, and producer/final unknown. Relax the existing coherence validator only enough to
   represent that truthful state; never claim source `adapter` for evidence the adapter did not supply.
4. **One numbered branch.** R2f0b remains one merge branch. The build order below is eight compile-correct stages;
   the bridge handoff may squash those stages into its single reviewed implementation commit.

No owner decision remains open for implementation.

## Complete boundary

R2f0b implements exactly four connected mechanisms:

1. a bounded attempt activity and meaningful-progress recorder driven by injected monotonic clocks;
2. a negotiated bridge-side `a2a_bridge.turn_evidence.v1` consumer and provider-free conformance fake;
3. one pure terminal resolver that keeps producer terminality, final-message presence, and bridge-owned ACP-child
   liveness independent; and
4. terminal failure publication after cleanup custody transfer but before cleanup settlement, closing the
   deterministic served-unary half of #47 without adding a client or provider timeout.

No new crate is required. Domain types live in `bridge-core`, ACP wire and ordered-turn handling in `bridge-acp`,
persistence in `bridge-store`, exact cleanup ownership in `bridge-coordinator`, and served/client projection in the
existing A2A inbound/outbound and CLI seams.

## Activity and meaningful progress

Add a synchronous, nonblocking, default-no-op `AttemptRecorder` port. A record contains only:

- bounded phase and reason enums;
- activity versus meaningful-progress kind;
- monotonic elapsed time relative to the attempt;
- a bounded numeric advance such as character count, token high-water mark, gate ordinal, or completed-set size.

Meaningful progress is a real phase transition, nonempty message/thought delta, component-wise usage high-water
increase, real tool-state transition, owned-child output growth, repository-state change ordinal, verification gate
start/reached-exit, completed-gate-set growth, or authoritative producer-terminal observation. Empty, duplicate,
decreasing, or non-advancing input is activity at most. Wall time identifies records and retention only.

Persist no prompt, agent text, tool/native id, path, command, raw process output, repository fingerprint, digest,
credential, or other unbounded/high-cardinality content. Overflow is sticky and explicit; it never changes the
primary workflow outcome. Stats and calibration exclude incomplete activity rather than imputing it.

The attachment schema migration is additive and uses the existing `PRAGMA table_info` plus conditional-ALTER
pattern so an existing R2f0a database receives the new fields. Existing rows remain valid and conservative.

## Terminal-evidence contract

Capability is explicit and generation-scoped. Package name or version never implies support. Existing adapters
that do not advertise v1 receive byte-identical initialize/prompt frames and remain usable with capability
`unsupported`, producer/final unknown, and incomplete terminal evidence.

For a supporting adapter, carry opaque attempt correlation only after successful capability negotiation. Reuse the
proven ordered `session/update` carrier with a reserved sentinel `agent_message_chunk`, empty text, and exactly one
`_meta` envelope. Bind every envelope to:

- generation;
- ACP session;
- bridge turn;
- attempt id;
- bridge-minted marker nonce;
- bounded adapter-native turn id and evidence sequence.

Accept one ordered v1 envelope before the prompt RPC resolves or rejects. An identical duplicate is idempotent. A
non-identical duplicate becomes a permanent conflict, not last-writer-wins. Missing, malformed, mismatched, late,
or conflicting evidence remains explicit and cannot be repaired by a later frame. A bounded closed-turn tombstone
may classify a late exact frame but cannot reopen or rewrite a terminal row.

On the consumer side, downgrade `final=absent` to unknown unless producer completion is authoritative and ordered
notifications were drained. Accept `final=nonempty` only from a same-turn `phase=final_answer` item or equivalent
native terminal field. Commentary, message id, stop reason, generic error, and process liveness never synthesize
producer or final facts.

The actual `codex-acp` producer mapping remains external. This repository may implement the consumer, types, fakes,
wire goldens, and conservative unsupported behavior, but must not claim selected-lane conformance or close the
null-final incident until that adapter explicitly advertises and proves v1.

## Terminal resolver

Extend the existing typed `AttemptTerminal`; do not create a parallel terminal authority.

- `completed + nonempty` is the only evidence-confirmed completed-final state.
- `completed + absent` is `protocol_incomplete_final`, even when the prompt RPC rejects.
- `completed + nonempty` with a rejecting RPC or missing deliverable final is a typed conflict/delivery failure.
- `failed` remains failed after any amount of commentary; `interrupted` remains interrupted.
- negotiated v1 with missing/malformed/mismatched/late/conflicting evidence yields the matching bounded protocol
  failure with sticky accepted-work uncertainty and no retry.
- an unsupported adapter with an errored accepted/uncertain prompt yields `protocol_terminal_unknown`, never
  success, proved process death, or retry; a normal non-v1 successful turn keeps its existing wire outcome while
  producer/final remain unknown.
- multi-provider-turn workflows keep attempt-level producer/final unknown and record bounded counts; exact evidence
  projects only when exactly one provider turn was reached.

Sample process liveness without consuming cleanup ownership: `try_wait == Ok(None)` is live, `Ok(Some(_))` is
exited, and error/no exact owned child is unknown. Name it bridge-owned ACP-child liveness, never provider or Codex
app-server liveness, and never let it decide producer disposition.

## #47 served-unary delivery inversion

Current ordering waits for warm cleanup before durable terminalization and response delivery. Replace that wait
only after proving exact cleanup custody transfer:

```text
backend terminal error
  -> observe terminal and seal evidence
  -> sample exact ACP-child liveness
  -> transfer exact warm cleanup flight to an observable detached owner
  -> terminalize durable failure with cleanup=pending
  -> return bounded JSON-RPC error data; submit prints locator/cause and exits nonzero
  -> detached owner later settles pending -> complete|failed exactly once
```

The custody path must reuse the existing exact claim/flight mechanism. If another owner already holds the flight,
record pending and do not claim completion. A history terminalization failure wins over the transient provider
cause because the cause has not become durable truth. No client total timeout is added.

Project the deepest already-sanitized `FailureDiagnostic` through one bounded wire DTO whose field set is locked to
the existing vetted redaction/debug surface. Raw summary/stderr never reaches the wire. `submit` keeps the already
printed execution/attempt locator and exits nonzero.

## Build order

1. Extract a generic ordered per-turn control channel and prove prefix-attestation frames remain byte-identical.
2. Add activity domain, recorder, accumulator, bounded attachment encoding, migration, overflow, and retention.
3. Thread attempt correlation and recorder producers through every direct/workflow/preflight surface.
4. Add typed terminal observation, truthful coherence update, ACP-child liveness sample, and clean-EOF rejection.
5. Add v1 capability negotiation, prompt correlation, ordered envelope carrier, tombstone, replay/conflict logic,
   and unchanged-frame goldens for unsupported adapters.
6. Add explicit detached cleanup custody, terminalize-before-response, post-terminal settlement CAS, bounded error
   data, and `submit` projection.
7. Project authoritative terminal evidence into direct and workflow outcomes, including multi-provider-turn counts.
8. Add verification/implementation progress instrumentation and the complete provider-free conformance matrix.

Each stage must compile before the next begins. Tests may be committed first and must be demonstrated red on the
frozen base before production behavior is changed.

## Required provider-free matrix

- #47 terminal error with cleanup held indefinitely; exact client receives the failure and exits nonzero while
  cleanup remains pending;
- commentary plus authoritative `completed/absent`, genuine error after the same commentary, and nonempty final
  gated on `phase=final_answer`;
- independent producer x final x ACP-child-liveness cross-product;
- capability absent, advertised, malformed, missing, malformed envelope, mismatch, late, identical duplicate, and
  conflicting duplicate;
- quiet live child, blocked child with output growth, and exited-child/wedged waiter without automatic action;
- silent healthy verification plus `NotReached`, gate start/exit, and completed-set growth;
- delivered structured provider limit and silent provider stream with cancel count zero;
- non-tool thought updates without persisting their content;
- failed root plus silent sibling with both states recorded and sibling cancel count exactly zero;
- activity overflow and attachment/store failure without primary-outcome rewrite;
- pre-migration database open, active-row capability conservatism, terminal/column coherence, retention cascade,
  exact settlement replay/conflict, and privacy sentinels;
- clean stream EOF without `Done`, plus explicit `Done` and structured-error negative controls.

Every positive behavior needs a same-path negative or edge case.

## Non-goals and exit boundary

R2f0b adds no production warning threshold, deadline, cancellation, retry, fallback, takeover, fan-out policy,
worktree preservation/deletion, session-close/debt policy, generation health action, stable ingress, live provider
reproduction, quota exhaustion, compatibility promotion, release, deployment, or operator mutation.

The generic progress/terminal mechanics for #24 may become complete, but the historical Kiro-specific alternative
remains open and unattributed. #47 is only closed for its deterministic terminal-delivery half here; R2f1b still
owns bounded execution/client liveness behavior, and R2f4 owns final provider-free closure plus any separately
authorized adapter conformance.

Before merge: run fail-first evidence, affected focused tests, format/diff, locked all-target/all-feature check,
warnings-denied Clippy, locked release build, repository hygiene, and the full serial workspace suite with exact
passed/failed/ignored totals. Name every live/provider/runtime/deployment path not exercised. Run a fresh
Sol/xhigh adversarial implementation review; use a second hard/complex lens only if that review exposes a qualifying
unresolved problem.
