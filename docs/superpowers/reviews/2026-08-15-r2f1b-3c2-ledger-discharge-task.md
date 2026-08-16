---
task-type: implement
---

# R2f1b 3c2 ledger discharge — test-hardening regressions (test-only slice)

## Description

The 3c2 request-flight feature landed on `main` (PR #51, head `6ad88565`) with a
carried ledger of review-deferred test-hardening items. This slice discharges
every deferred test item in one test-only commit series. It adds regression
tests and test-only seams; it must not change any production behavior.

**Binding constraints:**

- **Test-only.** Production behavior must be byte-identical. Where a test needs
  an observation or fault seam that does not exist, add it `#[cfg(test)]`-gated
  (the established lane pattern: cfg(test) ordering/fault gates). No new public
  API, no changes to production control flow, no `rustfmt::skip`, no
  `Cargo.lock` churn beyond nothing.
- **Red-first evidence.** Every new regression must be shown to discriminate:
  either run it against the named historical head (the branch
  `feat/r2f1b-3c2-request-flight` preserves pre-repair commits), or name the
  single-line mechanism mutation (or cfg(test) fault-gate configuration) under
  which it fails, and record that evidence in the handoff. A test that cannot
  be made to fail proves nothing — do not ship it silently; report it.
- **Caps:** soft 600 / hard 800 changed lines, tests and cfg(test) seams only.
  If an item cannot fit, ship the others and report the one that does not fit
  with a reason — do not blow the cap.
- **Scope:** only the files named below plus their test modules. Do not
  refactor neighbouring tests. Match each file's existing test idiom exactly.
- Keep `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, and the workspace test suite green on the final head.

**The ten items** (source: the counted Sol closure records for tasks D, E, F,
F2, G and the aggregate re-look; prescriptions repeated here verbatim-in-spirit
so you do not need those documents):

**D-1 — simultaneous-wrapper barrier** (`crates/bridge-core/src/remote_request_flight.rs`;
mechanism: `provider_send_claimed` CAS, field at ~:252). The existing
duplicate-wrapper regression is sequential. Add a two-thread barrier test in
which two wrappers contend for the send permit concurrently, proving exactly
one inner poll happens and the loser produces zero row effect (no journal row,
no publisher access).

**D-2 — publication-waiter latch** (same file; the publication-race
regressions). Replace the one-second absence-of-completion observation with a
deterministic latch: a `#[cfg(test)]` hook that fires when a settler observes
`Driving`; release the publisher only after that latch. The two existing
publication-race tests should become deterministic (no wall-clock absence
window).

**E-1 — admission-reset state table** (`crates/bridge-api/src/backend.rs`;
multi-round admission reset). Add a direct state-table test proving
`(Complete, acknowledged=false)` re-admits, while `Partial`, `Failed`,
`Unknown`, `SettlementRefused`, and `TimedOut` each refuse re-admission.
Closure sizing: ~30–50 lines. Historical red: the pre-repair head `1f3c3a82`
fabricated `acknowledged=true`, so this table discriminates against it.

**E-2 — bound stale-cell recreation** (same file). Today stale-cell recreation
is tested through manually assembled internals. Add a public-path barrier test
that: times out an old V3 cleanup, recreates the same session, releases the old
publisher late, proves the successor cell remains live, and proves later
cleanup still aggregates the old `Unknown`. Closure sizing: ~60–100 lines.

**F-1 — reqwest poll barrier** (same file; first-poll arming fence,
`RequestAcceptanceMarker` at ~:813). Add a test-only poll barrier around the
actual `RequestBuilder::send()` future: assert disposition `Failed,
acknowledged=false` when cancellation lands before the barrier releases (send
never polled), and `Partial, true` after the first poll.

**F-2 — refusing and mismatched publishers** (same file). Two backend tests
through the public cleanup path: (a) a result publisher that refuses
publication, (b) a publisher that returns a non-identical delivery echo. Both
must produce prompt failure and checked cleanup `Unknown` — never a false
cleanup `Complete`.

**F2-Z — signal-semantics test made hermetic** (`crates/bridge-core/src/process.rs`,
the group-kill/descendant signal test, ~:3268 on the pre-rebase tree). Replace
the fixed 200 ms post-kill sleeps with bounded polling that treats a zombie
(`Z` state) descendant as terminated, retaining a live-descendant negative
control. Red-first shape prescribed by the closure: deliberately hold a killed
descendant as a zombie and show the old strict-absence assertion failing, and
verify a genuinely running descendant still fails the assertion. This is the
ledgered container-hermetic flake class fix — report the test's in-container
result in the handoff.

**G-1 — configure-clean eviction regression**
(`crates/bridge-workflow/src/executor.rs`; `PreflightFault::Configure`).
Existing tests exercise `Configure` only with `Unknown`/`Retained`/`Preserved`/
error cleanup results. Add the missing case: `Configure + Ok(Complete)` must
produce two configurations, one prompt, two forgets, one invalidation,
successful fallback, and cached reuse.

**A-1 — equal-length commitment regression**
(`crates/bridge-core/src/namespace_transaction.rs`; the staged-commitment
SHA-256 check; the existing `"commitment"` case replaces the stage with five
bytes so length rejects before the hash branch). Add the discriminating case:
rewrite exactly one byte of the staged checkpoint without changing inode or
length, then assert reopen refusal and exact root-byte preservation. Closure
sizing: ~10–15 lines.

**Known environment reds (ledgered, not yours):** the in-container whole-bin
`api_entry_resolves_and_serves_through_registry` red is a hermetic flock-EBADF
class, host-green 8/8 — disregard it if it appears. The signal-semantics
container flake is the thing F2-Z fixes.

## Acceptance Criteria

1. All ten items implemented as tests (plus minimal `#[cfg(test)]` seams where
   named), or explicitly reported as not-fitting with a reason. No production
   behavior change anywhere in the diff.
2. Each new regression's red-first evidence is recorded in the handoff: the
   historical head or the named mutation/fault-gate under which it fails, and
   the assertion message it fails with.
3. `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
   -- -D warnings` clean; workspace test suite green on the final head (modulo
   the ledgered hermetic container classes, which must be named if hit).
4. Diff stays within the caps (soft 600 / hard 800 changed lines) and touches
   only the five named files' test surfaces plus cfg(test) seams.
5. The handoff lists, per item, the test name(s) added and one sentence on what
   the test pins.
