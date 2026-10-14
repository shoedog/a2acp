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
