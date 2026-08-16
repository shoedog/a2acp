---
task-type: implement
---

# R2f1b 3d — T2 extension repair 2: release the reservation when the control-root pin fails

## Description

Targeted repair on a FROZEN artifact. Base: `f66016e0` on branch
`salvage/r2f1b-3d-t2-extension-candidate` (the parked convergence-extension
candidate). Do **not** restart, redesign, or rework what is already there —
E1 (one phase CAS claimed by every pre-barrier terminal writer), E2's core
(the active flight is published BEFORE any per-flight blocking operation),
and E3 (lease-serialized exact-record terminal replacement) are all
**delivered and verified**. This repair closes exactly two findings and
nothing else.

## READ THIS FIRST — why the previous attempt's tests were red

The parked artifact's three red tests were **not** design failures. All three
are one mechanical omission: `unique_temp_dir(name)`
(`crates/bridge-worktree/src/backend.rs`) only *computes* a path — it never
creates the directory. Every other caller creates it (`provider_fixture` /
`backend_fixture` call `std::fs::create_dir_all`). The three new tests
construct `PreparationControlRootV1::new(tmp, …)` (or `std::fs::create_dir`)
against a path that does not exist:

- `failure_owned_runner_exit_completes_configure_result` →
  `open_claimed_for_session_admission()` returns `Err(StoreFailure)`
- `terminal_replacement_serializes_exact_open_writers` → same
- `preparation_control_root_refuses_identity_replacement` →
  `std::fs::create_dir(&root)` returns `NotFound` (non-recursive create, no parent)

**This was verified on the host: adding `std::fs::create_dir_all(&tmp).unwrap();`
to those three tests takes `cargo test -p bridge-worktree --lib` from
270 passed / 3 failed to 273 passed / 0 failed, with workspace
`cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`
both clean.** Apply exactly that; do not redesign the tests.

**Your container cannot compile.** The implement-lane egress allowlist
permits model APIs only — crates.io is deliberately excluded (ADR-0013), so
`cargo` will fail with an HTTP 403 on an uncached dependency download. This
is by design, not a fault to fix. Do not spend attempts on it, do not edit
proxy/egress/Cargo configuration, and do not weaken or delete a test because
you cannot run it. The bridge's verify stage compiles and runs the suite in
a dependency-capable container; that is your feedback channel.

## R1 — restore the artifact's own evidence (mechanical)

Add `std::fs::create_dir_all(&tmp).unwrap();` immediately after the
`unique_temp_dir(...)` call in the three tests named above (for
`preparation_control_root_refuses_identity_replacement` the `tmp` root must
exist before `std::fs::create_dir(&root)`). No other change to those tests.

## R2 — WRONG: a failing control-root pin permanently orphans the reservation

**Proven on the host at `f66016e0`**, driving the existing
`arm_nonreturning_control_root_pin` hook, removing the control root while the
pin is blocked, then releasing it:

```
owner published before the blocking pin = true      <- E2's core IS delivered
first  configure_bound_session(S, bound) = Err(StoreFailure)   <- correct
active entry retained after failure      = true                <- THE DEFECT
second configure_bound_session(S, bound) = Err(AgentOverloaded) <- permanent
```

**Mechanism.** `configure_bound_session`'s preflight inserts the
`ActivePreparationFlightV1` into `preparation_flights` before claiming the
root pin. When the runner's `task_root.pinned_root()` returns `Err`, the
`root_ready` arm calls `task_owner.complete_with_result(Err(error), Err(error))`
and then `runner_exit_guard.complete()` and returns. `complete_with_result`
does not touch the map, and `complete()` disarms the exit guard so
`terminalize_preparation_runner_exit` — the only path that removes the entry
— never runs. The session key is retained for the life of the process, and
the `flights.contains_key(&session_key)` admission check then refuses every
later `configure_bound_session` for that session with `AgentOverloaded`.

This is not confined to the flight whose pin failed: a failed
`open_claimed_for_session_admission` resets the root state to `Unpinned` and
`notify_all`s, so every flight parked in `pinned_root()` for that same pin
wakes into the identical `Err` arm and leaks its own reservation too.

**Required behavior.** On the `root_ready` error arm, the runner must release
its reservation, composed with E1's phase ownership:

1. Claim the terminal phase first via the flight's failure claim
   (`begin_failure_publication()`). If the claim **fails**, another writer
   (transfer) already owns the terminal — return exactly as the existing
   transfer-owned early return does: publish nothing, complete nothing,
   remove nothing.
2. If the claim succeeds: complete the caller with the typed error (current
   behavior, keep it), then remove the session's entry from
   `preparation_flights` **guarded by `Arc::ptr_eq`** against the owner this
   runner holds — the same guarded-removal idiom
   `terminalize_preparation_runner_exit` already uses. Never remove an entry
   that is no longer this owner.
3. Publish **no** durable record. The control root is precisely what failed
   to open, so no record can be written, and none should be: this path
   admits zero provider / session / process / `git worktree add` effects, so
   "no record, no effect, reservation released" is the honest terminal.
   State this explicitly in your handoff.
4. Keep the runner-exit guard disarmed on this path (the runner is
   terminalizing itself; it must not also fire the exit terminalizer).

**Red-first tests (all three required; each must fail on `f66016e0` and pass
after your change — record the exact pre-change failure in your handoff):**

- **T-A** — root pin fails: `configure_bound_session` returns the typed
  error, `preparation_guard_for_test(&session)` is `None` afterward, and a
  second `configure_bound_session` for the same session is admitted rather
  than refused `AgentOverloaded`. (Pre-change: entry retained, second call
  `AgentOverloaded`.)
- **T-B** — composition guard: a transfer claims the phase while the pin is
  blocked, then the pin fails. The transferred owner must still be in the
  recovery inventory, must not be removed from it, and the transfer's
  terminal must stand. (Pre-change: this must be shown not to regress.)
- **T-C** — two concurrent flights parked on one failing pin: both
  reservations are released and both callers get the typed error.

Drive the failure through the existing `arm_nonreturning_control_root_pin` /
`wait_for_control_root_pin` / `release_control_root_pin` hooks by making the
control root unopenable while the pin is blocked. Do not add a new
production code path to make the tests reachable.

## Out of scope — do NOT fix these

- **Per-flight blocking waits on the root pin** (each flight parks a
  `spawn_blocking` thread on the pin's condvar). Ruled a SMELL and
  **DEFERRED** with a ledger entry this round: it is a bounded resource
  concern with no demonstrated incorrect output, and consolidating the wait
  is an ownership redesign this repair will not carry.
- **s1 abort residue** (abort before first poll / during transfer
  publication) — already deferred until an aborting production consumer
  exists.
- The slice-4 binding observer obligation.
- Anything in T1's landed code, or any E1/E2/E3 mechanism not named above.

## Caps and constraints

- **Soft 150 changed lines, hard 250 changed lines** measured
  `git diff --numstat f66016e0..HEAD` (added + deleted). The previous repair
  breached its stated cap by 3 lines and that was ruled a contract blocker —
  respect this one, and if you approach the hard cap, say so in the handoff
  rather than silently exceeding it.
- Production changes confined to `crates/bridge-worktree/src/backend.rs`.
  If you believe another production file must change, stop and say why in
  the handoff instead of changing it.
- Zero production timers, zero spawned watchers, zero production arming of
  the preparation bound (slice 4 owns arming) — unchanged from T2.
- No new public API. No changes to the frozen custody transition table.
- `cargo fmt` clean and `cargo clippy --workspace --all-targets -- -D warnings`
  clean.

## Handoff must record

- The exact pre-change failure text for T-A, T-B, T-C.
- The `git diff --numstat f66016e0..HEAD` total against the caps.
- The "no durable record on root-pin failure" rationale from R2 step 3.
- The deferred SMELL (per-flight blocking wait) restated as a ledger item.
