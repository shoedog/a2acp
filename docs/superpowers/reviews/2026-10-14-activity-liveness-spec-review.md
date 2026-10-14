# Activity liveness design — spec review ledger

Cap: two Sol/xhigh rounds (`code-review` workflow, hard read-only), declared before round 1.

## Round 1/2 — artifact `6df1a262` (revision 1) — REJECT

Five WRONG·MATERIAL blockers, five SMELL·MATERIAL·DEFER; closed population, bounded fixes. All folded in revision 2.

| # | Finding | Disposition |
|---|---|---|
| W1 | `u64::MAX` scheduler cutoff can overflow `Instant` and panic | Folded: armed cutoff stays `Option`; no cutoff future when `None`; `u64::MAX` forbidden |
| W2 | "Watched" frozen at admission; a served watched run outlives its client unarmed | Folded by narrowing (controller refinement, no lease): watched only where the run dies with its client (local `run-workflow`; served unary with proven cancel-on-disconnect); served/detached workflows always unwatched by default |
| W3 | No carrier for the execution-id board key in local workflows | Folded: non-persisted `WorkflowRunContext.observer_key`, copied into the three `TurnMeta` sites; `executor.rs` added to I3 |
| W4 | Liveness config/board do not reach `container_rw`'s inner `AcpConfig` | Folded: `ContainerRwConfig` carries both; `bridge-container` added to I1/I3 |
| W5 | Workflow overrides lack a frozen tri-state representation | Folded: `Option<LivenessOverrideV1 { Off, Ms }>` for silence and work cutoff in TOML, invocation and CLI |
| S1 | Request durations unbounded | Folded: one checked ≤30-day parser for every source |
| S2 | I1 alone could change manual workflow turns | Folded: I1 writes `Off`/`Off` at workflow sites until I2 |
| S3 | Tracker tests miss id collision, concurrent permissions, unpause notify | Folded: I1 tests 14-16 |
| S4 | Board multiplicity/teardown unspecified | Folded: `(observer_key, turn_id)` RAII registration; fan-out tests |
| S5 | Usage key ignores optional cost | Folded: `Option<(amount bits, currency)>` key; tests |

## Round 2/2 — artifact `168eec1f` (revision 2) — REJECT

Five WRONG·MATERIAL blockers, three SMELL·MATERIAL and one SMELL·IMMATERIAL. All ten round-1 dispositions confirmed
incorporated; the W2 narrowing was judged sound.

**Classification at the cap.** Not converging overall (5 → 5). Two blockers (W1 cleanup tail, W3 admission
budgets) are new instances of the same class as round-1 W1: consumers of the workflow work cutoff that break
when it is unarmed. That part (the watched-workflow hard-cap opt-in, now I2b) is **open-class: parked**,
pending a complete consumer enumeration before any further review. The other findings are closed and bounded
and are folded into revision 3 for I1, I2a and I3; those increments proceed under their own two-round
source-review caps (disclosed to the owner; no third spec round).

| # | Finding | Disposition (revision 3) |
|---|---|---|
| W1 | Unarmed watched runs lose the 60 s cleanup tail after the nominal cutoff | Parked with I2b (open class) |
| W2 | Sticky tool overflow disables silence after all tools close | Folded: digest-keyed set of 4096 simultaneous entries, slots freed on close; overflow only beyond 4096 concurrently open |
| W3 | Explicit duration vs admission validators use different budgets | Parked with I2b (open class) |
| W4 | A stalled liveness RPC stops the heartbeat | Folded: per-poll deadline min(10 s, interval/2), elapsed-only on timeout |
| W5 | I2 and I3 not independent | Folded: workflow `client_watching` moves to parked I2b; I2a and I3 depend only on I1 |
| S1 | Raw tool ids not byte-bounded | Folded: SHA-256 key + 64-byte preview |
| S2 | Heartbeat titles not escaped | Folded: UTF-8-safe truncation then JSON-style escaping |
| S3 | Served-workflow observer key seam missing | Folded: coordinator.rs/detached.rs in I3, served-workflow test |
| S4 (IMMATERIAL) | Equal-instant deadline cause unspecified | Folded: fixed priority watchdog > work > silence |
