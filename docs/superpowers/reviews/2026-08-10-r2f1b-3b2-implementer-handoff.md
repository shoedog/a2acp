# R2f1b 3b2 — process authority II implementer handoff

Base: `b8471c24`
Date: 2026-08-10
Scope: destructive wrappers plus the single consolidated `AgentBackend` trait pass.

## 1. Trait-surface decision

The pass adds two synchronous, capability-free inspection/attachment methods:

```rust
fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError>;
fn attach_resource_flight_owner_v1(
    &self,
    session: &SessionId,
) -> Result<BackendResourceFlightV1, BridgeError>;
```

`BackendResourceFlightV1` is closed over `LegacyV2 | ProtectedV3`. Both methods default to
`Err(BridgeError::ResourceFlightUnsupported)`. The exposure value carries no signal capability;
it only lets a wrapper prove that the concrete backend generation owns the destructive operation
it is about to delegate. Refusing defaults make an unmodified decorator stop before effects.

The same pass changes all four checked cleanup methods to return
`Result<BackendCleanupDispositionV1, BridgeError>`:

- `forget_session_checked`
- `forget_session_observed`
- `release_session_checked`
- `release_session_observed`

`BackendCleanupDispositionV1` is a new core-owned type rather than a workflow wire type. Its values
are `Complete`, `Retained`, `Preserved`, and `Unknown`. This keeps adapter/session semantics out of
the persisted-history layer while allowing exact projection at that boundary. Composition uses
`Unknown > Preserved > Retained > Complete`; thus an outer preservation cannot erase inner
retention, and an ambiguity cannot be downgraded. Legacy checked-cleanup defaults return
`Complete` after their existing unit cleanup.

### Production overrides

| Backend | Exposure | Attachment | Cleanup behavior in 3b2 |
|---|---|---|---|
| `AcpBackend` | Reads the retained `OwnedProcessTreeV1` generation and reports V2/V3 | Attaches the session owner through the existing process flight, then reports that exact generation | Existing session-only cleanup maps to `Complete`; process retirement remains flight-owned |
| `WorktreeBackend` | Forwards to its inner backend | Forwards to its inner backend | Composes exact inner and checkout dispositions; direct cleanup and retire refuse before effects when the inner flight is unexposed |
| `ContainerRwBackend` | Explicit `LegacyV2` for this surface-only sweep | Refuses until 3c1 owns container attachment | Existing reaper results map to `Complete`; no 3c1 teardown behavior is implemented here |
| `ApiBackend` | Explicit `LegacyV2` for this surface-only sweep | Refuses until 3c2 owns request attachment | Uses the legacy default cleanup mapping; no 3c2 request flight is implemented here |
| `ReplayBackend` | Refuses | Refuses | No V3 claim is possible |

This does not arm a production V3 route.

## 2. Census and doubles ripple

The task's 120-site base census is source-only. Re-running
`git grep -nE 'impl( <[^>]*>)? AgentBackend for' -- '*.rs'` on this tree reports **122**:
the original 5 production implementations plus 117 test/harness implementations. The explained
`+2` drift is two purpose-built negative doubles added by this slice:

- registry `RefusingBackend`, which exposes the wrapper's pre-retire refusal point;
- Worktree `UnforwardedInner`, which intentionally inherits the new refusing defaults.

The pre-existing 115 non-production implementations absorb flight methods through refusing
defaults. Only doubles that override checked cleanup were mechanically migrated to the typed
return, and V2-path doubles used by changed wrappers explicitly expose `LegacyV2`. This preserves
the coverage signal: a V3-path double must opt in, while unrelated doubles do not acquire
authority accidentally.

## 3. Wrapper boundary and V2/V3 split

- Registry retirement funnels drain, resolve race-loss, invalidation, reload, and keyed subslot
  retirement through one `retire_join_or_refuse` check. Lease draining remains universal; an
  unsupported generation stops before `retire`, while either known generation delegates to the
  backend that owns its flight.
- `ResilientWarm::retire` exposes before delegation. Transient bookkeeping/classification remains
  universal. `LegacyV2` retains cancel → retire → reset → rebuild byte-for-byte; `ProtectedV3`
  delegates only the backend-owned cancel and makes retire → reset → rebuild unreachable.
- Session-manager claim/tombstone bookkeeping remains universal. Every fresh or retry cleanup
  flight exposes immediately before checked release. Refusal settles as a retryable cleanup
  failure without re-entering backend teardown.
- Coordinator dispatch owns no signal capability. It receives the session-manager result and
  projects `complete`, `retained`, `preserved`, `unknown`, or `failed` exactly through detached
  cleanup and terminal settlement.
- Worktree custody/preservation bookkeeping remains unchanged. Flight exposure occurs before
  preservation, inner cleanup, provider removal, or sidecar removal; retirement also exposes
  before sealing/draining. Known V2/V3 generations delegate to the inner owner.

Explicit refusal tests cover registry drain, invalidation, reload, keyed retirement, and resolve
race-loss; the session cleanup-retry capability; coordinator detached cleanup; and Worktree direct
cleanup/retire. Existing V2 controls remain in place, including registry lease-drain/reload/
invalidation/keyed tests, `transient_death_respawns_once_and_completes`, session release/retry
tests, `actual_prompt_barrier_failure_cleans_up_then_terminalizes_once`, and Worktree's V2 teardown
controls.

## 4. Disposition and composition evidence

The three session-manager tests independently assert that `Retained`, `Preserved`, and `Unknown`
arrive unchanged and are not reported as `Complete`. Coordinator tests project those same three
values into terminal history, whose validation vocabulary now admits `retained` and `preserved`.

Worktree tests cover each inner protective result, outer-preserved + inner-retained,
outer-retained + inner-preserved, and outer-preserved + inner-unknown. They also count the inner
cleanup call, pinning one signal per composed flight.

## 5. Post-change composition basis

The production factory still constructs `WorktreeBackend` directly around `AcpBackend`.
`ContainerRwBackend` remains a separate decorator constructed directly around `AcpBackend`; it is
not outside a custody-owning Worktree backend. Therefore defaulted custody methods remain
unreachable by the same spawn-factory construction basis recorded in 2c2.

Flight composition is now stronger than the old custody-only basis: Worktree explicitly forwards
both flight methods and refuses direct cleanup/retire before effects if an inner decorator fails to
forward them. `default_unforwarded_flight_cannot_signal_through_worktree_cleanup_or_retire` proves
the negative, while `protected_flight_exposure_and_attachment_forward_to_inner_teardown` proves
the positive. Container flight forwarding remains deliberately owned by 3c1.

## 6. Real-host closure and frozen slots

`protected_v3_child_grandchild_stop_then_child_first_kill_has_no_live_leaks_host_signal_semantics`
spawns a real root and a real TERM-ignoring descendant through the V3
`OwnedProcessTreeV1` constructor. A pipe handshake replaces sleep-race readiness. The test drives
zero-grace closure, requires journaled SIGSTOP observations before SIGKILL for both processes,
requires descendant SIGKILL before root SIGKILL, and boundedly proves neither PID remains live.

`crates/bridge-core/src/retained_resource_flight.rs` has no diff.
`LIFECYCLE_SLOTS = 4`, `PROCESS_LIFECYCLE_SLOTS = 7`, and the exact
`process_lifecycle_reserved` golden with `reserved_lifecycle_slots:7` remain unchanged.

## 7. Verification record

Completed:

- `cargo fmt --all` and `cargo fmt --all -- --check` parse/format gate;
- `CARGO_INCREMENTAL=0 cargo build --workspace --all-targets --locked --offline`;
- `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --all-features --locked --offline --
  -D warnings`;
- `RUSTDOC=<toolchain-rustdoc> CARGO_INCREMENTAL=0 cargo test --workspace --locked --offline
  --exclude bridge-container -- --skip _host_signal_semantics --skip
  lock_release_failure_is_loud_not_silent --skip staged_candidate_`: across 88 harness results,
  **3,858 passed, 0 failed, 12 ignored, 0 measured, 16 filtered out**;
- the explicitly selected real-syscall containment test, outside the hermetic suffix skip:
  **1 passed, 0 failed, 0 ignored** (546 unrelated bridge-core tests filtered out);
- focused post-fix targets: inbound **223/223**, MCP client **10/10**, and deletion gate **5/5**;
- `cargo run -p a2a-bridge --locked --offline -- validate --repo-hygiene`;
- `git diff --check`;
- source-only trait census: 122, reconciled above;
- frozen-slot constants/golden and read-only-file diff checks.

No billable provider workflow or smoke was run.

## 8. Review evidence admissibility

The initial review attempt timed out during authentication, before initialization or diff
inspection, and produced no findings. That attempt is inadmissible as correctness evidence and is
not counted as approval. The merge gate must remain fail-closed until a subsequent reviewer
successfully initializes and substantively inspects this security-critical diff; retrying the same
review target does not authorize a code or acceptance-criteria waiver.

## 9. R1-R4 repair record

R1 widens both `MemoryWorkflowHistory::settle_cleanup` and
`SqliteStore::settle_cleanup` to the full closed terminal vocabulary:
`complete|retained|preserved|unknown|failed`. `pending` and arbitrary strings still fail with
`LedgerUnavailableReason::Schema`, identical replay remains idempotent, and a different second
terminal disposition remains `TerminalWrite::Conflict`.

Three end-to-end coordinator tests drive `retained`, `preserved`, and `unknown` independently
through the real `finish_with_detached_cleanup` path. Every test runs against both Memory and
SQLite history, waits for the detached settlement, and asserts the exact durable terminal value,
the invalid-value refusals, and the conflicting-second-write refusal. For red proof, an archive of
the exact pre-repair base `90359127` received only the three new test hunks: all three tests failed
and each failure named both `memory` and `sqlite`. The repaired focused run is **22 passed, 0
failed**.

R2 now projects successful sibling cleanup from the returned typed disposition; only an actual
cleanup error maps to `failed`. R3 adds load-bearing ordering comments at both ACP attachment sites:
internal attachment may see the helper's `None` branch only after spawn has published the
supervisor, while public attachment must retain the adjacent post-attach resource-flight re-read.
R4 funnels all four discarded registry retirement results through one warning path. Flight refusal
and retire failure have distinct static `failure_kind` values, the agent id is the only dynamic
identity field, and the detached retirement control flow is unchanged.

Repair verification completed:

- `cargo fmt --all -- --check` and `git diff --check`;
- `CARGO_INCREMENTAL=0 cargo build --workspace --all-targets --locked --offline`;
- `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --all-features --locked --offline --
  -D warnings`;
- the hermetic workspace suite with its configured exclusions/skips and the active toolchain's
  absolute `RUSTDOC`: across 88 harness results, **3,861 passed, 0 failed, 12 ignored, 0 measured,
  16 filtered out**;
- focused repaired targets: coordinator detached cleanup **22/22**, registry **45/45**, inbound
  **223/223**, store cleanup **5/5**, and ACP process-owner attachment **3/3**;
- `CARGO_INCREMENTAL=0 cargo build --release --bin a2a-bridge --locked --offline`;
- `cargo run -p a2a-bridge --locked --offline -- validate --repo-hygiene`;
- the frozen `retained_resource_flight.rs` has no diff from `b8471c24`; `LIFECYCLE_SLOTS = 4`,
  `PROCESS_LIFECYCLE_SLOTS = 7`, and the `reserved_lifecycle_slots:7` golden remain unchanged.

The aggregate runner needed `NO_PROXY=127.0.0.1,localhost` so its loopback `wiremock` request did
not enter the ambient egress proxy. No billable provider workflow, compatibility run, or smoke was
run for this repair.
