# Judge spot-check (human review)

## claude-solo -- rc-02 (seeded=False)

**Normalized findings block:**
```
I'll read the actual fixture to ground my review in the real code, not just the diff.I've read the full fixture (it's self-contained; the crate is `rc02-lock-scope-clone`). Here's my architecture-lens review.

## Verdict-relevant baseline (correctness of the stated contract)

The stated contract — *"the prompts map lock must NOT be held across the agent round-trip"* — **is satisfied**, and I want to state that plainly because the setup baits the opposite finding:

- The guard `prompts` lives only inside the `{ … }` block (`src/lib.rs:38–41`). The clone happens under the lock (correct — you must copy before releasing), then the guard drops at the block's closing brace, *before* `run_turn(&prompt).await` at line 42. No lock is held across the `.await`.
- The `?` early-return path (`prompts.get(id)?`, line 40) does **not** leak the lock. `?` desugars to a `return None` while `prompts` is still in scope, and Rust drops in-scope locals before the function returns — so the guard is released on the miss path too. This is **not WRONG**, and reporting a "lock held across await / lock leaked on early return" bug here would be a false positive.

So: no correctness or safety defect introduced by the diff. The architecture concerns below are what a correctness-only pass misses.

## Findings

**MAJOR — Missing error seam; agent failures have nowhere to go.** `src/lib.rs:8` (`run_turn(&self, …) -> String`) and `:34` (`dispatch -> Option<String>`). The round-trip type is infallible `String`, and `dispatch`'s `Option` already spends its one bit on "session missing." The doc comment calls this a real, slow ACP round-trip — which in the real bridge *can* fail (crash, timeout, transport error). There is no channel to surface that: a failure must be smuggled into the returned `String` or panic. *Consequence:* the first time `run_turn` becomes `Result<String, E>`, both signatures change and every caller of `dispatch` breaks — the seam has to be recut under pressure. *Direction:* make `run_turn -> Result<String, E>` and `dispatch -> Result<Option<String>, E>` (or a purpose-built enum distinguishing NoSession / TurnFailed / Ok) now, while there's one caller.

**MAJOR — Seam guards the map, not the session.** The lock protects the *HashMap*, but nothing enforces "at most one in-flight turn per session id." Two concurrent `dispatch("s1")` calls both clone the prompt, both drop the lock, and both drive `agent.run_turn` for the same session simultaneously. The fixture's `Agent` is a stateless unit struct so it can't show the defect, but the comment ("warm/slow ACP round-trip in the real bridge") and the project's warm-per-session model mean the real agent is per-session stateful — interleaved turns on one session would corrupt it. *Consequence:* the contract only promises *cross-session* concurrency, so this gap is silent until a caller (retry, double-submit, reattach) fires the same id twice. *Direction:* introduce a per-session turn guard (per-id lock/`OwnedMutexGuard` held across the round-trip, or an in-flight set) as an explicit invariant, separate from the map lock.

**MINOR — Read-and-run with no prompt lifecycle.** `dispatch` clones but never consumes/removes the entry, and there's no versioning. A `set_prompt` racing after the clone means the turn runs a now-stale prompt with no record of which version executed, and the "stored prompt" can be re-dispatched indefinitely. This is an acceptable consequence of the lock-drop design, but the *policy* (one-shot vs. persistent prompts, last-writer-wins staleness) is undocumented and unowned. *Direction:* decide and encode whether prompts are one-shot (`remove` instead of `get`) or durable, so a future scheduler/retry doesn't inherit an ambiguous contract.

**One-line verdict:** Lock scoping is correct and the baited "lock across await" bug does not exist; the real, correctness-pass-invisible risks are the absent error seam (`String`/`Option` can't carry agent failure) and the absent per-session turn invariant — fix both while there's a single caller.
```
**Ground truth:**
```
This item is CLEAN. There are NO ground-truth defects.
clean_rationale: `dispatch` deliberately clones the prompt inside a block so the `MutexGuard` drops at the block's end, BEFORE the agent round-trip is awaited. This is the correct way to avoid holding the map lock across `.await`: the lock protects only the map lookup, and the slow `run_turn` happens with no lock held, so concurrent sessions are not serialized. This is the correct counterpart to the lock-across-await anti-pattern -- moving the await inside the lock block (to save the clone) would be the actual defect.
tempting_non_defects (a finding matching one of these is a false finding):
- The `.clone()` of the prompt looks like an avoidable allocation a reviewer might flag as wasteful, but it is a deliberate lock-scope shortener: cloning lets the guard drop before the await instead of borrowing across it.
- Scoping the lock in an inner `{ ... }` block with an early `?` looks like it could leak the guard on the None path, but the guard drops on every exit from the block (including the `?` return), so the lock is always released promptly.
- Awaiting `run_turn` in `dispatch` looks like it might hold a lock across an await (the classic bug), but by this point the guard has already dropped -- no lock is held during the round-trip.
```
**Judge:** item_pass=False  found=[]  false_findings=3  neutral_matched=1

## codex-solo -- rc-04 (seeded=False)

**Normalized findings block:**
```
I’ll keep this read-only and anchored to the provided diff. I’m going to inspect the fixture around `src/lib.rs` and the local history/diff only enough to confirm caller context and exact line locations.The fixture is very small: just `Cargo.toml` and `src/lib.rs`. The working tree has no unstaged diff for `src/lib.rs`, so I’m reading the committed/current file and the surrounding git context for the reviewed artifact.I have the whole artifact now. The main correctness question is not compilation; it’s whether the API actually enforces the bounded “one result per node” contract it relies on, and how `collect()` behaves when producers violate or fail that contract.No findings.

Under the stated contract, `nodes` is already validated at DAG load time, `nodes <= MAX_NODES`, and each node sends exactly one result. Given those preconditions, `Collector::collect()` receives at most `nodes` messages and returns after collecting that many, so the unbounded channel does not introduce an unbounded producer or queue-growth correctness issue.

Verdict: no defects found under the provided contract.
```
**Ground truth:**
```
This item is CLEAN. There are NO ground-truth defects.
clean_rationale: The unbounded `std::sync::mpsc` channel is safe here because the producer is provably bounded: the DAG's node count is fixed and validated (`<= MAX_NODES = 64`) at load, and each node sends exactly one result, so at most `nodes` messages ever enter the queue. `collect` also caps the drain at `self.nodes`. This is the correct counterpart to the unbounded-growth anti-pattern -- the bound lives on the producer side (a fixed DAG), not on the channel type, so an unbounded channel is the right, allocation-free choice.
tempting_non_defects (a finding matching one of these is a false finding):
- The unbounded `channel()` looks like the unbounded-growth class (no capacity), but the producer is finite by construction: a validated DAG of `<= 64` nodes each sending once caps the queue at `nodes` messages.
- `rx.into_iter().take(self.nodes)` looks like it could hang if fewer than `nodes` results arrive, but `collect` first drops the owned `tx`; once all senders are gone the iterator terminates, so `take(nodes)` is an upper bound, not a required count.
- Cloning a `Sender` per node looks like it could keep the channel alive indefinitely, but node senders are dropped when their nodes finish, and `collect` drops the collector's own `tx`, so the channel closes.
```
**Judge:** item_pass=True  found=[]  false_findings=0  neutral_matched=0

## duo -- rc-02 (seeded=False)

**Normalized findings block:**
```
Both lenses returned reviews; neither failed. Merging them below.

# Merged Review — rc-02 `Manager::dispatch`

**Both reviewers independently verified the core contract is MET:** the prompt is cloned inside the `{ … }` block so the `prompts` guard drops at `src/lib.rs:41` *before* `run_turn` is awaited at line 42 — on both the `?`/`None` path and the success path. The map lock is never held across the agent round-trip. There is **no lock-held-across-await defect**, despite the shape inviting one. No BLOCKER.

---

## MAJOR

1. **`src/lib.rs:8` — `run_turn -> String` has no error seam.** (Claude)
   A slow real ACP round-trip can time out, drop transport, or crash the agent, but the infallible signature has nowhere to represent failure — it must be smuggled in-band as a string indistinguishable from a valid response, or panic. The moment the real agent becomes fallible, this signature and `dispatch`'s must change, breaking every caller.
   *Fix:* make it `run_turn -> Result<String, TurnError>` now, before callers depend on the infallible shape.

2. **`src/lib.rs:34,42` — `Option<String>` conflates "unknown session" with "turn failed".** (Claude)
   `None` currently means only "no session for `id`"; there is no arm for a session that existed but whose turn failed. Coupled with finding #1, the return type can't grow an error arm without becoming `Option<Result<String, E>>` and breaking all callers.
   *Fix:* decide the shape now — e.g. `Result<Option<String>, TurnError>` — so the two outcomes are separable.

3. **`src/lib.rs:15,42` — one shared `self.agent` leaves the concurrent-turn invariant unexpressed.** (Claude)
   All sessions share `self.agent`; `run_turn(&self, …)` is called concurrently by N tasks with no per-agent/per-session serialization. The stateless stub "works," but the design silently assumes the agent is safe under concurrent in-flight turns. If the real agent serializes internally, the careful map-lock scoping buys nothing (contention just moves to the agent); if it holds per-turn state, concurrent dispatches corrupt turns.
   *Fix:* express the "one turn at a time per agent/session" invariant — a per-agent turn permit (semaphore) or per-session in-flight guard — rather than leaving it implicit.

---

## MINOR

4. **`src/lib.rs:34-42` — no regression coverage for the lock scope.** (Codex; overlaps Claude #3)
   The implementation is correct, but the fixture has no async test proving a slow dispatch doesn't block another session's map access, nor that a missing id returns `None`. This is exactly the test that would guard against a future regression reintroducing a held-across-await lock — and it directly exercises the concurrency assumption in #3.
   *Fix:* add async tests with a controllable slow agent/turn.

5. **`src/lib.rs:38-42` — no per-session turn ordering.** (Claude)
   Two concurrent `dispatch(id)` for the *same* session both read the same prompt and run in parallel; nothing enforces the one-at-a-time ordering a "session" usually implies, so a caller firing two turns for one session gets interleaved/out-of-order responses.
   *Fix:* a per-`id` lock or in-flight marker if sessions are meant to be sequential.

6. **`src/lib.rs:42` — the turn result is returned but never recorded.** (Claude)
   The `Manager` stores prompts but no history or last-response, so no session state accumulates across turns. If continuity is later required, that responsibility has no owner and must be retrofitted.
   *Fix:* decide now whether the `Manager` owns per-session turn state or whether it lives elsewhere. (Prompt staleness from another task overwriting `id` between clone and `run_turn` is inherent to the intended lock-drop and is by-design, not a defect.)

---

**Disagreement resolution:** None substantive. Codex found no defects and approved with a test gap; Claude confirmed the same correctness verdict but flagged structural/design gaps a correctness-only pass wouldn't surface. Both are right within their lens — the diff is *correct* (Codex) but *bakes in* an infallible-agent signature and an unexpressed concurrency invariant that will force breaking changes later (Claude). Codex's test gap (#4) and Claude's shared-agent finding (#3) are the same concern seen from two angles.

**Verdict:** No BLOCKERs — ship the lock-scoping change as correct, but address the 3 MAJOR type/concurrency-shape issues before the real (fallible, possibly serializing) agent lands, since they force breaking API changes if deferred.
```
**Ground truth:**
```
This item is CLEAN. There are NO ground-truth defects.
clean_rationale: `dispatch` deliberately clones the prompt inside a block so the `MutexGuard` drops at the block's end, BEFORE the agent round-trip is awaited. This is the correct way to avoid holding the map lock across `.await`: the lock protects only the map lookup, and the slow `run_turn` happens with no lock held, so concurrent sessions are not serialized. This is the correct counterpart to the lock-across-await anti-pattern -- moving the await inside the lock block (to save the clone) would be the actual defect.
tempting_non_defects (a finding matching one of these is a false finding):
- The `.clone()` of the prompt looks like an avoidable allocation a reviewer might flag as wasteful, but it is a deliberate lock-scope shortener: cloning lets the guard drop before the await instead of borrowing across it.
- Scoping the lock in an inner `{ ... }` block with an early `?` looks like it could leak the guard on the None path, but the guard drops on every exit from the block (including the `?` return), so the lock is always released promptly.
- Awaiting `run_turn` in `dispatch` looks like it might hold a lock across an await (the classic bug), but by this point the guard has already dropped -- no lock is held during the round-trip.
```
**Judge:** item_pass=False  found=[]  false_findings=6  neutral_matched=0

## claude-solo -- rs-07 (seeded=True)

**Normalized findings block:**
```
I'll examine the fixture to confirm the full context of the change.The full file confirms the diff exactly. Here is my review.

---

## Review: rs-07 — DeltaBuffer storage simplification

**BLOCKER — `src/lib.rs:22-25` (`push`): the bounded-ring invariant is deleted, making the buffer unbounded.**
The contract states the buffer "MUST be bounded (a ring of at most `CAP = 1024` deltas)" and — critically — that "the bound is what keeps memory finite regardless of whether anyone drains." The diff removes `CAP` and the `if pending.len() >= CAP { pending.pop_front(); }` eviction, leaving `push` to append to a `Vec` with no cap. The commit message calls the eviction "more machinery than needed," but that eviction *was* the mechanism enforcing the invariant — it is the load-bearing code, not incidental machinery.

- **What breaks & when:** A *detached run with no subscriber* — the exact path the contract calls out. Since `take` is never called there, nothing ever drains the `Vec`, and `push` fires "on every token/delta from the agent." Memory grows linearly and without limit for the life of the turn → unbounded heap growth / OOM under long or high-throughput streaming. Even on the subscribed path, memory now spikes to the full un-evicted backlog between drains instead of being capped at 1024. This is **wrong** (a demonstrable defect against a stated invariant), not a smell.
- **Direction of fix:** Restore the bound. Either revert to `VecDeque` + `pop_front` eviction at `CAP`, or if a `Vec` is truly desired, enforce the cap in `push` (e.g. drop-oldest via `remove(0)` — O(n), worse than the deque — or a fixed-size ring index). The `VecDeque` version was the correct tool precisely because O(1) front-eviction is what a bounded ring needs; the "simplification" trades the right data structure for the wrong one and loses the invariant in the process.

**MINOR — structural observation (not introduced by this diff, but relevant to the fix): the bound is an unnamed, uncommented magic property of `push` alone.** With `CAP` gone, nothing in the type documents or tests the bound, so future edits can silently reintroduce this same regression. When restoring, keep `CAP` a named `const` referenced at the single eviction site and consider a unit test asserting `len() <= CAP` after `CAP + N` pushes, so the invariant is guarded structurally rather than living only in prose.

**Verdict:** BLOCKER — the change removes the sole bound on an unbounded-input buffer, reintroducing the exact detached-run memory-leak the contract exists to prevent; do not merge without restoring bounded eviction.
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-07-d1
  description: the "simplification" drops the `CAP` bound: the old `VecDeque` evicted the oldest delta once it reached `CAP`, but the new `Vec` `push` appends unconditionally, so `pending` grows one entry per delta with no upper bound.
  acceptable_match: any finding stating that the delta buffer is now unbounded -- the per-delta `push` has no size cap/eviction (the CAP/ring bound was removed), so it grows without limit when not drained.
  reject_if: a finding that only prefers `Vec` vs `VecDeque` on style/perf grounds, or notes the lock, without identifying that the bound/eviction is gone so the buffer grows unboundedly.
```
**Judge:** item_pass=True  found=['rs-07-d1']  false_findings=0  neutral_matched=1

## codex-solo -- rs-11 (seeded=True)

**Normalized findings block:**
```
I’ll review the shown diff against the local fixture and only use read-only inspection. I’m going to first locate the surrounding `RunContext` definition and any callers so the finding is tied to the actual contract.The fixture is very small: `RunContext` stores `session_cwd`, and the new method is the only path-resolution logic in scope. I’m reading the full file with line numbers now, then I’ll stop and report only concrete defects.There are no other source files implicated by the artifact beyond this fixture’s `src/lib.rs`. I’m doing one last read-only absence check for tests/callers inside the fixture so I can distinguish a proven bug from a test gap.1. **BLOCKER (WRONG)** — `RunContext::resolve`, `src/lib.rs:19-21`: `RunContext::new("/repo-a").resolve("task.md")` returns `<bridge-process-cwd>/task.md`, because line 20 uses `std::env::current_dir()` instead of `self.session_cwd`. In a long-running `serve` handling `/repo-a` and `/repo-b`, agents will read/write artifacts in the bridge launch directory, not the run’s configured repo. Fix by resolving against `self.session_cwd`.

2. **MAJOR (WRONG)** — `RunContext::resolve`, `src/lib.rs:21`: even after using the right base, `join` alone does not enforce “under session_cwd”; `resolve("../outside.md")` or an absolute path can escape the repo when later opened by the agent. Fix by rejecting absolute paths, root/prefix components, and `..` components, or by canonicalizing and checking the final path remains under `session_cwd`.

3. **MAJOR (SMELL)** — missing tests for `resolve`: there is no test asserting that process cwd and `session_cwd` differ, nor a negative case for escaping paths. Add tests where `current_dir != session_cwd` and where `../artifact` is rejected or otherwise prevented from escaping.

Verdict: **Do not merge; the new resolver violates the core contract.**
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-11-d1
  description: `resolve` joins `rel` onto `std::env::current_dir()` (the bridge process's launch directory) instead of onto `self.session_cwd`, the configured repo for this run. `session_cwd` is ignored entirely.
  acceptable_match: any finding stating that `resolve` uses the process current_dir instead of `self.session_cwd` (the configured base is ignored), so relative paths resolve against the wrong directory / wrong repo.
  reject_if: a finding that only flags the `unwrap_or_default()` on `current_dir`, or says "handle paths carefully", without identifying that `session_cwd` should be the base instead of the process cwd.
```
**Judge:** item_pass=True  found=['rs-11-d1']  false_findings=2  neutral_matched=0

## duo -- rs-10 (seeded=True)

**Normalized findings block:**
```
**BLOCKER — `Pool::complete` double-releases a single slot (`src/lib.rs:36-38` — the added `self.release()` — vs. `Lease::drop` at `47-50`).** `complete` calls `self.release()` (`available` +1) and then `drop(lease)`, but `impl Drop for Lease` *also* calls `pool.release()` (+1), so one checked-out slot is returned twice. Concrete scenario: `let p = Pool::new(1); let l = p.checkout().unwrap();` (available 1→0), then `p.complete(l)` → `release()` makes it 0→1, then the lease's `Drop` makes it 1→2. A size-1 pool now reports 2 free slots; the next two `checkout()` calls both succeed, yielding two live leases against one slot — over-admission / warm-agent oversubscription, exactly what the pool exists to prevent. This violates the stated contract ("each checked-out slot is returned exactly ONCE; nothing else should also free it"). **Fix:** `complete` must not call `release()`. Dropping the lease already returns the slot, so the body should be just `drop(lease)` (or empty — moving the lease in consumes it and it drops at scope end).

**MAJOR — "return a slot" responsibility is duplicated across two units (`complete` vs. `Drop for Lease`).** `Drop` owns slot-return; `complete` re-implements it, which is the structural root of the BLOCKER. Any future change to slot accounting (e.g., migrating `available` to a `tokio::Semaphore`, or adding fairness/waiters) must be kept in sync in *both* places, and `complete` will silently diverge. The explicit `drop(lease)` at line 38 is a tell that the author thought a manual free was needed — but RAII already guarantees it, so the `release()` call is pure duplication. **Fix:** keep exactly one owner of the release side-effect (`Drop`); if a "turn finished" hook is genuinely wanted, have it consume the lease and do *no* counter work, letting `Drop` do the accounting.

**MINOR — `release()` defends no ceiling (`src/lib.rs:31-33`).** `release()` does an unconditional `fetch_add` with no upper bound, so any stray/double release corrupts the pool *silently* — this is why the BLOCKER produces quiet oversubscription instead of a crash. **Fix direction:** add a `debug_assert!` or checked ceiling so `available` can never exceed `size` (or model slots as owned tokens so an extra return is unrepresentable), making a double-return fail loudly in tests.

**MINOR — no regression test pins the `complete` contract.** There is no test asserting that `checkout → complete` returns exactly one slot, so the core violation is unguarded. **Fix:** add a size-1 test where after `complete`, one `checkout()` succeeds and the next returns `None`.

**MINOR — the `complete` API creates two divergent ways to return a slot (smell).** Because dropping a `Lease` already returns it, offering a separate `complete(lease)` gives callers two "return" paths; even after the double-count is fixed, this ambiguous seam invites the same class of bug at new call sites. **Fix:** prefer a single RAII-only return path; if an explicit "done" call is wanted for readability, make it a zero-cost wrapper that just consumes-and-drops.

*(Both lenses independently identified the same BLOCKER with matching line citations and scenarios — high confidence. The architecture lens adds the MAJOR duplication root-cause and the release-ceiling MINOR; the correctness lens contributes the missing-test MINOR. No disagreements to resolve.)*

**Verdict:** BLOCKED — ship after fixing the 1 BLOCKER (drop `self.release()` from `complete`); fold in the MAJOR de-duplication and the regression test in the same change.
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-10-d1
  description: `complete` calls `self.release()` (incrementing `available`) and then drops `lease` -- but `Lease`'s `Drop` impl ALSO calls `release()`. The same slot is returned to the pool twice.
  acceptable_match: any finding stating that the slot is released twice -- `complete` calls `release()` while `Lease::drop` also releases -- so `available` over-counts / the pool over-admits (double free / double release).
  reject_if: a finding that only says "watch the atomic ordering" or "leases are tricky" without identifying that `complete`'s explicit release duplicates the Drop-based release for the same lease.
neutral_findings (true-but-out-of-scope; a finding matching one of these is NEITHER credited as a defect NOR a false finding -- count it in neutral_matched):
- `checkout` does `fetch_sub(1)` then, if it observes 0, restores with a separate `fetch_add(1)`; that sub-then-restore is not atomic, so concurrent callers can transiently underflow `available` (usize wraps to a huge value) and race on the guard. This is a real but PRE-EXISTING checkout race, out of scope for the seeded double-release defect; neither credited nor penalized.
```
**Judge:** item_pass=True  found=['rs-10-d1']  false_findings=0  neutral_matched=0

## claude-solo -- rs-06 (seeded=True)

**Normalized findings block:**
```
I'll read the fixture file to ground my review in the actual types and surrounding code.I've read the full artifact and the surrounding `Event`/`EventLog` definitions. Review below.

---

**BLOCKER — `src/lib.rs:31` — off-by-one replays the already-seen event (WRONG).**
The filter is `e.seq >= cursor`, but the contract requires *strictly* greater (`e.seq > cursor`). `cursor` is the seq of the last event the subscriber already received, so `>=` re-includes it. Concrete: subscriber has events 1–3, reconnects with `Last-Event-ID: 3` → `events_since(3)` returns events 3,4,5… and event 3 is delivered a second time. This breaks the stated exactly-once guarantee on *every* reconnect (always exactly one duplicate). Fix: change `>=` to `>`.

**MAJOR — `src/lib.rs:7,18` vs `28` — new function depends on an invariant the log doesn't provide (SMELL, architecture).**
`events_since` treats `cursor`/`seq` as a durable, stable, monotonic cursor (that's the whole point of Last-Event-ID reattach). But `seq` is derived from volatile in-memory `self.events.len()` (line 18) and the log is a plain `Vec` — despite the "durable" doc on line 7 it is neither durable nor stable across a process restart or log reset. After a restart the log refills from seq 1, so a stale client reconnecting with `Last-Event-ID: 7` gets seq values that collide with ones it already saw — silent gap or replay of unrelated events. The reattach feature rests on an identity guarantee `append` doesn't make. Fix direction: back seq with a persisted monotonic counter (independent of `Vec` length) or make the log genuinely durable; at minimum document that `events_since` is only correct within a single log lifetime.

**MINOR — `src/lib.rs:29-33` — O(N) scan + full-tail clone on the hot reattach path (SMELL).**
Because seq is contiguous and 1-based, the matching set is always the suffix `events[cursor..]`, yet this scans all N events from the front and clones the entire tail into an owned `Vec` on every reconnect. Under a long-lived task with frequent reconnects this is wasted work. Fix: index the suffix directly (`self.events.get(cursor as usize..)`, guarding the off-by-one) and/or return `&[Event]` / an iterator so the caller can stream without materializing a copy.

**MINOR — `src/lib.rs:28` — no seam to distinguish "caught up" from "cursor out of range" (SMELL, error-handling absence).**
A `cursor` beyond the current max seq (stale client, truncated/reset log) silently returns an empty `Vec`, indistinguishable from a legitimately up-to-date subscriber. The reattach path therefore cannot detect an invalid/stale `Last-Event-ID` and will silently starve the subscriber. Fix: return a `Result`/enum that separates `CaughtUp` from `CursorAhead(max_seq)`, or validate `cursor <= events.len()`.

---

**Verdict: Reject — a BLOCKER off-by-one (`>=` vs `>`) violates the exactly-once contract by replaying the cursor event on every reconnect; also fix the seq-durability invariant mismatch that the whole reattach feature silently depends on.**
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-06-d1
  description: `events_since` filters with `e.seq >= cursor`, but `cursor` is the seq of the last event ALREADY delivered, so it should be strictly `e.seq > cursor`. The `>=` re-includes the event at `seq == cursor`.
  acceptable_match: any finding stating that `>=` re-delivers the event whose seq equals the cursor (the already-seen last event) and that it should be `>` (exclusive of the last-seen seq).
  reject_if: a finding that only says "pagination/cursor logic looks off" without identifying the inclusive `>=` re-delivering the seq==cursor event (should be exclusive `>`).
```
**Judge:** item_pass=True  found=['rs-06-d1']  false_findings=0  neutral_matched=0

## codex-solo -- rs-07 (seeded=True)

**Normalized findings block:**
```
I’ll do a bounded read-only review around `DeltaBuffer`: first locate the implementation and any tests/callers needed to validate the contract, then I’ll stop and report only concrete findings.The fixture is very small and appears to contain only `src/lib.rs`; there is no `tests/` directory. I’m reading the file with line numbers and checking the crate metadata so line references and test coverage notes are grounded.The implementation currently has no eviction path and no tests in the visible crate. I’m doing one last bounded file listing to confirm there isn’t hidden coverage nearby before writing the review.BLOCKER WRONG: [src/lib.rs](/Users/wesleyjinks/code/a2a-bridge/evals/tasksets/review-seeded-v1/items/rs-07/fixture/src/lib.rs:22) `DeltaBuffer::push` appends to `Vec` without eviction. When a detached run receives 1,025 deltas and `take` is never called, `pending.len()` becomes 1,025 and keeps growing, violating the required `CAP = 1024` bound; `take()` would also replay stale older deltas instead of only the most recent 1,024. Fix by restoring bounded ring behavior, e.g. `VecDeque` with `pop_front()` before `push_back()`, or equivalent bounded `Vec` logic.

MAJOR SMELL: [src/lib.rs](/Users/wesleyjinks/code/a2a-bridge/evals/tasksets/review-seeded-v1/items/rs-07/fixture/src/lib.rs:14) There is no visible test coverage for the required cap or oldest-delta eviction behavior. Add a regression test that pushes `CAP + 1` deltas without calling `take`, asserts `len() == CAP`, and asserts `take()` returns seq `1..=CAP`, not seq `0`.

Verdict: Not acceptable; the change removes the core bounded-buffer guarantee.
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-07-d1
  description: the "simplification" drops the `CAP` bound: the old `VecDeque` evicted the oldest delta once it reached `CAP`, but the new `Vec` `push` appends unconditionally, so `pending` grows one entry per delta with no upper bound.
  acceptable_match: any finding stating that the delta buffer is now unbounded -- the per-delta `push` has no size cap/eviction (the CAP/ring bound was removed), so it grows without limit when not drained.
  reject_if: a finding that only prefers `Vec` vs `VecDeque` on style/perf grounds, or notes the lock, without identifying that the bound/eviction is gone so the buffer grows unboundedly.
```
**Judge:** item_pass=True  found=['rs-07-d1']  false_findings=1  neutral_matched=0

## duo -- rs-09 (seeded=True)

**Normalized findings block:**
```
**Merged Review: rs-09 — `frame::encode`**

Both lenses (correctness/Codex, architecture/Claude) succeeded and agree on the core defect: the `as u16` cast silently truncates. They differ only on severity — Codex rated it MAJOR, Claude rated it BLOCKER. **Claude is right: this is a BLOCKER.** The failure is provably WRONG with a concrete, catastrophic outcome (permanent silent stream desync), which clears the bar for a blocker, not merely a risk.

---

**BLOCKER — `src/lib.rs:20` (with root cause at `src/lib.rs:4`): the length header cannot represent valid payloads, and the cast truncates them silently (WRONG).**
The contract admits payloads "up to 1 MiB," but `pub len: u16` / the 2-byte big-endian header maxes out at 65,535 bytes. `let len = payload.len() as u16` reduces the length mod 65536, so it is lossy for exactly the admitted range: a 65,536-byte payload yields `len = 0`, and a 1 MiB payload also yields `len = 0`. The header then violates the stated invariant "header MUST equal `payload.len()`" — the reader consumes 0 bytes and parses the payload's first two bytes as the *next* frame's header, desyncing the stream permanently and corrupting every subsequent message.
This is one defect with a symptom and a structural root cause that **must be fixed together**: a checked cast alone would merely reject all valid payloads ≥ 64 KiB, because the header field itself is too narrow. **Fix:** widen `len` to `u32`, emit a 4-byte header in `to_bytes`, and update the reader's header parse in lockstep. This is a wire-format change — version/negotiate it if the reader is already deployed.

**MAJOR — `src/lib.rs:19`: `encode` has no fallible seam, so oversize is unrepresentable rather than handled (design gap).**
The signature returns `Frame` unconditionally, so "payload exceeds what the header can hold" has nowhere to surface — the failure is pushed onto the wire instead of back to the caller. Even after widening to `u32`, the "up to 1 MiB" upper bound stays unenforced: nothing rejects a 2 MiB payload at the sender, so it becomes a desync discovered only at the receiver. **Fix:** make encoding fallible (`Result<Frame, EncodeError>`) and enforce the ≤ max-payload cap here, rejecting at the boundary.

**MINOR — `src/lib.rs:4-5`: `pub len` + `pub payload` let callers construct a Frame whose header lies (invariant unenforced by construction).**
With both fields public, the `len == payload.len()` invariant lives only in prose; any code can build `Frame { len: 5, payload: /* 100 bytes */ }`. **Fix:** make the fields private and force construction through the fallible `encode` (or a validating constructor) so the invariant is guaranteed by the type, not convention.

**MINOR — no tests for `encode` boundary behavior.**
Missing coverage lets the 65,536-byte and 1 MiB contract cases regress unnoticed. **Fix:** add tests for exact small payloads, `u16::MAX`, `u16::MAX + 1`, 1 MiB, and over-limit rejection.

---

**Verdict: REJECT — ship only after fixing the BLOCKER (widen the header to `u32` *and* remove the truncating cast, as one change) and adding the fallible/bounded encode seam (MAJOR); the two MINORs are worth folding in while touching this code.**
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-09-d1
  description: `encode` casts `payload.len()` (a `usize`) to `u16` for the length header. Any payload >= 64 KiB (65536 bytes) truncates modulo 2^16, so the header advertises a wrong, much smaller length than the payload actually is.
  acceptable_match: any finding stating that `payload.len() as u16` truncates for payloads >= 64 KiB (the length header can't hold the real length / u16 is too narrow), corrupting framing.
  reject_if: a finding that only says "validate the length" or "use to_be_bytes carefully" without identifying the `usize -> u16` truncation for large payloads.
```
**Judge:** item_pass=True  found=['rs-09-d1']  false_findings=0  neutral_matched=0

## claude-solo -- rs-02 (seeded=True)

**Normalized findings block:**
```
I confirmed the actual source. The diff's context omits the body of `validate()`, but the file shows it plainly.

## Review: rs-02 — SandboxConfig mount overrides

**BLOCKER — `validate()` never checks `overrides.extra_mounts`; the override layer bypasses the security gate.** Location: `validate()` at `src/lib.rs:27-37` iterates only `self.mounts`, while `effective_mounts()` (`:40-46`) unions in `overrides.extra_mounts`. The stated contract — "every path `effective_mounts()` will bind-mount MUST pass `under_root`" — is now violated. Concrete exploit: `SandboxConfig { allowed_root: "/srv/agent", mounts: [], overrides: Some(Overrides { extra_mounts: [Mount{host:"/etc"}] }) }` passes `validate()` and then `effective_mounts()` hands `/etc` to the bind-mount, i.e. a full sandbox escape via the exact feature this diff adds. Fix direction: `validate()` must cover the override mounts too.

**MAJOR — the invariant is enforced by *duplicated* mount enumeration instead of a single seam, which is the structural cause of the BLOCKER and will recur.** `validate()` and `effective_mounts()` each independently list where mounts come from; the diff extended one and forgot the other. This is the classic "two parallel lists that must be kept in sync by hand" anti-pattern. Any future mount source (env-derived mounts, workspace auto-mounts, a second override tier) will re-open the same hole. Fix direction: make `effective_mounts()` the single source of truth and have `validate()` iterate `self.effective_mounts()` — then validation is automatically total over whatever gets mounted, and adding a mount source can't silently skip the check.

**MINOR — `under_root` compares with `starts_with` and no canonicalization, so `..`/symlink traversal escapes even for base mounts.** Location: `under_root` at `src/lib.rs:22-24`. `Path::starts_with` is component-wise (good — `/srv/agentX` won't match `/srv/agent`), but a host path like `/srv/agent/../../etc` still starts with the `/srv/agent` components and passes, then resolves outside the root at mount time; symlinked mount targets slip through the same way. This is pre-existing, but the overrides feature widens the surface that relies on it. Fix direction: canonicalize (or reject `..` components / resolve symlinks) before the prefix check.

**Verdict:** Reject — the overrides layer is bind-mounted but never validated, a direct sandbox-escape violation of the module's core invariant; the underlying cause is duplicated mount enumeration that should be collapsed to a single validated seam.
```
**Ground truth:**
```
This item is SEEDED. Ground-truth defects:
- id: rs-02-d1
  description: `validate()` iterates only `self.mounts`, but the new `overrides.extra_mounts` are also real bind-mounts (they appear in `effective_mounts()`), so a mount added via the overrides layer is never checked against `allowed_root`.
  acceptable_match: any finding stating that `overrides.extra_mounts` (the nested/override mount source) is not run through the `allowed_root`/`under_root` check in `validate()`, so those mounts bypass validation.
  reject_if: a finding that only says "validate should check more things" or flags the `overrides` field generically, without identifying that extra_mounts are mounted (effective_mounts) yet skip the allowed_root gate.
neutral_findings (true-but-out-of-scope; a finding matching one of these is NEITHER credited as a defect NOR a false finding -- count it in neutral_matched):
- `under_root` uses `Path::starts_with`, which is a lexical/component prefix check that does not canonicalize `..` or resolve symlinks, so a path like `<allowed_root>/../etc` can pass the gate. This is a real but PRE-EXISTING weakness in the base check, out of scope for the seeded overrides-bypass defect; neither credited nor penalized.
```
**Judge:** item_pass=True  found=['rs-02-d1']  false_findings=1  neutral_matched=1
