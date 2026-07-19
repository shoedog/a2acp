# R3d2 — authority, admission, preflights, and accounting implementation plan

- **Status:** ACTIVE — R3d2a through R3d2e are implemented. Sol/xhigh reviews one through three returned `REVISE`
  at `1373985`, `28e7d28`, and `d082b49`; exact-head review four returned `REVISE` at `c418df4` after resolving all
  ten inherited items, with two new `WRONG` and one new `SMELL`. Mechanism commit `5a01ce7` closes the three fresh
  findings with pre-change-red regressions. Exact-head full deterministic gates, a fresh Sol closure re-review,
  and the post-Sol Fable lens remain pending
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

No internal commit is independently activatable. At the completed R3d2 boundary, `schedule-tick` accepts no source
arguments and returns the typed `r3d5_activation_not_enabled; no_effects` refusal. No live route can construct an
admitted capability. The internal transaction mechanism used by deterministic tests—and available only to future
R3d5 activation code—cannot transfer a provider-capable spawn unless one shared transaction has already:

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
- `supervisor/`: R3d1 owner-private journals with collision-free generation prefixes for every externally
  derived id; and
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
3. Exclude `.` from the already bounded externally derived supervisor ids, in addition to rejecting `.`/`..`,
   separators, and other noncanonical bytes, so prefix-style generation names cannot alias another record.
4. Add supervisor-owned cancellation while still `Prepared`; cancellation before `Running` cannot strand a child
   or create an unjournaled provider-capable spawn.
5. Add an exact-runner-exit control primitive that observes the retained child capability rather than inferring
   exit solely from process enumeration.
6. Add retained, owner-private state-root/directory/file primitives and nonblocking owner-wide and authority-state
   locks with the sole permitted lock order.

Required tests and red proof:

- injected Darwin empty/error/resize results separate absence from ambiguity;
- one failed signal registration preserves the other, while two failures refuse;
- `.` and all path-collision mutations fail; two valid record ids cannot alias a journal generation prefix;
- cancellation before and during the Prepared-to-Running boundary produces no unowned workload;
- an exited runner with a retained descendant is not confused with a live runner, and a live retained runner is
  not declared exited after an observation error;
- two processes contend on the owner-wide lock and exactly one obtains it without queuing;
- wrong modes, symlinks, special files, network/nonlocal roots where detectable, and reversed lock order fail.

#### R3d2a checkpoint evidence — 2026-07-19

Implemented mechanism:

- Darwin group enumeration clears/captures thread-local `errno` around both `proc_listpgrppids` calls; only
  zero with zero `errno` proves an empty group. The Apple wrapper returns PID counts and can collapse its
  underlying `-1 / sizeof(int)` to zero while preserving `errno`.
- SIGINT and SIGTERM registrations are independent; either viable stream remains usable when the other
  registration fails, and two failures refuse.
- Supervisor ids no longer accept `.`, closing generation-prefix aliasing without changing the shared
  owner-private journal directory layout.
- `Prepared` cancellation durably enters no-later-signal `Reaping`, fences possible workloads both before the
  transition and immediately before retained-anchor release, performs only exact cleanup/absence proofs, and
  ends as `cancelled_before_running`. Recovery resumes the same cleanup without a numeric-group signal.
- `RetainedRunnerExit` owns the waitable direct child and exact identity. Its tests distinguish a live runner,
  a mismatched identity, cached terminal status, and a runner exit while a same-start descendant remains live
  after reparenting.
- The fixed local-APFS production state-root model retains `0700` root/child descriptors, opens verified `0600`
  single-link lock files relative to the retained lock directory, refuses broadened/symlink/special objects,
  uses nonblocking cross-process `flock`, releases on normal or abrupt process exit, and exposes only a combined
  admission capability that owns both locks until authority-before-owner release plus a separate authority-only
  operator-mutation capability.

Pre-change red proof:

- `supervisor.1` was accepted as a record id and collided with the prefix grammar.
- cancellation requested in `Prepared` remained `Prepared` and did not own cleanup.
- one injected signal-registration failure discarded the other viable signal path.
- a Darwin zero return with `EIO` was accepted as absence.
- the recovery race regression observed `ReleasedReaped` instead of `RetainedLive` when a possible workload
  appeared after the `Reaping` journal generation; the just-before-release fence makes it green.
- the first nested-lock type allowed the owner guard to be dropped while its authority guard remained live;
  `run-2` then acquired the owner lock. The combined admission capability keeps both locks live and closes that
  concrete overlap.
- the retained-runner and scheduler-state APIs were absent at the R3d1 base, so their behavioral test modules
  were compile-red there.

Deterministic gates:

- `cargo fmt --all -- --check`, tracked/untracked diff checks, and
  `RUSTFLAGS='-D warnings' cargo check -p a2a-bridge --all-targets` are green.
- Focused tests are process group **7/0**, independent signal registration **1/0**, schedule schema **32/0**,
  supervisor **41/0**, and scheduler state/locks **8/0**.
- The complete `a2a-bridge` binary is **562/0/0**. Its first run was **559/1** because the new retained-child
  test used scheduler yields with no wall-clock allowance; bounded 10 ms polling passed **10/0** repeatedly,
  the descendant edge passed **3/0** repeatedly, and the complete rerun is green.
- The exact post-lock-fix full serial workspace is **2,298/0/12 ignored** across **72** reported test/doc-test
  result groups, **55** of them nonempty.

Not verified or authorized at this checkpoint: real OS-delivered SIGINT/SIGTERM, a nonlocal production root,
creation or mutation of the real operator state root, a provider/model/registry/image/GitHub/iCloud effect,
timer installation, production-operator lifecycle action, or an independent implementation review. R3d2a is
not independently activatable, and `schedule-tick` retains its typed no-effects refusal.

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

#### R3d2b checkpoint evidence — 2026-07-19

Implemented mechanism:

- Owner-issued characterization batches, standing provider-effect grants, and storage consents are canonical,
  domain-separated, self-hashed records reduced through copy-validate-commit state changes. Provider rollback
  increments the standing-grant and every nonterminal one-shot revocation generation; storage revocation remains
  independent.
- One-shot state is append-only `available -> consumed_unreconciled -> reconciled`. Entry ids and authority-wide
  nonces are unique, a materialized profile index is rebuilt only from authoritative records, and a same-profile
  reissue must name the sole terminal predecessor, next generation, and reviewed reason. Concurrent operator
  issuance takes the nonblocking authority-state lock; a second process refuses rather than queues.
- Mode-`0600` canonical JSON authority generations form one contiguous hash chain under a retained mode-`0700`
  directory capability. Immutable envelopes/manual consumptions cannot disappear or change, lifecycle and
  revocation transitions are monotonic, partial/corrupt authoritative generations hold, and only a divergent
  rebuildable profile projection may be repaired and durably superseded.
- Explicit characterization selects only one exact available `characterization_once` entry and now binds its
  reviewed command in addition to source/profile/execution/effective identity/provider/effects/caps and the full
  owner/host/binary/bundle/price/legacy/deadline environment. Unattended daily/main/test-merge selection accepts
  only a standing grant with its exact completed non-inconclusive characterization and, where applicable, exact
  launchd label/plist binding.
- Direct generic manual derivation accepts only the internal direct-local-CLI origin plus explicit billable
  acknowledgement, rejects caller nonce input, uses 256 bits from `SystemRandom`, seals a maximum-15-minute
  `ManualAdmissionV1`, and exposes an exactly-once durable nonce-consumption reducer. Serve, A2A, timer, watcher,
  characterization-purpose, replay, expiry, mutation, and persistent/manual-arm mixtures refuse. R3d2e still owns
  joining this reducer to the shared admission commit and CLI route.
- The foundation loader now exposes immutable scheduled and claimed-support bindings containing raw source/row,
  stable profile/bundle, requested/effective identity, effect/cap maximum, semantic config-template, and exact raw
  config hashes. Scheduled and claimed-support source generators load this state, seal a strict source, then reopen
  and independently rederive every foundation-bound field. The source DTOs now explicitly carry the config-template
  digest that the design required but R3d0 omitted.

Pre-change and mutation red proof:

- At checkpoint `5104332`, the authority module, journal, source reducers, manual reducer, and authority-only
  cross-process contention tests did not exist; their modules/tests are compile-red against that tree.
- The first duplicate-nonce issuance regression returned `Err` but left the candidate model mutated and invalid.
  Copy-validate-commit leaves the prior model byte-equivalent and the regression is green.
- Removing only the one-shot `entry.command == request.command` comparison made
  `one_shot_selection_fences_all_bound_identity_and_revocation_state` fail **0/1** at its wrong-command assertion;
  restoring the comparison passes the exact test **1/0**.
- A stale source mutation fails its self-hash. Rebuilding every dependent execution/admission/source hash after an
  exact-config or template mutation still refuses only when the validator independently reopens the checked-in
  foundation, proving the source's own internally consistent bytes are not authority.
- Manual replay leaves the prior state unchanged; deleting a consumed manual record from a later journal generation,
  rewriting immutable authority history, corrupting authoritative bytes, or publishing a partial generation refuses.

Deterministic gates:

- `cargo fmt --all -- --check`, tracked and untracked whitespace checks,
  `RUSTFLAGS='-D warnings' cargo check -p a2a-bridge --all-targets`, and warnings-denied all-target Clippy are green.
- Focused tests are authority/source/manual/journal **15/0**, scheduler state/locks **10/0**, strict schedule schema
  **32/0**, and foundation **9/0**.
- The complete `a2a-bridge` binary is **579/0/0**. The full serial workspace is
  **2,315/0/12 ignored**; ignored cases are the existing explicitly authenticated/live Kiro and local-Ollama tests.

Not verified or authorized at this checkpoint: a real authority envelope, real manual CLI route or shared admission
commit, provider/model/credential/registry/image/runtime/GitHub/iCloud effect, timer/watcher installation, real
production state root, production-operator lifecycle action, source-schema compatibility review, or independent
implementation review. R3d2b is not independently activatable, and `schedule-tick` retains its typed no-effects
refusal.

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

#### R3d2c checkpoint evidence — 2026-07-19

Implemented mechanism:

- The final fence validates the sealed scheduled, claimed-support, or direct-manual source, independently reloads
  the checked-in foundation where applicable, and rederives the profile, trigger-independent case execution,
  tagged-authority admission attempt, evidence-purpose/freshness equivalent-work key, and repeat-bound attempt
  idempotency key. Trigger/request/window/attempt/repeat, Daily versus ScheduledMain, standing-grant rotation, and
  standing versus manual authority change admission identity without changing exact execution/equivalent work.
- Equivalent-work state is copy-validate-commit and validates both directions of every materialized index. One live
  exact execution refuses rather than queues. Terminal equal/stronger evidence creates a new authority/trigger-bound
  consumption; claimed-support and explicitly reviewed characterization may satisfy advisory work. First
  characterization and generic manual diagnostic never reuse. A consumption must dereference exact completed
  evidence, and malformed, identity-drifted, or unreviewed characterization evidence cannot become reusable.
- Characterization history is append-only and self-hashed. A matched green or known-issue terminal record may be the
  sole current characterization for its exact profile; inconclusive or observed-identity-mismatched records remain
  immutable history without promotion, and ids/profiles cannot be rebound.
- Safety-hold openings and explicit clearances, quarantine openings and explicit closures, and exact-execution
  waste/confirmation state are independent immutable histories with complete reverse indexes. Holds never expire;
  quarantine expiry remains blocked until explicit owner closure. Authority, budget, and quarantine blocks do not
  enter waste state.
- Typed immutable failure suppresses immediately. A first typed/untyped transient becomes `confirmation_due`; any
  later attempt for that exact execution requires the one separately authorized next window, repeat nonce, same
  evidence purpose/freshness/equivalent-work identity, and same standing-grant authority. A second identical typed
  transient suppresses, an identical untyped or nonidentical transient remains unknown, and a pass records recovery.
  Repeated `candidate_unknown` only advances count/time audit state and never creates confirmation or suppression.
- Observed-effective drift atomically records nonreusable `candidate_unknown` plus
  `identity_drift_after_effect` hold while retaining the original profile, execution, admission, and equivalent-work
  identities. A reusable terminal record with expected/observed drift is invalid.
- `compatibility_schedule_state.rs` required no R3d2c edit: R3d2c owns provider-free reducer models only; their
  create-new journal/commit integration under the existing owner lock remains the explicit R3d2e transaction.

Pre-change and mutation red proof:

- At R3d2b checkpoint `b8b5a13`, the admission module, identity rederivation, equivalent-work/characterization/control
  reducers, and their tests did not exist, so the new module/tests are compile-red there.
- Removing only the confirmation's same-standing-grant comparison made
  `immutable_transient_confirmation_recovery_and_unknown_paths_are_disjoint` fail **0/1** at the wrong-generation
  negative; restoring it passes the exact test **1/0**.
- Removing only the global `confirmation_due` authorization guard made
  `unauthorized_repeat_and_untyped_confirmation_never_suppress` fail **0/1** because an un-authorized repeat was
  accepted; restoring it passes the exact test **1/0**.
- The typed profile mutation proof changes every serialized profile field independently, including nested
  environment and cap fields, and each changes the profile hash. Changing only exact config bytes intentionally
  leaves profile identity stable; the exact-execution mutation test proves generated-config bytes change execution
  and equivalent-work identity instead. Test-merge fields, scheduled-main range, candidate, every exact binding,
  requested/expected identity, and within-profile actual-cap mutations are likewise non-equivalent.

Deterministic gates:

- `cargo fmt --all -- --check`, tracked/untracked whitespace checks, workspace all-target/all-feature check, and
  workspace all-target/all-feature Clippy with warnings denied are green.
- Focused admission/identity/equivalent-work/characterization/control tests are **16/0**; the independent canonical
  profile-field sensitivity test is **1/0**.
- The complete `a2a-bridge` binary is **596/0/0**. The exact full serial workspace is
  **2,332/0/12 ignored**; ignored cases are the existing explicitly authenticated/live Kiro and local-Ollama tests.

Not verified or authorized at this checkpoint: a real authority/admission/confirmation, real manual CLI route,
durable shared admission commit, provider/model/credential/registry/image/runtime/GitHub/iCloud effect, timer/watcher
installation, real production state root, production-operator lifecycle action, source-schema compatibility review,
or independent implementation review. R3d2c is not independently activatable, and `schedule-tick` retains its typed
no-effects refusal.

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

#### R3d2d checkpoint evidence — 2026-07-19

Implemented mechanism:

- The owner-wide lock now grants the only ledger/admission/supervisor state capability. The ledger rebuilds its
  authoritative view from separate create-new, single-link mode-`0600` reservation, reconciliation, and legacy-
  import records; a mutable projection cannot create headroom. Canonical record bytes, derived filenames, record
  hashes, reservation/reconciliation crosslinks, attempt/equivalent-work identities, accounting-policy hash,
  source-derived case/provider, trigger, class, UTC admission day, and rolling-window identity are revalidated on
  every open. A partial, noncanonical, rebound, orphaned, duplicated, broadened, or unknown record holds.
- Admission reserves the exact one-attempt token/cost/time maximum before spawn. An absent reconciliation and every
  typed spawn/prompt/KILL/crash/usage/price/currency/evidence ambiguity remain a full conservative charge. Only an
  explicit proved-pre-effect terminal releases all dimensions. Valid terminal evidence may reconcile token/cost/
  time downward, while the provider attempt remains charged even for a zero-USD subscription observation. A
  regenerated execution is eligible only after the proved-pre-effect disposition; possible acceptance never
  auto-replays.
- Rebuilt views enforce current UTC-day and rolling-24-hour caps plus exact per-case, per-provider, and per-trigger
  ceilings. Characterization uses its sealed one-shot conservative maximum. Scheduled and test-merge work consume
  only their protected pools. Generic manual work uses only the active accounting grant's explicit
  `manual_unallocated` allocation and cannot borrow either protected pool. A clock rollback behind durable state
  refuses. The reservation remains charged to its admission day across midnight.
- Generic manual authority now seals canonical `case_id` and `provider_family` alongside its execution fingerprint.
  Scheduled and claimed-support final rederivation validate identities and derive case/provider accounting labels
  from the same reopened multi-file foundation snapshot; manual labels come from the same sealed manual record as
  its identities. Each path returns one source-derived ledger context, and the ledger's external request constructor
  accepts that context rather than caller-supplied accounting labels.
- Validated legacy aggregates and conservative/unknown-initial-window ceilings are immutable ledger imports. The
  unknown initial charge ages out only after its exact rolling window. Both legacy fences compare full PID/start/
  parent/group/session identity, executable path/device/inode/hash, and complete argv against the sealed inventory.
  They allow the exact current scheduler, the exact retained production `serve`, and explicitly inventoried
  non-compatibility processes; a live legacy `compatibility`, divergent bridge executable, unapproved bridge
  process/descendant, missing or drifted required process, inventory-authority mismatch, or missing import creates
  a typed safety hold. This path has no signal/kill capability.
- Initial and final fences execute the same closed 18-item local checklist: owner/architecture, effect authority and
  policy, candidate/config/manifest/recipe/registry, controls/characterization, ledger, OAuth/environment,
  price/ranking, storage, present-image/no-pull control, legacy inventory, supervisor recovery, and action
  directories. The protocol exposes no provider, model-discovery, registry-effect, image-pull, or agent-spawn
  capability; every malformed proof or typed local refusal records zero such calls. Preflight identities use one
  domain separator plus one canonical payload, and a read-only platform regression exercises current-process PID,
  executable, argv, process-group, session, and start-identity discovery on supported hosts.
- Action-time trusted-root and requested-cwd bindings reject aliases during planning, then re-snapshot and
  descriptor-pin the exact objects. Every containment component is reopened no-follow relative to the retained root,
  checked for exact ownership and non-writable group/world mode, retained/rechecked, and matched to the independently
  pinned cwd. Symlink swap, rename, replacement, outside/missing target, wrong owner/mode, or identity drift refuses.

Pre-change and mutation red proof:

- At R3d2c checkpoint `71e39a1`, neither ledger/preflight module nor their focused tests existed, the state guard did
  not expose a lock-scoped ledger capability, and manual authority did not seal case/provider accounting dimensions;
  the new module/tests are compile-red there.
- Replacing the unreconciled-reservation full charge with zero made
  `reservation_restart_and_reconciliation_crash_points_are_conservative_and_idempotent` fail **0/1**, observing
  `0/0/0/0` instead of `1 attempt / 100 tokens / 1000 microusd / 30 seconds`; restoring it passes **1/0**.
- Pointing the manual class at the deliberately larger protected-scheduled pool made
  `accounting_classes_use_disjoint_pools_and_manual_never_borrows` fail **0/1** because the second manual request was
  admitted instead of returning `ManualUnallocatedExhausted`; restoring the manual pool passes **1/0**.
- Comparing a legacy allowance by numeric PID alone made
  `missing_drifted_or_unreconciled_legacy_state_holds_without_process_action` fail **0/1**: a changed start identity
  returned `Clear` instead of `AllowedProcessIdentityDrift`; restoring the full process identity passes **1/0**.
- Re-appending the canonical payload in `preflight_hash` made
  `preflight_hash_is_one_domain_separated_canonical_payload` fail **0/1**, observing
  `209320cb...60d61f` instead of the single-payload `d3cc35b9...5e99b`; restoring the single append passes **1/0**.
- Independent negatives also prove manual case/provider mutation invalidates its seal; all eight conservative reason
  codes preserve every cap; each case/provider/trigger and protected/manual dimension refuses independently; a torn
  reservation or reconciliation holds on restart; and every checklist item refuses at both fences with fake
  provider/model/registry/runtime counters still zero.

Deterministic gates:

- `cargo fmt --all -- --check`, tracked/untracked whitespace checks, workspace all-target/all-feature check, and
  workspace all-target/all-feature Clippy with warnings denied are green.
- Focused R3d2d gates are ledger **12/0**, preflight/legacy/action-directory/platform **11/0**, and source-context
  integration **1/0**. The complete `a2a-bridge` binary is **619/0/0**.
- The exact full serial workspace is **2,355/0/12 ignored**; ignored cases are the existing explicitly
  authenticated/live Kiro and local-Ollama tests.

Not verified or authorized at this checkpoint: a real authority/admission/ledger record, full host process inventory
against a sealed legacy plan, manual CLI route, shared admission commit,
provider/model/credential/registry/image/runtime/GitHub/iCloud effect,
timer/watcher installation, real production state root, production-operator lifecycle action, source-schema
compatibility review, or independent implementation review. R3d2d is not independently activatable, and
`schedule-tick` retains its typed no-effects refusal.

### R3d2e — shared transaction and default-off entrypoint integration

Implementation breakdown and crash order:

- **R3d2e1 — commit journal.** Add an owner-lock-scoped admission journal whose create-new commit is the sole
  linearization point. A commit binds the exact source-derived identities, authority action and previewed authority
  generation, equivalent-work disposition, prepared ledger reservation, both preflight results, action-directory
  bindings, and prepared supervisor record. Recovery replays only a complete canonical hash-chained commit; a torn,
  rebound, skipped, or divergent generation holds.
- **R3d2e2 — authority/source/accounting join.** Under owner-wide then authority-state locks, reopen the authority
  journal and checked-in foundation, reselect exactly one one-shot/standing/manual authority path, derive the ledger
  policy only from that selected record, run the final zero-effect fence, and prepare all reducer outputs without
  publishing them. The durable commit precedes authority-journal and ledger publication; recovery completes those
  idempotently before any handoff.
- **R3d2e3 — supervisor handoff and reconciliation.** Materialize the commit-bound R3d1 `Prepared` generation before
  transferring an opaque admitted capability to an injected runner. Recovery may recreate or resume `Prepared` but
  never repeats provider handoff. Typed pre-effect cancellation releases the ledger/equivalent-work reservation;
  possible acceptance or a supervisor hold remains conservatively charged and non-replayable; exact terminal evidence
  reconciles once.
- **R3d2e4 — default-off boundary.** No command-line source path, serve/A2A route, timer, watcher, or production
  operator action can construct an internal runner source or admitted capability. With no private R3d authority, the
  existing explicitly acknowledged manual compatibility command remains the legacy-compatible path. If private R3d
  authority exists, legacy manual execution must not bypass the shared transaction. `schedule-tick` remains a typed
  zero-effect refusal until R3d5 issues authority and activates trusted triggers.

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

#### R3d2e implementation checkpoint — 2026-07-19

Implemented mechanism:

- `11e892a` adds the owner-lock-scoped, canonical create-new admission journal and the single durable commit point.
  Each hash-chained commit binds the rederived source and exact identities, one authority action and previewed
  authority generation, equivalent-work disposition, prepared ledger reservation, both zero-effect fences,
  descriptor-planned action roots, and the exact R3d1 `Prepared` supervisor generation. A torn, skipped,
  noncanonical, state-mutated, authority-divergent, or partially published commit holds on reopen. Recovery
  idempotently publishes authority, ledger, and `Prepared` state but has no runner argument and cannot replay an
  effect.
- The safe transaction session first reconciles every prior complete commit while retaining owner-wide then
  authority-state locks, snapshots the exact resulting authority head, and requires source rederivation,
  preparation, final fencing, and commit under that same non-cloneable capability. Scheduled standing,
  claimed-support one-shot, and R3-aware generic-manual paths reopen their authoritative records, derive accounting
  only from the selected record, and reach the same commit reducer with distinct tagged authority identities. Raw
  prepare/commit helpers remain module-private.
- A successful reserved publication yields one opaque, non-`Clone` admitted capability bound to the exact commit,
  effect envelope, supervisor id, identities, and pinned action directories. Only the injected one-shot handoff
  trait may consume it. Static reachability inspection finds every constructor and handoff reference inside
  `compatibility_schedule_transaction.rs` and its tests; no CLI, `serve`/A2A, timer, watcher, or operator lifecycle
  path references it.
- `8da4171` adds a same-generation canonical terminal journal. Exact `cancelled_before_running` proof releases the
  ledger and nonreusable equivalent-work reservation; a safety hold or possible acceptance keeps the full charge and
  blocks a successor. `f481f39` removes the caller-constructed completed-terminal arm: only an opaque proof loaded
  from the supervisor-joined immutable one-case child aggregate may complete work. Reconciliation derives requested
  and observed identity, prompt acceptance, candidate and manifest identity, terminal time, elapsed time, token/cost
  usage, and missing-observation fallback from those bytes; it validates the aggregate totals/counters, charges a missing
  observation at the reserved cap, and refuses identity drift or cap overflow. One-shot lifecycle reconciliation
  binds the immutable terminal-file hash. Every recovery reopens the R3d1 journal and requires the terminal's
  complete supervisor record and tail hash to equal its actual immutable tail before changing ledger or authority
  state. The same commit previews equivalent-work disposition before allocating ledger/supervisor artifacts, making
  completed standing-work reuse reachable without a second reservation or handoff.
- `f3bbee0` closes the default-off boundary. `schedule-tick` accepts no source arguments and returns the typed
  `r3d5_activation_not_enabled; no_effects` refusal. The fixed production state root is derived from the effective
  account's passwd home rather than `$HOME`. `f700cde` opens every fixed suffix component descriptor-relatively with
  `O_NOFOLLOW`, so an intermediate or final symlink, broadened root, nonlocal root, or identity swap fails closed.
  Presence probing remains read-only. Absence preserves the explicitly acknowledged legacy manual run command;
  presence marks private R3d takeover and refuses that billable command before manifest/resolution/output or provider
  access; ambiguous state also refuses. Only R3d5 may initialize production state or activate trusted triggers.

The standalone R3c `compatibility resolve` command remains a separate provider-free but registry/image-effectful
operator action under its own `--acknowledge-resolution-effects` boundary, even after private R3d state exists. It
cannot issue or consume provider authority, construct an admitted capability, authorize a later run, or replace
production pins/tags. Scheduler-owned resolution/materialization in R3d5 must instead be covered by the shared R3d
admission and effect envelope. This separation adjudicates the first Sol review's takeover-scope `SMELL` without
silently broadening R3d2's legacy billable-run guard.

Pre-change and mutation red proof:

- At R3d2d checkpoint `f832604`, the admission commit/terminal journals, safe transaction session, admitted
  capability, shared source joins, and the transaction test module were absent, so the focused e1-e3 suite is
  compile-red there. The stale/revoked/effect-mismatched source negatives, torn/skipped/noncanonical commit
  negatives, duplicate-handoff refusal, cancellation/hold/completed terminal paths, pre-terminal successor refusal,
  reducer-state mutation, and supervisor-tail mutation each target a distinct new branch.
- The first review showed that the original completed-terminal API itself was the wrong trust boundary: a caller
  could construct nominal identity/usage that was never joined to the immutable child aggregate. The new test is
  API/compile-red before `f481f39`, loads a real aggregate through `VerifiedChildArtifact`, and proves observed-model
  drift, over-cap tokens, zero accepted prompts, false telemetry-completeness counters, and divergent aggregate cost
  totals all refuse without a terminal. Its missing-telemetry edge charges token/cost caps. The standing reuse test
  is behavior-red before the preview fix because a preallocated ledger/supervisor reaches the reducer's reuse arm
  and fails the disposition invariant; it now returns `Reused` with no new ledger or supervisor entry.
- Without the actual supervisor-tail comparison, `terminal_supervisor_tail_mutation_holds_recovery` accepts the
  canonical mutated terminal during recovery.
- Before the fixed-root guard, an acknowledged legacy run could proceed regardless of private R3d state and the root
  opener accepted a final-component symlink after canonicalization. The original `f3bbee0` fix still followed a
  symlinked intermediate suffix component; the new `fixed_production_presence_rejects_a_symlink_ancestor` regression
  is red there and green after descriptor-walking from the operator-home anchor in `f700cde`. Removing either root
  identity check or the boundary refusal makes its positive or negative assertion fail. The argument-bearing
  `schedule-tick` test also fails against the prior R3d2-pending status contract.
- The first warnings-denied exact-final Clippy run found four `large_enum_variant` failures in the new transaction
  records. `f6a33af` boxes only the large payloads (and both published-result arms); Serde remains transparent, and
  all 16 canonical journal/recovery tests remained green. The first complete binary run then exposed a pre-existing
  R3d2d idempotency contradiction: **638/1/0**, with
  `reservation_restart_and_reconciliation_crash_points_are_conservative_and_idempotent` rejecting its same-request
  reopen as `attempt id was rebound`. An isolated rerun failed **0/1**, ruling out test ordering. `2b333ba` makes
  `prepare_reservation` return the durable original record after `check_headroom` proves an identical request, while
  a new same-attempt/different-case assertion still refuses. The exact regression is now **1/0**, full ledger is
  **12/0**, binary is **639/0/0**, and the full workspace is green.

Reviewed-candidate deterministic evidence at `1373985` before first Sol review:

- Format and tracked whitespace checks, workspace all-target/all-feature compilation with `RUSTFLAGS=-D warnings`,
  warnings-denied all-target/all-feature Clippy, locked release build, and dependency policy are green. `cargo deny`
  reports its configured duplicate-version warnings and ends `advisories ok, bans ok, licenses ok, sources ok`.
- Repository hygiene is **37 tracked artifacts / 7 validated example configs**. The pinned manifest validates at
  **9 cases**, floating recipes at **4 cases**, and the schedule foundation at **6 scheduled advisory / 4 claimed-
  support profiles** with profile-policy-bundle SHA-256
  `aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`.
- Transaction/journal/source/recovery/terminal tests are **16/0**; local state/root/lock tests are **13/0**; the
  pure legacy authority boundary is **1/0**; and the complete compatibility CLI integration suite is **22/0**.
- The complete `a2a-bridge` binary suite is **639 passed / 0 failed / 0 ignored**. The full serial workspace is
  **2,376 passed / 0 failed / 12 ignored** across **72** reported test/doc-test targets. The ignored set is the
  existing explicitly live/authenticated Codex, Kiro, Claude, Gemini, and local-Ollama integration tests; none was
  unignored or invoked.
- The locked release binary is **26,617,024 bytes**, SHA-256
  `a11af936e077706dc7a9c670d8249f371a5ec754c5e904202fb2dc2e40751e67`.

Not verified or authorized at this checkpoint: any real authority issuance, production scheduler root creation,
live admitted runner handoff, provider/model/credential/registry/image/runtime/GitHub/iCloud effect, timer/watcher/
launchd installation, production-operator lifecycle action, evidence/status/retention publication, Sol closure
approval, or the independent Fable release lens. R3d2 is mechanically complete but deliberately non-activatable;
R3d5 remains the sole activation owner.

#### First Sol implementation review and remediation — 2026-07-19

The bridge-mediated `gpt-5.6-sol`/xhigh/read-only review froze exact candidate
`1373985cde2b7f0d8b7b97a39757dad42a254a22` against merged R3d1 base
`cbcfd1f06b914064456d1798be71bacdc294f3d5` and returned `R3D2 IMPLEMENTATION: REVISE` with four `WRONG` and
one `SMELL`. Its complete SSE artifact is retained at
`/private/tmp/a2a-bridge-r3d2-sol-review-02.sse`, 24,901 bytes, SHA-256
`0f9bc9f8251f5fcf2a2bef166b6636fe8ffcd1e3a52cdba1597c17f1ec1f56c5`:

1. completed reconciliation trusted caller-supplied nominal terminal identity/usage instead of the joined child
   aggregate;
2. safe-session reuse was unreachable because proposal preparation always allocated ledger/supervisor artifacts;
3. fixed-root canonicalization could follow an intermediate suffix symlink;
4. this master plan and its restart cursor still described R3d2a/b rather than the reviewed R3d2a-e boundary; and
5. `SMELL`: the takeover contract did not explicitly say whether standalone effectful `compatibility resolve`
   remained available.

`f481f39` closes items 1-2 with the opaque aggregate proof, conservative missing-telemetry accounting, and full
standing-reuse regression; `cdf833a` also retains the aggregate terminal time instead of substituting reconciliation
time. `f700cde` closes item 3 with descriptor-relative optional child traversal and the
intermediate-symlink regression. This docs fold closes item 4 and adjudicates item 5 as the intentionally separate
R3c resolver boundary described above. Post-remediation focused gates are transaction **17/0**, state/root/locks
**14/0**, supervisor **41/0**, and local-file **11/0**; warnings-denied workspace check and all-target/all-feature
Clippy are green. The exact remediation binary is **641/0/0** and the serial workspace is
**2,378/0/12 ignored** across **72** targets. Format/diff, locked release, dependency policy, hygiene **37/7**,
manifest **9**, recipes **4**, foundation **6/4**, compatibility CLI **22/0**, and the pure legacy boundary **1/0**
are green. The release binary is **26,604,912 bytes**, SHA-256
`5454b5eb38ca7454bd1e3c9feae7d1c97e6565602d704ff5f434bc7e7479f584`. The earlier **639/0/0** and
**2,376/0/12 ignored** totals remain labeled reviewed-candidate evidence rather than being conflated with this
changed head. The Sol closure result and its subsequent remediation are recorded next.

#### Second Sol closure review and remediation — 2026-07-19

The bridge-mediated `gpt-5.6-sol`/xhigh/read-only closure review froze exact candidate
`28e7d28a3759d38cb98eea63e68bba9000ac5138` against merged R3d1 base
`cbcfd1f06b914064456d1798be71bacdc294f3d5` and returned `R3D2 IMPLEMENTATION: REVISE`. Its report is retained
at `/private/tmp/a2a-bridge-r3d2-sol-closure-28e7d28/review.md`, 16,277 bytes, SHA-256
`5d51097976a691aa52eddc6fb68262d4b876f6ad1c676ed69b2fd824ab560162`. It independently reran transaction
**17/0**, state/root/locks **14/0**, preflight **11/0**, compatibility CLI **22/0**, and the exact diff/boundary.
It marked the terminal-child proof, safe reuse, fixed-root traversal, and standalone-resolver scope `RESOLVED`; it
left the release cursor `UNRESOLVED` and found three new mechanism failures:

1. valid preflight passes for source A could be replayed into distinct admission/source B because the pass carried
   no subject, authority-head, directory, or commit-time binding and the transaction accepted caller-built passes;
2. admission accepted only an arbitrary supervisor deadline digest, not the validated derivation or executable
   authority-contained `HardDeadline`; and
3. the same-process owner/authority check-then-increment sequences could interleave and return both lock
   capabilities.

All three pre-change regressions failed on the reviewed mechanism before remediation:

- `preflight_passes_cannot_be_replayed_across_admissions` admitted source B using source A's valid passes;
- `arbitrary_deadline_digest_cannot_admit` accepted a syntactically valid arbitrary digest; and
- `concurrent_owner_and_authority_publication_cannot_return_both_capabilities` forced the interleaving with a
  barrier and observed both guards returned.

Mechanism commit `f18e74aa970a0b30f84ede376ae51e2f214f3f9b` closes those failures. Each preflight pass now binds a canonical
digest of the exact selected source/admission, authority snapshot, effect envelope, ledger/supervisor/deadline
records, action-directory identities, authority terminal deadline, and commit time. The session exposes one
`admit` operation that runs both fences internally and commits without returning a pass or proposal to its caller.
`PreparedSupervisorV1` now owns the full validated supervisor record plus its non-`Clone` executable
`HardDeadline`; admission verifies exact run/window/case identity, selected-case ledger timeout, and remaining time
against the authority terminal window, persists the full derivation in the commit, revalidates it on reopen, and
moves the same deadline into the admitted handoff capability. The owner and authority-only same-process
transitions now serialize check, nonblocking file lock, and holder publication under one process-local mutex;
cross-process exclusion remains the existing nonblocking `flock` boundary.

The added negative matrix rejects a mutually self-consistent deadline/supervisor pair for the wrong run, wrong
window, wrong case, over-cap case timeout, or overlong authority window. The positive handoff proves it carries the
same live derivation that admission committed. After formatting, focused gates are preflight **11/0**,
state/root/locks **15/0**, supervisor **41/0**, and transaction **20/0**; `git diff --check` and all-target
`cargo check -p a2a-bridge` are green.

Exact docs-fold candidate `840f486ab24bf53848b985d76d845e8274db91d9` then passed the complete deterministic
gate. Format/diff, warnings-denied workspace all-target/all-feature check and Clippy, locked release build,
dependency policy, repository hygiene **37/7**, manifest **9**, floating recipes **4**, schedule foundation
**6/4**, and the complete binary **645/0/0** are green. The one-job/one-thread full workspace is
**2,382/0/12 ignored** across **72** result groups, **55** nonempty. The ignored set remains the existing explicitly
live/authenticated provider integrations; none was unignored or invoked. The release binary remains byte-identical
at **26,604,912 bytes**, SHA-256
`5454b5eb38ca7454bd1e3c9feae7d1c97e6565602d704ff5f434bc7e7479f584`, consistent with the internal mechanism
remaining unreachable from production routes. The previously reported unrelated
`warm_streaming_records_usage_without_emitting_usage_frame` raw-substring/UUID flake did not recur; no unrelated
test was rebaselined or changed.

#### Third Sol closure review and remediation — 2026-07-19

The bridge-mediated `gpt-5.6-sol`/xhigh/read-only closure re-review froze exact candidate
`d082b499d26d2c8d52fbab69e484dc6c000a7196` against merged R3d1 base
`cbcfd1f06b914064456d1798be71bacdc294f3d5` and returned `R3D2 IMPLEMENTATION: REVISE`. Its prompt is retained
at `/private/tmp/a2a-bridge-r3d2-sol-closure-d082b49/review-task.md`, 9,627 bytes, SHA-256
`ae3863fb1519b7196f52ccf9d3a75d482a41d9109632d5e10d01321bb3b3f2be`; its report is retained at
`/private/tmp/a2a-bridge-r3d2-sol-closure-d082b49/review.md`, 15,737 bytes, SHA-256
`5f90a07d795e73c619f860a4dfe68cf10c545a992e7d776ce6ee8a5a048f79da`. It independently reran transaction
**20/0**, state/root/locks **15/0**, preflight **11/0**, supervisor **41/0**, compatibility CLI **22/0**, the exact
diff, and a same-process separate-open `flock` probe.

The review marked the terminal proof, safe reuse, fixed-root traversal, standalone resolver scope,
admission-bound preflights, executable hard-deadline join, and lock-publication race `RESOLVED`. The release cursor
and restart cursor remained `UNRESOLVED`. It found no new `WRONG` and one new `SMELL / Medium`: after durable
publication, the capability deadline could expire before runner invocation because `handoff_admitted` did not make
a final local deadline check.

The deterministic regression `deadline_expired_after_durable_publication_never_calls_the_runner` failed **0/1**
on the reviewed mechanism: forced post-publication expiry still invoked the runner, ruling out implicit runner or
capability enforcement as the alternative explanation. Commit
`248e3733662ebb59eaf9f2c5d80de790fe1f9c50` adds a final `remaining() > 0` fence immediately before invocation.
Expiry now refuses without invoking the runner, consumes the one-shot capability, and preserves the conservative
pending reservation for later reconciliation. The regression, existing positive handoff, and complete transaction
module are green at **1/0 + 1/0 + 21/0**; format and diff checks are green. This docs fold closes both literal cursor
residuals without changing the mechanism.

That remediation made a fresh Sol/xhigh closure re-review of the exact cursor-remediation head the next semantic
gate. The controller froze head/base/merge-base/changed paths and supplied exact-head full deterministic evidence;
the resulting fourth review is recorded below. Fable remained blocked because that review did not approve.

#### Fourth Sol closure review and remediation — 2026-07-19

The bridge-mediated `gpt-5.6-sol`/xhigh/read-only closure re-review froze exact candidate
`c418df41a209391e1600a441faed27bad3a055d0` against merged R3d1 base
`cbcfd1f06b914064456d1798be71bacdc294f3d5` and returned `R3D2 IMPLEMENTATION: REVISE`. Its prompt is retained at
`/private/tmp/a2a-bridge-r3d2-sol-closure-c418df4/review-task.md`, 11,027 bytes, SHA-256
`a5363563e15acd870488a8992db490df27f74b01db099711dc727882bafd5f15`; its mode-`0644` report is retained at
`/private/tmp/a2a-bridge-r3d2-sol-closure-c418df4/review.md`, 18,784 bytes, SHA-256
`704678853b95b2b17577affba573917823b5347f8c4a95ef4f2884235b98be6a`. It marked all ten inherited findings
`RESOLVED`, confirmed the branch/status boundary was literally consistent, and found:

1. `WRONG / Medium`: standing reuse could consume evidence at a clock time earlier than the evidence terminal time
   because reuse skipped the ledger's durable rollback fence;
2. `WRONG / Medium`: exact advisory evidence selected for a direct-local manual request was rejected by transaction
   validation instead of consuming the one-run manual authority; and
3. `SMELL / Medium`: supervisor publication dropped the retained state-directory capability to a pathname and could
   split authority/admission/ledger state from supervisor state after directory replacement.

All three deterministic regressions failed on the reviewed mechanism before remediation:

- `equivalent_work_refuses_reuse_when_clock_precedes_selected_evidence` terminalized at 15, attempted reuse at 14,
  and failed **0/1** because reuse succeeded instead of leaving state unchanged; its time-16 positive remains green;
- `completed_work_reuses_for_standing_and_manual_authority_without_new_effects` failed **0/1** with
  `one-shot/manual work cannot reuse evidence` after the reducer selected exact reusable advisory evidence; and
- `supervisor_directory_replacement_refuses_publication_without_a_capability` failed **0/1** because publication
  returned a capability and wrote the pathname replacement.

Mechanism commit `5a01ce768f2e8d9a77d68d111d7314085f1513e8` closes them. Equivalent-work validation now binds every consumption
to an evidence-availability watermark (terminal time, or the later review time for reviewed characterization) and
atomically refuses rollback. Reused dispositions now permit only the exact standing/standing pair or an exact
`ManualAcknowledgement` plus manual action for `ProviderPathAdvisory`; characterization and `ManualDiagnostic`
remain non-reusable, the manual nonce is durably consumed once, and replay fails before a second commit.
`FileSupervisorJournal` now owns the scheduler's `PinnedDirectory`, scans through its stable object path, performs
create/open/sync relative to the retained descriptor, and verifies that the canonical pathname still names that
object before and after journal operations. The replacement regression returns no admitted capability and leaves
both the replacement and retained directories empty.

Post-remediation focused gates are admission **17/0**, supervisor **41/0**, and transaction **22/0**; format/diff,
warning-denied workspace all-target/all-feature check, and the three isolated regressions are green. The complete
exact-head deterministic gate has not yet been rerun after this mechanism change. The next semantic action is that
full gate, followed by a fresh Sol/xhigh closure re-review explicitly adjudicating these three findings. Fable
remains blocked until Sol approves.

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

Resume branch `agent/reliability-r3d2-authority-admission` in a newly verified clean trusted worktree; do not depend
on a prior `/private/tmp` worktree or review mirror. The branch is based on merged R3d1 main
`cbcfd1f06b914064456d1798be71bacdc294f3d5`. Fourth review of exact `c418df4` resolved all ten inherited items and
returned `REVISE` with two new `WRONG` plus one retained-directory `SMELL`; mechanism commit `5a01ce7` closes all
three with demonstrated pre-change-red regressions and focused admission/supervisor/transaction gates
**17/0 + 41/0 + 22/0**. Read
this plan, the R3d design of record, the durable roadmap, `AGENTS.md`, and
`skills/a2a-bridge-operator/SKILL.md` before editing. Preserve the single R3d2 merge boundary, the
owner-wide-then-authority lock order, the single admission linearization point, the zero-effect default, and the
separation between provider authority and storage consent. The next semantic action is the complete exact-head
deterministic gate, then a fresh Sol/xhigh closure re-review explicitly adjudicating the clock-rollback, manual-reuse,
and retained-supervisor-directory findings. Its prompt must carry the frozen exact boundary and gate evidence. Run
the single Fable lens only after Sol approves; then fold final evidence, rerun exact-final gates, and publish the
non-draft PR.
