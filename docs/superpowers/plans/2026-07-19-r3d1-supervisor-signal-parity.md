# R3d1 — supervisor and signal-parity implementation plan

- **Status:** IN REVIEW
- **Branch:** `agent/reliability-r3d1-supervisor`
- **Base:** `origin/main` at `c2d147fb1f0df275f3c6452cdd212e185c002d08`
  (PR #38 merged R3d0)
- **Design of record:** the R3d supervision and signal contract in
  [`2026-07-11-r3-compatibility-canaries.md`](2026-07-11-r3-compatibility-canaries.md)
- **Effects:** non-billable. Tests may spawn local fake child processes. This slice does not read
  credentials, discover models, contact a provider, access a container runtime or registry, publish to
  GitHub, install a timer, issue authority, or touch the long-lived operator.

## Delivery boundary

R3d1 delivers the reusable supervision mechanism that R3d2 can place around an admitted compatibility
aggregate. It does not implement R3d2 admission, provider-effect authority, preflights, owner-wide locking,
or accounting, and it does not implement R3d3 evidence retention/publication. In particular:

- `compatibility schedule-tick` becomes a recognized parent entrypoint, captures its monotonic process-entry
  origin before dispatch, and refuses with a typed R3d2-not-implemented result before credentials or any
  provider-capable spawn. It is not advertised as an executable scheduler until the R3d2 path exists.
- Deadline derivation is a checked, versioned, hashable contract. One in-memory absolute monotonic deadline is
  computed from process entry; phase derivation time consumes the same bound; absent phases contribute zero;
  overflow or an insufficient schedule/grant/accounting window refuses.
- The R3c retained group-leader anchor is factored into one reusable primitive. The resolver keeps its existing
  behavior through that primitive. R3d1 adds TERM, exact anchor identity checks, an anchor for an existing
  same-session descendant group, journal-before-terminal-signal ordering, and final release/reap only after no
  later group signal is allowed.
- The supervisor is a deterministic state machine over exact process identities. Production Unix process
  inspection supplies PID, parent PID, process group, session, and a platform start marker; fake backends drive
  all cancellation, wedge, recycled-PGID, recovery, and unrelated-process regressions.
- The parent supervisor record and child aggregate reference join on exact run/window identifiers and hashes.
  A missing or mismatched child artifact cannot become a successful parent record.
- An anchor-acquisition/liveness ambiguity, new-session escape, unproved survivor, or crash ambiguity becomes a
  durable safety hold. Recovery never retries a numeric process-group signal when the prior terminal-signal
  ordering is uncertain.

## Implementation tasks

### Task 1 — reusable anchored process groups

Files:

- add `bin/a2a-bridge/src/compatibility_process_group.rs`
- edit `bin/a2a-bridge/src/compatibility_resolution.rs`
- edit `bin/a2a-bridge/src/main.rs`

Work:

1. Factor the R3c `CommandProcessGroupGuard` without weakening resolver cleanup.
2. Retain the anchor child and stdin capability until explicit final release/reap.
3. Record the anchor's exact process identity immediately after spawn.
4. Allow only TERM/KILL against a group whose retained anchor identity still matches.
5. Add same-session `anchor_existing_group`; mismatch, vanished group, or acquisition failure returns hold-worthy
   failure without signaling.
6. Make repeated finalization and post-terminal cancellation no-ops.

Required tests:

- the short-lived resolver leader remains protected from PGID reuse;
- TERM reaches a fake workload while the anchor remains live;
- release before a later signal is rejected;
- an existing same-session descendant group can be anchored, while a new-session group cannot;
- anchor identity mismatch/death refuses a group signal;
- final release/reap removes the anchor and repeated finalization performs no second signal.

### Task 2 — deadline and supervisor record contracts

Files:

- edit `bin/a2a-bridge/src/compatibility_schedule_schema.rs`
- edit `docs/compatibility-scheduling-foundation.md`

Work:

1. Add strict `DeadlineDerivationV1`, phase-budget, exact-process-identity, anchored-group, child-artifact-reference,
   and `SupervisorRecordV1` schemas.
2. Add `deadline-derivation` and `supervisor` validator kinds.
3. Validate bounded checked sums, canonical phase coverage/order, elapsed derivation, containment windows, exact
   identity/group consistency, monotonic phase transitions, terminal-signal journal state, safety-hold reasons,
   and parent/child run/window/hash joins.

Required tests:

- a complete derivation validates and any single phase, sum, elapsed, or containment mismatch fails;
- absent phases are explicit zero and cannot hide allowance;
- overflow and deadline already consumed fail closed;
- successful/cancelled/killed/held records accept only their exact required fields;
- a parent record with the wrong run, window, aggregate hash, or child artifact hash is rejected;
- unknown fields, unknown states, stale group identities, and post-terminal signaling permission are rejected.

### Task 3 — supervisor state machine and recovery

Files:

- add `bin/a2a-bridge/src/compatibility_schedule_supervisor.rs`
- edit `bin/a2a-bridge/src/main.rs`

Work:

1. Derive one hard absolute deadline from the captured monotonic process-entry origin.
2. Persist state before each effectful transition through an injected journal interface.
3. Register the anchored runner group and exact descendant identities; acquire retained same-session anchors for
   descendant-created groups or enter hold.
4. On first cancellation/deadline, journal and TERM the runner, then begin the bounded grace period.
5. On a second cancellation during grace, or grace expiry, journal the one terminal KILL attempt before signaling
   only still-anchored exact groups.
6. Reap direct workload children, prove no non-anchor group member, mark that no later group signal is permitted,
   release/reap anchors, and prove absence. Otherwise retain hold/admission ownership for R3d2.
7. On startup, reconcile incomplete records: resume only provably pre-signal work; terminal-signal ambiguity or
   missing anchors holds and never retries a numeric PGID.
8. Emit a terminal parent artifact whose child reference joins by run/window and optional aggregate hash.

Required fake-process tests:

- ignored TERM escalates to KILL;
- SIGSTOP/uninterruptible observation remains held;
- exited runner plus a surviving descendant group is contained;
- publication wedge reaches the same absolute deadline;
- repeated cancellation escalates once and later cancellation sends nothing;
- unproved exit keeps a safety hold;
- startup recovery before TERM may continue, while crash immediately before/after terminal-signal journal holds
  with no stale retry;
- the recycled-PGID mutation demonstrates an unrelated group can be hit only if the anchor is prematurely
  released; the production ordering keeps the anchor and unrelated group alive;
- unrelated processes and exact-label-unrelated containers are never targeted.

### Task 4 — SIGTERM parity and fail-closed parent

Files:

- edit `bin/a2a-bridge/src/compatibility.rs`
- edit binary CLI integration tests

Work:

1. Replace the one-shot Ctrl-C waiter with one Unix shutdown-signal helper that treats SIGINT and SIGTERM
   identically and keeps the current aggregate cancellation semantics.
2. Recognize `compatibility schedule-tick`, capture its process-entry monotonic origin before reading scheduling
   inputs, and refuse before effects until R3d2 provides authority/admission.
3. Keep `validate`, `resolve`, `run`, and `compare` behavior and acknowledgement boundaries unchanged.

Required tests:

- injected SIGINT and SIGTERM both set the same cancellation state;
- the next case is not admitted and the active floating observation is downgraded on either signal;
- repeated signals do not create multiple cancellation transitions;
- `schedule-tick` is recognized but cannot read a credential or spawn a provider-capable child in R3d1;
- existing compatibility help and unknown-subcommand behavior remain coherent.

## Gate and review order

1. Demonstrate the focused regressions red against the pre-change mechanism or an isolated single-mechanism
   mutation; record which proof applies to each behavior.
2. Run focused anchored-group, schedule-schema, supervisor, runner-signal, and CLI tests.
3. Run `cargo fmt --all -- --check`, `git diff --check`, workspace all-target check, warnings-denied all-target
   Clippy, locked release build, dependency policy, repository hygiene, all local validators, then the full
   serial workspace suite and report pass/fail/ignored totals.
4. Run one fresh bridge-mediated Sol/xhigh adversarial implementation review of the frozen exact head.
5. Fold every `WRONG`; adjudicate each `SMELL` explicitly. Re-review with Sol only if the mechanism changed.
6. After Sol is green, use the design-approved single Fable/xhigh adversarial implementation/release lens because
   the anchor/journal/concurrency seam is hard and cross-cutting. Do not use Fable as a re-review loop.
7. Publish a non-draft PR only after deterministic gates and required reviews are green. No live provider gate is
   authorized in R3d1.

## Implementation evidence

- The R3c resolver now uses the shared retained-anchor primitive without weakening its exact-identity cleanup.
  R3d1 adds exact TERM/KILL containment, same-session descendant anchoring, retained-anchor finalization, and
  Linux/macOS process-identity inspection.
- The versioned deadline and supervisor schemas validate checked phase sums, elapsed derivation, exact process and
  group identities, signal/outcome/hold shape, parent/child joins, and hash domains. The runtime journal additionally
  enforces a prepared first generation, monotonic phases, immutable identity/deadline fields, append-only groups,
  write-once effects/outcomes, and a one-way anchor lifecycle across generations.
- The default-off supervisor journals before effects, uses one absolute monotonic deadline, escalates first
  cancellation/deadline through TERM then bounded grace and one KILL, proves exact exit/group absence before anchor
  release, and turns effect or observation ambiguity into a durable safety hold. Startup recovery never retries an
  ambiguously journaled numeric process-group signal.
- SIGINT and SIGTERM now share the aggregate runner's one-shot cancellation path. `compatibility schedule-tick` is
  recognized but returns the typed `r3d2_authority_admission_not_implemented; no_effects` refusal before credential
  access or a provider-capable spawn.
- Pre-change-red or isolated mutation proofs cover missing shared anchor/schema/supervisor APIs, TERM delivery,
  stale exact workload identity, deadline publication wedge, cross-generation phase rollback, terminal-first
  initialization, already-absent recovery, observation-error holds, SIGINT/SIGTERM parity, and the fail-closed
  `schedule-tick` boundary. Each new effect path has a negative or edge fixture, including ignored TERM, SIGSTOP,
  repeated cancellation, recycled PGID, unrelated-process survival, journal wedge, crash ambiguity, and mismatched
  child joins.
- Focused gates: process-group **5/0**, resolver compatibility **1/0**, schedule-schema **27/0**, supervisor
  **21/0**, cancellation **3/0**, compatibility CLI **21/0**, and R3d1 CLI **2/0**.
- Full serial workspace: **2,262 passed / 0 failed / 12 ignored** across **56** test binaries. Format/diff,
  workspace all-target check, warnings-denied all-feature Clippy, locked release build, dependency policy,
  repository hygiene (**37** tracked artifacts / **7** example configs), pinned manifest (**9**), floating recipes
  (**4**), and schedule foundation (**6** advisory / **4** claimed-support) are green.
- No timer, private authority issuance, live characterization, model discovery, credential access, container/runtime
  access, registry/image effect, compatibility provider turn, GitHub check mutation, or production-operator
  lifecycle action occurred. The authenticated live-agent/two-bridge/Kiro and local-Ollama tests remain the same
  **12 ignored** cases; no live compatibility gate was run or authorized.

## Restart point

Continue in `/private/tmp/a2a-bridge-r3d1-supervisor` on branch
`agent/reliability-r3d1-supervisor`. Re-read this plan, the active R3d design supervision section, and the
central reliability roadmap. Freeze `HEAD`, `origin/main`, merge base, cleanliness, and changed paths before
review or publication. The next action is to commit the exact candidate, run one fresh bridge-mediated Sol/xhigh
review, fold every `WRONG` and adjudicate every `SMELL`, then run the single design-approved Fable/xhigh
implementation/release lens only after Sol approval. Never touch the long-lived operator lifecycle during R3d1.
