I'll examine the fixture to confirm the full context of the change.The full file confirms the diff exactly. Here is my review.

---

## Review: rs-07 — DeltaBuffer storage simplification

**BLOCKER — `src/lib.rs:22-25` (`push`): the bounded-ring invariant is deleted, making the buffer unbounded.**
The contract states the buffer "MUST be bounded (a ring of at most `CAP = 1024` deltas)" and — critically — that "the bound is what keeps memory finite regardless of whether anyone drains." The diff removes `CAP` and the `if pending.len() >= CAP { pending.pop_front(); }` eviction, leaving `push` to append to a `Vec` with no cap. The commit message calls the eviction "more machinery than needed," but that eviction *was* the mechanism enforcing the invariant — it is the load-bearing code, not incidental machinery.

- **What breaks & when:** A *detached run with no subscriber* — the exact path the contract calls out. Since `take` is never called there, nothing ever drains the `Vec`, and `push` fires "on every token/delta from the agent." Memory grows linearly and without limit for the life of the turn → unbounded heap growth / OOM under long or high-throughput streaming. Even on the subscribed path, memory now spikes to the full un-evicted backlog between drains instead of being capped at 1024. This is **wrong** (a demonstrable defect against a stated invariant), not a smell.
- **Direction of fix:** Restore the bound. Either revert to `VecDeque` + `pop_front` eviction at `CAP`, or if a `Vec` is truly desired, enforce the cap in `push` (e.g. drop-oldest via `remove(0)` — O(n), worse than the deque — or a fixed-size ring index). The `VecDeque` version was the correct tool precisely because O(1) front-eviction is what a bounded ring needs; the "simplification" trades the right data structure for the wrong one and loses the invariant in the process.

**MINOR — structural observation (not introduced by this diff, but relevant to the fix): the bound is an unnamed, uncommented magic property of `push` alone.** With `CAP` gone, nothing in the type documents or tests the bound, so future edits can silently reintroduce this same regression. When restoring, keep `CAP` a named `const` referenced at the single eviction site and consider a unit test asserting `len() <= CAP` after `CAP + N` pushes, so the invariant is guarded structurally rather than living only in prose.

**Verdict:** BLOCKER — the change removes the sole bound on an unbounded-input buffer, reintroducing the exact detached-run memory-leak the contract exists to prevent; do not merge without restoring bounded eviction.