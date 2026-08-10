---
task-type: implement
---
# R2f1b 3b1 continuation — close five verified defects (D1–D4, D6) and revert one out-of-scope edit (D5)

## Description

Your prior 3b1 run landed `bec2e01b` on `feat/r2f1b-3b1-process-authority`
(+2,690/−251 — Supervised internal to `OwnedProcessTreeV1`, three signal paths
join-or-refuse, V2 legacy arms with controls, spawn-caller migration,
observed-and-journaled signal observations, diagnostic rider). The bounded
loop hit its 3-attempt cap with a REJECT whose findings the operator has now
VERIFIED AGAINST SOURCE — every one is real. This continuation closes exactly
those five items. Do not restructure anything else; the base design stands.

### D1 — BLOCKER: containment closure never verifies stopped state

`close_and_kill` (`crates/bridge-core/src/process.rs` ~`:1331`) declares the
census stable when two member sets around a SIGSTOP volley are equal, then
SIGKILLs child-first. SIGSTOP delivery is asynchronous: a member with the stop
still pending can be mid-`fork()`; the second census runs before its child
exists (sets equal → "stable"); the fork completes; the child is in no census
and survives the sweep. The window shrank; it did not close.

Fix constraint (your design details): after the SIGSTOP volley, VERIFY every
member actually REACHED stopped state via the platform status probe (darwin:
`proc_pidinfo` `pbi_status == SSTOP`; Linux: `/proc/<pid>/stat` state `T`)
before the census can be declared stable — an unstopped member means
not-stable, rescan. All-members-verified-stopped + census-stable is a true
closure (a stopped process cannot fork); only then the child-first SIGKILL
sweep runs. Termination stays bounded by `CONTAINMENT_CLOSURE_LIMIT` with the
`ContainmentUnstable` refusal. Extend the fake-OS port + ordering tests to
MODEL async SIGSTOP delivery: a member that keeps running through a volley
and forks after a matching census must be contained by the verified-stop
mechanism (deterministic red on the pre-fix logic).

### D2 — failed kills read as successful ACP teardown

`terminate_blocking`/`final_drop_join_or_refuse` return
`Ok(ResourceActionResultV1)` whose `disposition` is `Failed` when a signal
syscall failed (`settle_dispatch` maps `rc == -1 && errno != ESRCH` to
`Failed`), but all four consumers check only `.is_err()`:
`crates/bridge-acp/src/acp_backend.rs:2157`, `:2281`, `:2289`, `:7116`. A
failed kill therefore records a clean teardown. Fix: every consumer consults
the returned disposition; `Ok(Failed)` follows the same loud/protective path
as `Err` (per-site contract — cleanup proceeds protectively, the failure is
recorded/logged, never silently absorbed). One red per site family.

### D3 — owner attach/detach errors silently discarded

`attach_process_flight_owner` / `detach_process_flight_owner`
(`acp_backend.rs` ~`:4391-4411`) discard the `Result`s (`let _ =`) and a
poisoned `supervised` lock silently skips. An owner the flight never
registered breaks row 9 ("escalation lists every active owner"). Fix: attach
and detach outcomes are loud — log with the owner id and journal through the
flight's observation path where available; decide and DOCUMENT (design note)
whether an attach failure blocks the turn or proceeds-with-record. The
poisoned-lock arm must also record. Red test: a failing attach (injected via
the port/double) is observable, not silent.

### D4 — the production V3 constructor has no caller

`AcpBackend::spawn_with_durable_process_flight_v3` (`acp_backend.rs:3104`,
"Production V3 constructor") has ZERO call sites — a write-only adapter
entry, the exact class the 2b2 routing repair fixed. Fix: BIND it at the
production consumer that owns attempt scope (the attempt-owned registry +
durable file journal — the recorded ONE-REGISTRY-PER-ATTEMPT wiring
obligation), so the plumbing from attempt admission to the flight-owning
spawn exists and is exercised by a test proving the bound route constructs
ONE registry per attempt and hands the SAME flight to the spawn. Production
UNREACHABILITY may remain ONLY via the upstream V3 admission refusal
(`AutomaticR2f1b` both-entrance refusal — slice 4 arms it), never via an
uncalled constructor. If the true consumer belongs to a crate outside this
slice's files, bind at the nearest in-scope seam and DOCUMENT the remaining
hop in your handoff (design note), rather than reaching into out-of-scope
crates.

### D5 — revert the out-of-scope fs_custody tripwire weakening

`crates/bridge-core/src/fs_custody.rs`
`open_directory_no_follow_refuses_a_symlinked_directory`: the prior run
replaced the deliberate exact-per-platform errno assertion (whose comment
explicitly declared it a tripwire) with an `ENOTDIR | ELOOP` tolerance to get
the container verify green. Revert this hunk VERBATIM (restore the exact
per-platform constants and the original comment). The container-environment
accommodation is now handled in the verify config (the operator excluded this
test from hermetic runs), and your observation that in-container Linux
returns `ENOTDIR` is recorded in the operator's ledger for the fs_custody
owner — it is not yours to decide in this slice.

### D6 — the darwin start-identity probe does not compile on macOS

The operator's HOST gate (darwin) fails to build `bridge-core`:
`crates/bridge-core/src/process.rs:795-796` uses `libc::kinfo_proc`, which
does not exist in the workspace's PINNED libc on darwin (E0425), plus a
downstream E0282 at `:840` (`.and_then(|value| value.checked_add(micros))`
needs the closure parameter type once the struct resolves). The in-container
verify could never catch this — the `cfg(target_os = "macos")` lane is not
compiled on Linux. Fix constraint: the darwin probe must compile and run on
the PINNED dependency set — prefer a local `#[repr(C)]` definition of the
needed `sysctl` result prefix (through `kp_proc.p_starttime`) over a libc
version bump; if a bump is genuinely unavoidable it must keep `--locked`
verify green and be called out in your handoff. You are building in a Linux
container and cannot execute the darwin lane: get it COMPILING for
`target_os = "macos"` by inspection (the operator's host gate is the
executable evidence), and keep the Linux lane green.

### Rules

- Scope is EXACTLY D1–D5. No new surface, no trait changes, no edits outside
  the files named above plus their tests.
- The binding constraints from the base run still hold: one registry per
  attempt; no signal path off `ResourceFlightJournal::records()`; joins from
  non-async contexts via the typed refusal path; V2 paths byte-identical with
  their controls.
- Tests needing HOST process-tree/signal semantics carry the
  `_host_signal_semantics` suffix (hermetic verify excludes them; the host
  gate runs them).
- Feature-unification gotcha: feature-sensitive assertions verified under
  `--workspace` semantics.

## Acceptance Criteria

1. D1: the fake-OS containment tests include an async-SIGSTOP-delivery
   interleaving (member runs through a volley, forks after a matching
   census) that is red on the pre-fix logic and green with verified-stop
   closure; stopped-state verification uses the platform status probe on
   both platforms (Linux lane green or declared-excluded with darwin
   evidence).
2. D2: all four consumer sites treat `Ok(disposition: Failed)` as a failed
   teardown (loud + protective), with at least one red test per site family
   proving `Ok(Failed)` is not recorded as clean.
3. D3: a forced attach failure and a forced detach failure are observable
   (log/journal), never silent; the poisoned-lock arm records; design note
   documents the block-vs-proceed decision.
4. D4: a test proves the bound production route constructs exactly one
   registry per attempt and passes the same flight into
   `spawn_with_durable_process_flight_v3`; a workspace grep in your handoff
   shows the constructor has a production caller.
5. D5: the fs_custody hunk is byte-reverted; `git diff` for that file against
   `0a3c2434` is empty.
6. Workspace verify green in-container modulo the configured hermetic skips;
   fmt + clippy clean; exact totals in the handoff.
7. D6: `cfg(target_os = "macos")` code no longer references symbols absent
   from the pinned libc (no `libc::kinfo_proc`); the E0282 closure is typed;
   `Cargo.lock` unchanged unless disclosed. The operator's darwin host build
   is the executable gate for this item — your handoff states what you
   changed and why it compiles for the macos target.

## Files

`crates/bridge-core/src/process.rs`,
`crates/bridge-acp/src/acp_backend.rs`,
`crates/bridge-core/src/fs_custody.rs` (revert only),
`crates/bridge-core/src/retained_resource_flight.rs` (only if D4's binding
needs its seam), plus tests for the above.

## Spec Refs

- The base-run task spec (mirrored at
  `docs/superpowers/reviews/2026-08-10-r2f1b-3b1-task-spec.md` on the
  planning branch) — its mandate and constraints carry.
- `docs/superpowers/plans/2026-08-09-r2f1b-slice3-brief.md` §3 "3b1".
- `docs/superpowers/reviews/2026-08-10-r2f1b-3a-dual-review.md` "Ledger: 3b1
  (binding)".

## Commit Message

fix: 3b1 continuation — verified-stop containment closure, disposition-aware teardown, loud owner attach, V3 route binding
