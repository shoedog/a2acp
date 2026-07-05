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