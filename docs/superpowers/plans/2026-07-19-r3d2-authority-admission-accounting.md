# R3d2 — authority, admission, preflights, and accounting implementation plan

- **Status:** ACTIVE — focused implementation plan; no R3d2 mechanism has been implemented or reviewed yet
- **Branch:** `agent/reliability-r3d2-authority-admission`
- **Base:** `origin/main` at `cbcfd1f06b914064456d1798be71bacdc294f3d5`
  (PR #40 merged R3d1)
- **Design of record:** the approved R3d D1-D10 decisions, authority/admission contract, and deterministic
  evidence requirements in
  [`2026-07-11-r3-compatibility-canaries.md`](2026-07-11-r3-compatibility-canaries.md)
- **Merge boundary:** one R3d2 PR after all five internal subincrements and the exact-final branch gates/reviews
- **Effects:** non-billable. Tests use fake clocks, process inventories, effect controls, and owner-state roots.
  This slice does not issue real authority, characterize a profile, discover models, read a provider credential,
  start a registry/image/provider operation, publish to GitHub/iCloud, install or load a timer, or touch the
  long-lived production operator.

## Delivery boundary

R3d2 converts R3d0's inert contracts and R3d1's default-off supervisor into one fail-closed admission mechanism.
It owns private provider-effect authority state, one-run local manual authority, source rederivation, owner-wide
serialization, exact-execution deduplication, durable reserve/reconcile accounting, action-time preflights, legacy
process reconciliation, and the three automatic control reducers. It does not own R3d3 evidence retention/status,
R3d4 launchd/GitHub triggers, or R3d5 live characterization and activation.

No internal commit is independently activatable. Until the final integration subincrement, `schedule-tick` keeps
the R3d1 typed `r3d2_authority_admission_not_implemented; no_effects` refusal. At the completed R3d2 boundary it
may parse and validate a scheduler request, but it cannot reach a provider-capable spawn unless one shared
transaction has already:

1. acquired the nonblocking owner-wide admission lock;
2. reconciled every incomplete supervisor, authority, equivalent-work, and ledger record;
3. taken the authority-state lock after the owner-wide lock;
4. rederived the checked-in policy/profile bundle and exact input-source bindings;
5. selected exactly one valid persistent authority arm, or derived exactly one non-reusable local manual arm;
6. passed both the initial and final zero-effect preflight fences, including action-time root/cwd and legacy-process
   checks;
7. durably committed one admission record that binds authority consumption, attempt fingerprint,
   equivalent-work disposition, and budget reservation; and
8. transferred the admitted capability to the R3d1 supervisor without reopening any caller-controlled identity.

Failure before the durable admission commit is `not_run(<typed-reason>)` with zero provider effect. Failure at or
after a possibly accepted spawn remains non-replayable and conservatively charged until terminal reconciliation.
Revocation before commit refuses; revocation after commit blocks successors but neither kills nor replays the one
already bounded attempt.

## State and lock model

Production scheduler control state resides on the operator Mac's local APFS volume below
`~/Library/Application Support/a2a-bridge/operator/compatibility-scheduler/`. The root and every state directory
are mode `0700`; authority envelopes, journals, indexes, snapshots, and lock files are regular single-link mode
`0600` objects opened relative to retained directory descriptors. Test-only constructors may inject a temporary
root; no production CLI flag, source record, checked-in policy, timer, A2A request, or `serve` request may redirect
the control root.

The state root separates:

- `authority/`: the owner-written envelope, revocation state, one-shot profile index, one-shot entry lifecycle,
  manual-admission nonce consumption, and characterization index;
- `admission/`: committed admission records and equivalent-work reservations/consumptions;
- `ledger/`: append-only reservation/reconciliation records plus rebuilt materialized UTC/rolling views;
- `supervisor/`: R3d1 private per-record journals, now collision-free for every externally derived id; and
- `locks/`: the owner-wide admission lock and the narrower authority-state lock.

The only admission lock order is owner-wide then authority-state. Operator-only issuance/revocation that cannot
admit work takes only authority-state. Locks are nonblocking for billable callers; contention refuses rather than
queues. An OS-lock release after a crash grants no authority by itself: the next contender must reconcile durable
state while holding the owner-wide lock before a new reservation can commit.

The admission linearization point is one immutable, create-new commit record written and synced after all proposed
records are prepared. It binds hashes of the selected authority state, any one-shot/manual consumption, the
authority-bound attempt fingerprint, equivalent-work reservation or reuse consumption, and ledger reservation.
Recovery treats a complete valid commit as consumed/reserved and any ambiguous or partially published transaction
as fail-closed, non-replayable, and conservatively charged when possible prompt acceptance cannot be disproved.
Materialized indexes are rebuildable projections, never authority.

## Internal subincrements

### R3d2a — R3d1 production-integration hardening and local state primitives

Files expected:

- edit `bin/a2a-bridge/src/compatibility_process_group.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_supervisor.rs`
- edit `bin/a2a-bridge/src/compatibility.rs`
- edit `bin/a2a-bridge/src/local_file.rs` only for a generally reusable retained-directory primitive
- add `bin/a2a-bridge/src/compatibility_schedule_state.rs`
- edit `bin/a2a-bridge/src/main.rs`

Work:

1. Make Darwin process-group enumeration distinguish a proved empty group from syscall failure/ambiguous zero by
   clearing and inspecting `errno`; only proved absence can release production ownership.
2. Register SIGINT and SIGTERM independently and preserve whichever registration succeeds; fail closed only when
   neither viable shutdown path exists.
3. Give every supervisor record a private directory and reject `.`/`..`/separator-bearing or otherwise
   noncanonical externally derived ids.
4. Add supervisor-owned cancellation while still `Prepared`; cancellation before `Running` cannot strand a child
   or create an unjournaled provider-capable spawn.
5. Add an exact-runner-exit control primitive that observes the retained child capability rather than inferring
   exit solely from process enumeration.
6. Add retained, owner-private state-root/directory/file primitives and nonblocking owner-wide and authority-state
   locks with the sole permitted lock order.

Required tests and red proof:

- injected Darwin empty/error/resize results separate absence from ambiguity;
- one failed signal registration preserves the other, while two failures refuse;
- `.` and all path-collision mutations fail; two valid record ids cannot share a journal directory;
- cancellation before and during the Prepared-to-Running boundary produces no unowned workload;
- an exited runner with a retained descendant is not confused with a live runner, and a live retained runner is
  not declared exited after an observation error;
- two processes contend on the owner-wide lock and exactly one obtains it without queuing;
- wrong modes, symlinks, special files, network/nonlocal roots where detectable, and reversed lock order fail.

### R3d2b — private authority state, revocation, and source reducers

Files expected:

- add `bin/a2a-bridge/src/compatibility_schedule_authority.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_schema.rs` only for runtime-state contracts missing from R3d0
- edit `bin/a2a-bridge/src/compatibility_schedule.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_state.rs`

Work:

1. Reopen and validate owner-only `CharacterizationAuthorizationV1`, `ProviderEffectGrantV1`, and
   `StorageConsentV1` records, including owner/host/binary/bundle/price/legacy/time/effect/cap and revocation
   bindings. Storage consent remains an independently fenced storage capability, never provider authority.
2. Implement one-shot entry lifecycle `available -> consumed_unreconciled -> reconciled`, immutable revocation,
   and a profile-index uniqueness check across batches. A corrupt/divergent index rebuilds from authoritative
   journals or holds; it never admits by trusting the projection.
3. Implement mutually exclusive persistent reducers: explicit characterization may select only
   `characterization_once`; unattended scheduled/main/test-merge work may select only `standing_grant` and only
   after an exact completed characterization.
4. Derive `ManualAdmissionV1` only inside a direct local generic `compatibility run` after explicit
   `--acknowledge-billable`. Generate the nonce internally, seal the record, and consume it in the same admission
   transaction. It cannot originate from `serve`, A2A, timer, watcher, or caller-supplied source bytes.
5. Generate and validate `ScheduledExecutionSourceV1` and
   `ClaimedSupportCharacterizationSourceV1` from trusted local state, then independently reopen the checked-in
   foundation and rederive the profile-policy bundle, profile, execution, identity, cap, and tagged-authority
   bindings before admission.
6. Implement rollback revocation under authority-state lock for the standing grant and every `available` or
   `consumed_unreconciled` one-shot entry without mutating reconciled history.

Required tests and red proof:

- absence, expiry, pre-commit revocation, stale generation/hash/bundle/binary/owner/host/price/legacy identity,
  widened cases/effects, invalid cap, wrong trigger/label/plist, and deadline beyond authority expiry all refuse;
- revocation after an already committed reservation blocks successors but leaves that bounded admission valid;
- same-batch, cross-batch, concurrent issuance, crash, and corrupt-index duplicate profiles fail closed;
- crash before consumption leaves an entry available with no admission; crash at/after commit leaves it consumed,
  non-replayable, and conservatively reserved; reissue requires a new reviewed entry naming the prior outcome;
- one-shot before first characterization is positive; standing grant before characterization, one-shot after
  characterization, both/unknown/mixed arms, or characterization authority from timer/main/test-merge are negative;
- direct generic manual positive reaches the shared transaction; missing acknowledgement, caller nonce, replay,
  wrong origin/source/profile/execution/purpose/cap/effect, expiry, mutation, or any persistent/manual arm mixture
  refuses;
- valid/invalid provider authority crossed with valid/invalid storage consent proves the two capabilities are
  independent. R3d2 validates the consent fence only; it performs no cold write.

### R3d2c — exact identities, equivalent work, and control reducers

Files expected:

- add `bin/a2a-bridge/src/compatibility_schedule_admission.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_schema.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_state.rs`

Work:

1. Recompute canonical characterization-profile, exact case-execution, authority-bound admission-attempt, and
   equivalent-work identities at the final fence. Never accept a caller's hash as its own proof.
2. Implement the closed evidence-purpose lattice. Equal or stronger complete evidence may create a consumption
   record; live equivalent work refuses; first characterization never reuses earlier evidence; manual diagnostic
   remains incomparable by default.
3. Add the characterization state reducer and the three disjoint controls: automatic safety hold, exact-execution
   waste suppression, and explicit operator quarantine. Authority/budget/quarantine/unknown outcomes never enter
   waste suppression.
4. Implement first-transient `confirmation_due`, one separately authorized next-window confirmation, second
   identical complete transient suppression, and immediate typed-immutable suppression. Repeated
   `candidate_unknown` records audit/notification progress but never suppress.
5. Preserve the admitted execution/equivalent-work key if observed effective identity later differs; record
   `candidate_unknown`, hold, refuse evidence reuse, and never re-key after effect.

Required tests and red proof:

- mutate every profile field independently and prove profile/recharacterization changes; mutate every exact-run
  field independently and prove execution/equivalent-work changes without gratuitously changing the profile;
- trigger/request/window/attempt/authority mutations change admission identity but not equivalent work; exact target
  changes are intentionally non-equivalent;
- concurrent/sequential duplicate sources yield one reservation; completed stronger evidence reuses through a new
  consumption; manual diagnostic and first characterization do not reuse;
- immutable, first transient, second identical transient, recovered confirmation, repeated unknown, safety hold,
  quarantine, and budget-blocked outcomes take only their approved reducer path;
- observed-effective mismatch retains the original identities and cannot become successful evidence.

### R3d2d — durable ledger, legacy reconciliation, and zero-effect preflights

Files expected:

- add `bin/a2a-bridge/src/compatibility_schedule_ledger.rs`
- add `bin/a2a-bridge/src/compatibility_schedule_preflight.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_admission.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_state.rs`
- reuse existing compatibility, resolution, smoke, and local-file validators without adding a diagnostic provider
  call

Work:

1. Implement create-new append-only reservation/reconciliation records, idempotent recovery, canonical UTC-day and
   rolling-24-hour views, per-case/provider/trigger/shared caps, protected scheduled/test-merge pools, and manual
   unallocated-headroom rules.
2. Reserve conservative attempt/token/cost/time maxima before spawn. Release only after proved pre-effect failure;
   terminal valid usage may reconcile downward; possible prompt acceptance, spawn/KILL/crash ambiguity, missing or
   invalid telemetry, unknown price/currency, and subscription attempts retain the conservative charge.
3. Detect pre-R3d executables/processes by exact executable/start identity and argv. Allow only the retained exact
   production `serve` identity; hold on a live legacy compatibility command or ambiguous provider-capable child.
   Import validated legacy aggregates; otherwise charge the conservative ceiling or wait for an unknown initial
   rolling window to age out. Never kill a legacy process.
4. Run all automated zero-effect preflights at the initial and final fences: owner/architecture, authority and
   policy hashes, candidate/config/manifest/recipe and scheduled registry, controls/characterization, ledger,
   OAuth runway, environment bindings, price/ranking snapshot, storage headroom, present-image/no-pull container
   start control, legacy inventory, and supervisor recovery.
5. Immediately before durable admission, descriptor-pin/re-resolve the trusted owner root and requested cwd as real
   directories and prove the cwd remains within that same root object. R3d0 static validation is never action-time
   authority.

Required tests and red proof:

- reservation/spawn/prompt/reconciliation crash points, restart idempotence, missing usage, midnight crossing,
  rolling guard, class/protected-pool exhaustion, and manual non-borrowing preserve exact conservative totals;
- a pre-effect refusal releases, while possible acceptance does not; test-merge regeneration before versus after
  possible acceptance follows that distinction and never auto-replays;
- exact production `serve` is allowed; legacy `compatibility`, ambiguous child, divergent executable identity, and
  unreconciled legacy aggregate hold at both fences;
- every preflight failure produces a typed zero-effect refusal and the fake provider/model/registry/runtime controls
  prove zero calls;
- action-time root/cwd symlink swap, rename, replacement, outside target, missing directory, wrong owner/mode, or
  identity drift refuses even when the earlier static foundation validated.

### R3d2e — shared transaction and default-off entrypoint integration

Files expected:

- edit `bin/a2a-bridge/src/compatibility.rs`
- edit `bin/a2a-bridge/src/compatibility_schedule_supervisor.rs`
- edit the R3d2 authority/admission/ledger/preflight modules
- edit `bin/a2a-bridge/tests/compatibility_cli.rs`
- add a focused R3d2 CLI integration test if keeping the existing file bounded is clearer
- edit operator/foundation/reliability docs and the durable roadmap

Work:

1. Join persistent and generic-manual reducers to one admission transaction with the required lock order and one
   durable commit point. Persist authority/attempt/equivalent-work/ledger identities before transferring an admitted
   capability to the supervisor.
2. Add sealed internal scheduled and claimed-support-characterization runner sources. They are generated by trusted
   scheduler/explicit-characterization code, not arbitrary direct CLI input; independent revalidation occurs again
   immediately before admission.
3. Integrate R3d1 supervision through injected production/fake control traits, including pre-Running cancellation
   and exact-runner-exit ownership. A safety hold retains admission ownership until reconciliation proves release.
4. Keep every live scheduler route default-off in the merged artifact: no private authority is issued, no timer or
   GitHub trigger is installed, and ordinary `schedule-tick` without exact private state is a typed zero-effect
   refusal. R3d5 remains the sole live characterization/activation authority.
5. Emit the local R3d2 admission/ledger/supervisor artifacts needed by later R3d3, without implementing hot/cold
   evidence storage, iCloud publication, status notification, or GitHub checks.

Required tests and red proof:

- persistent one-shot, persistent standing, and generic manual positives reach the same durable reservation and
  supervisor handoff while binding distinct authority identities;
- every missing/both/mixed/stale/revoked authority form fails before fake provider access;
- a crash at every transaction journal point recovers to exactly one of: no admission/effect, one committed bounded
  admission, or conservative non-replayable hold—never duplicate execution or charge;
- scheduled versus manual and daily versus test-merge sources targeting equivalent work produce one billable
  reservation and the correct new consumption records;
- cancellation, supervisor hold, and exact exit reconcile the ledger once and cannot admit a successor early;
- CLI/static tests prove no `serve` endpoint, A2A caller, timer, watcher, source-file mutation, or production-operator
  lifecycle action can manufacture authority or bypass the shared transaction;
- the unchanged default checkout has no authority and all R3d2 entrypoints remain zero-effect.

## Verification and review gates

1. Before each mechanism is accepted, demonstrate its focused regression against the pre-change implementation or
   an isolated single-mechanism mutation. Record the exact red command/assertion and pair every new effect path with
   a negative or edge case.
2. After each internal commit, run format/diff checks plus its complete focused unit/integration tests. Do not call a
   provider or container registry/runtime merely to validate the mechanism.
3. Before review, run `cargo fmt --all -- --check`, `git diff --check`, workspace all-target check,
   warnings-denied all-target Clippy, locked release build, dependency policy, repository hygiene, manifest/recipe/
   policy/foundation validators, all scheduler CLI tests, and the full serial workspace suite. Report exact
   passed/failed/ignored totals and every unexercised boundary.
4. Freeze exact head/base/merge-base/changed paths and run one fresh bridge-mediated Sol/xhigh adversarial
   implementation review. Fold every `WRONG`; adjudicate every `SMELL`. Re-review with Sol only after a mechanism
   change.
5. After Sol is green, run the design-approved single Fable/xhigh adversarial implementation plus release/
   compatibility lens because authority/concurrency/accounting is hard and cross-cutting. Do not use Fable as a
   re-review loop.
6. Rerun exact-final deterministic gates after the final docs/evidence fold and publish one non-draft R3d2 PR.

## Explicitly unverified until later slices

- No real one-shot characterization authorization, provider-effect standing grant, or storage consent is issued.
- No live provider/model/registry/image/container effect or compatibility turn occurs.
- No launchd job, GitHub watcher/check publisher, timer tick, or required context exists.
- No iCloud byte, hot/cold evidence object, retention/GC action, or notification is written.
- No production operator process is stopped, restarted, drained, rotated, or used as a scheduling endpoint.
- R3d3 owns evidence/status/retention; R3d4 owns trusted triggers; R3d5 owns characterization and staged activation.

## Restart contract

Resume from `/private/tmp/a2a-bridge-r3d2-authority-admission` on branch
`agent/reliability-r3d2-authority-admission`, based on merged R3d1 main
`cbcfd1f06b914064456d1798be71bacdc294f3d5`. Read this plan, the R3d design of record, the durable roadmap,
`AGENTS.md`, and `skills/a2a-bridge-operator/SKILL.md` before editing. Preserve the single R3d2 merge boundary,
the owner-wide-then-authority lock order, the single admission linearization point, the zero-effect default, and
the separation between provider authority and storage consent.
