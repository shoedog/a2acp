# R2f1b 3c1 — container authority implementer handoff

Base: `f397ee5f92b24f2458656dc4ba2524052d78c8a2`
Date: 2026-08-11
Scope: typed ContainerRw spawn evidence, immutable-ID container flights, and the ContainerRw teardown census.

## 1. Typed spawn result and pre-ID state

`ContainerSpawn` now receives `ContainerSpawnRequestV1 { runtime, name, ownership }` and returns
`ContainerSpawnResultV1 { backend, immutable_container_id, ownership_labels }`. Production obtains the result's
identity evidence with a bounded runtime inspect after the ACP backend has spawned. A name remains a discovery
selector only; it is not destructive authority.

Before that result is validated, `ReapOwner` contains `SpawnAuthorityState::Pending` and carries no controller.
Any teardown entrance atomically changes Pending to `RefusedUnknown`. `SpawnSettlementGuard::drop` does the same
for cancellation/unwind of the spawn future. A later spawn result cannot bind over that refusal. Consequently the
resource may exist but removal and subordinate ACP cancellation both remain uncalled, and checked cleanup reports
`Unknown`. This is intentional sparing behavior, not a delayed name-based cleanup.

The cold/warm cancel, cold/warm retire, cold/warm checked-release, aborted-spawn, and unexpected-label controls all
assert this boundary. The resource-bearing control additionally proves the late-created object remains present and
the injected removal port has zero calls.

## 2. Container flight and durable wire

After spawn evidence validates, the generation key is `container-id:<immutable_container_id>`. Construction reserves
one `ResourceFlightKeyV1::ContainerGeneration`, attaches the spawn/session owners, and appends
`ContainerIdentityCaptured { ResourceIdentityV1::ManagedContainer { ... } }` while admission is open. Teardown then
closes admission, journals intent, journals dispatch, re-observes the selector, validates the exact immutable ID and
complete `a2a.*` label namespace, and only then calls removal with the captured ID. It appends
`ContainerRemovalObserved` and settles through the same retained flight.

The two journal events are additive. Exact JSON goldens and top-level/nested unknown-field negatives were added;
all existing variants and the frozen `LIFECYCLE_SLOTS = 4` / `PROCESS_LIFECYCLE_SLOTS = 7` values are unchanged.
The controller is the first production caller of `journal_container_identity`.

`DurableProcessFlightAttemptV3::bind_container_generation` reuses the attempt's exact registry, journal, attempt ID,
and publisher. Its pointer-identity control now spans two process descriptors and one container descriptor. It never
constructs a registry from journal records, and the durable descriptor's generation must equal the identity row.
Production still supplies `None`, so slice-4 V3 arming remains route-unarmed.

## 3. Canonical ownership labels

`ContainerLabels::canonical_ownership` is the single constructor used by argv composition, spawn-result validation,
and removal-time validation. Digest bytes are JSON over the constructor's pinned label order. Runtime validation is
order-independent but requires the exact `a2a.*` namespace: duplicate, missing, changed, or extra `a2a.*` keys
refuse. Unrelated image/runtime labels are deliberately outside the ownership namespace.

The fixed sample digest is
`4126b2d6672d795aaf23bd4b819f6b3449e9484466bd87525c2e81119624f055`. Tests pin order and digest, accept runtime
reordering, and reject an extra `a2a.future` both in core and at the typed spawn boundary.

## 4. Exposure and composite ownership

Without an attempt route, ContainerRw reports `LegacyV2`. With the explicit route it reports `ProtectedV3`; no
production constructor supplies that route in this slice. Attachment before a per-turn/warm generation exists is a
future-generation declaration: reservation automatically records the real session owner. Attachment after
publication reaches the exact bound controller and flight. It never derives a capability from the planned name.

The outer container controller owns one subordinate closure for `inner.cancel(session)`. After the immutable-ID and
label gate passes, inner ACP cleanup and exact-ID removal run concurrently inside the one controller attempt. Every
ContainerRw teardown entrance joins that controller, so cancel plus stream drop cannot invoke either half twice.
Recycled-name or label-drift refusal happens before both halves. In V3, a subordinate failure cannot collapse behind
a successful outer removal; it settles as `container.reap.subordinate_cleanup_failed` and projects protectively.

The inner ACP process route and outer container descriptor share the one attempt registry. The process still owns
its live signal capability; the container flight invokes it only through `AgentBackend::cancel`, exactly once.

## 5. Binding-row decisions: lattice and S2

The shipped lattice remains `Unknown > Preserved > Retained > Complete`. Reordering it globally would change the
already-landed Worktree composition contract. The container-leak scenario is not hidden on the reachable production
shape: the factory constructs ContainerRw directly around ACP, not inside Worktree, and ContainerRw returns its
outer `Retained`/`Unknown` result directly to checked cleanup. The protected-container regression forces a recycled
identity and asserts the decorator returns `Retained`, with zero removal calls. A future architecture that wraps
ContainerRw in a preservation-owning decorator must adopt the sanctioned two-field split or revisit the lattice;
that architecture is not constructed here.

For S2, protective container results are not held only in an evictable cleanup cell. `session_reaps` retains the
exact `ReapOwner` until and unless the result is `Complete`; `Unknown` and `Retained` keep it resident, block a new
generation for that session, and let later checked/observed cleanup rejoin the stable result. This backend-lifetime
capability is the durable re-derivation mechanism for ContainerRw and cannot restart at `Complete` across cleanup-cell
lifetimes.

## 6. Teardown census reconciliation

All source-enumerated ContainerRw entrances now funnel through `ReapOwner::reap_detached/reap_observed`:

- `SpawnSettlementGuard::drop`: `aborting_spawn_future_refuses_pre_id_cleanup_as_unknown`.
- cancel plus stream ownership (`ContainerReaper`, its Drop, and double-reap):
  `cancel_reaches_inner_and_reaps_once`, `stream_completion_reaps_once_and_clears_inflight`, and
  `early_drop_reaps_once`.
- cold/warm cancel, retire, and checked release during Pending: the six cold/warm pre-ID controls.
- final prompt installation races: the six `*_waits_for_winning_inner_prompt_dispatch` controls.
- spawn/prompt/configuration failures: `prompt_spawn_failure_refuses_pre_id_removal_and_errors`,
  `launch_failure_is_preserved_without_keyword_promotion_or_name_reap`,
  `cold_cancel_during_turn_configuration_prevents_late_dispatch`,
  `warm_cancel_during_first_turn_configuration_prevents_late_dispatch`, and
  `warm_edit_turn_open_failure_refuses_name_removal_and_clears`.
- forget/release checked and observed: `observed_cold_release_reports_unknown_after_agent_spawn_failure`,
  both canceled-release controls, both observed-forget controls, and the warm release controls.
- retire/backend Drop/off-runtime reaper Drop: `retire_cancels_and_reaps`,
  `warm_retire_reaps_cached_container`, `dropping_warm_backend_starts_cached_container_cleanup`, and
  `off_runtime_reaper_drop_does_not_panic`.
- `TurnGuard::drop` remains intentionally non-destructive and epoch-scoped; warm live cancel is turn-scoped and
  retains the container. Their controls are `warm_stale_turn_guard_clear_is_epoch_scoped`,
  `warm_cancel_clears_turn_active_without_reaping`, and `warm_cancel_then_reprompt_survives_old_stream_drop`.

`session_reaps` is now the retained generation owner, not a second removal path. Prompt/configure/cancel/release,
stream Drop, backend Drop, and retirement therefore cannot bypass flight admission.

## 7. Recycled-name and no-name-removal proof

The injected runtime probe control `recycled_name_refuses_every_composite_flight_action` supplies the same selector
with a different ID. Repeated detached/observed entries return the same `IdentityChanged`; removal and subordinate
counts remain zero. The valid control proves the removal port receives `sha256:captured`, never `stable-name`, and
both composite halves run once.

Source grep used for the handoff:

```text
rg -n 'args\(\["rm", "-f", (name|selector)|reap_argv\([^\n]*a2a-(rw|ro)|rm -f <name>|rm -f.*name' crates bin --glob '*.rs'
# no output
```

The remaining destructive argv sites receive variables named `immutable_id`/`id`; recovery and the explicit
operator reap surface already discover runtime IDs, while live ContainerRw removal additionally requires the
journaled, admission-closed controller flight. `sandbox::reap_argv` is now explicitly ID-only. There is no
`ReapController::from_legacy` use in ContainerRw.

## 8. V2 controls and verification

V2 remains the production default. Existing successful-path controls preserve one reap for stream completion,
early drop, cancel plus stream drop, cold/warm release, post-ID prompt/configure failure, retire, and backend Drop. The
managed V2 controller uses the same admission/journal/identity gate in memory; injected `ReapFn` still receives one
target and all stable typed runtime failures remain one-shot. The mandated pre-ID case is the intentional behavior
change: it now spares with `Unknown` instead of waiting to acquire future authority.

Completed locally:

- `cargo fmt --all` and `cargo fmt --all -- --check` — green;
- `git diff --check` — green;
- the no-name-removal and producer/census source greps above — green;
- landing size: code 1,971 insertions / 305 deletions; with this 150-line handoff 2,121 / 305, below the 3,300-line stop tripwire.

Compilation, Clippy, and test totals are not claimed in this clone. The online workspace check was refused by the
environment's crates.io CONNECT proxy while fetching `a2a-lf`; the locked offline check then reported missing cached
`arc-swap`. No provider workflow, smoke, compatibility case, container daemon, release, deployment, or running
operator was invoked. The operator host gate must run the bridge-container deterministic tests plus the configured
workspace build/Clippy/suite and record exact totals before fold.

## 9. Operator gate repair (2026-08-11, post-landing): retire() flight-join deadlock in the retirement/observed-release join test

The darwin host gate (first run on `ed840e81`) wedged 3+ hours in `cargo test --workspace` at the
pre-existing (R2b3-era) test `tests::retirement_and_observed_release_join_without_retaining_observer`
(bridge-container). Controls, both same-host: standalone repro at `ed840e81` hangs >60s (watchdog-killed;
stack sample shows the tokio current-thread driver parked with nothing runnable); the identical invocation
at parent `f397ee5f` passes in 0.00s. Confirmed regression of this slice's diff.

Mechanism: this slice's `retire()` now JOINS each generation's reap flight before returning
(`owner.reap_observed().await`) where the pre-slice `retire()` started reaps detached and returned. The
test gated its injected reap attempt on a `Notify` released only after `retire().await` returned — a cycle
(`retire().await` ← flight ← attempt ← `release.notified()` ← notify after retire returns), so the old test
structure deadlocks by construction under the new semantics.

Adjudication (operator, on the mandate): repair choice (a) — test-semantics restructure. Joining the flight
before returning is the mandated teardown semantic (B16 teardown census; the 3b2 join-or-refuse wrapper
binding). The 3a typed-join-refusal contract governs journal-failure joins, not healthy in-flight attempts,
so it does not forbid this join. The repair runs `retire()` on a spawned task concurrently with the gated
attempt and preserves the test's invariants unchanged: exactly one reap-attempt call end-to-end (the
observed release JOINS the in-flight retirement reap, never re-dispatches), the settled controller retains
no observer (`Weak::upgrade` → `None`), and `calls == 1` at the end. Both joins are now bounded by
`tokio::time::timeout(30s)` so a future join regression fails loudly instead of wedging the workspace gate.
Red was observed twice (gate wedge + standalone probe); green: the restructured test passes in 0.01s and
the full bridge-container harness is 62/62.

Dispatch note: the bridge repair dispatch died at Authenticate with the ledgered
`spawn xdg-open ENOENT` signature (recurrence #2 of the single-token-family rotation flaw; clone
`impl-44182-chpc52z8` stranded clean, reaper-eligible). Repair executed operator-side under the
owner-sanctioned fallback; redispatch stays blocked until an owner `codex login` reseed.

Disclosure for the lens round (observed during the trace, deliberately NOT fixed here): the new `retire()`
no longer calls the pre-slice `inner.cancel(&session).await`; the removed inner backend is bound to
`_inner`, held uncancelled across the flight join, and dropped after the generation settles. Whether
drop-after-join fully substitutes for the explicit cancel (ACP-latch/cancel-token semantics) is a review
question for the dual-lens round, not adjudicated by this repair.

## 10. R2f1b 3c1 dual-lens repair: R1-R7 + C1-C4

The §9 subordinate-cancel disclosure is resolved. The one managed flight now takes its subordinate closure once,
runs it on every dispatch/refusal path, and releases it at settlement; only exact-ID container removal remains
identity-gated. The injected `Timeout` control proves one subordinate call, zero removals, and a dead `Weak` after
the non-`Complete` settlement.

Implementation choices and evidence:

- R1/C1 use the runtime-independent `{{.Id}}{{"\t"}}{{json .Config.Labels}}` template. The pure byte parser accepts
  a real-TAB Docker golden and rejects literal backslash-`t`; the ignored host Docker round-trip covers the full
  `a2a.*` label map.
- R2 classifies absence by a successful no-trunc inventory, never stderr text, for Docker and Podman fixtures.
  `AlreadyGone` completes without `rm`; inventory failure stays protective. `RefusedUnknown` is clearable only
  when `SpawnSettlementGuard` proves the failed spawn never published, and the retry control reaches spawn count 2.
- R3 makes subordinate ownership one-shot. The exact injected-probe-`Timeout`/zero-removal/`Weak` control and the
  pre-dispatch failure control prove unconditional execution and release.
- R4 maps only `Complete` to success in unit-returning destructive wrappers. `Retained`, `Preserved`, and `Unknown`
  become stable `DurableEvidenceUnavailable` refusals; checked/observed APIs retain their typed disposition.
- R5 threads backend disposition through node/workflow aggregation and terminal projection. Node vocabulary maps a
  protective value to `unknown_legacy`; workflow vocabulary retains `retained`/`preserved`/`unknown`. The pre-ID
  observed control pins `container.teardown.unknown`, while `container.teardown.reaped` is Complete-only.
- R6 captures runtime, immutable ID, and the full observed `a2a.*` label set at the `:ro` start boundary and operator
  discovery. Teardown revalidates before exact-ID `rm`; recycled-name, cross-runtime, inspect-failure, label-drift,
  and removal-failure controls all spare or report unresolved.
- R7 returns the durable settle winner, terminalizes pre-dispatch failure, leaves successful physical removal
  authoritative across an observation-journal failure, and records the shared monotonic clock duration. The
  recovery-winner control pins `Unknown` at driver, joiner, journal, and public projection.
- C2 requires canonical keys present-and-equal, tolerates other `a2a.*` keys, and records those extras in the existing
  removal observation through an optional defaulted field; empty-record JSON remains byte-identical. C3 adds the
  pre-release `is_finished` assertion and bounded entry wait. C4 keys the timeout fixture on argv `$5`.

Mandated inversions: `cold_generation_cannot_replace_unknown_cleanup_owner` became
`cold_failed_spawn_is_joinable_but_next_prompt_retries_once`; cold/warm pre-ID cancel and retire controls now require
typed Unknown refusal instead of success. No other existing test expectation was weakened, and no new adjacent
defect was discovered. Frozen `LIFECYCLE_SLOTS = 4`, `PROCESS_LIFECYCLE_SLOTS = 7`, and the
`process_lifecycle_reserved` golden have zero diff.

Local verification: `cargo fmt --all -- --check`, `git diff --check`, manifest metadata, frozen-slot grep, and the
no-name-removal source grep are green. Workspace check, Clippy, build/tests, and repository hygiene are not claimed:
online Cargo resolution was blocked by the configured crates.io CONNECT proxy (403 fetching `a2a-lf`), while the
offline cache lacks `arc-swap`; hygiene therefore could not build its binary. No daemon-backed Docker test, provider
turn, smoke, compatibility case, release, deployment, or running operator was invoked. The operator gate must rerun
the full configured workspace/release/Clippy/test/hygiene set plus the ignored Docker round-trip.
