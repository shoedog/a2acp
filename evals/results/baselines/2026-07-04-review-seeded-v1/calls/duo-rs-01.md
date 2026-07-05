# Merged Review: rs-01 — `SessionManager::dispatch`

Both lenses (correctness + architecture) succeeded and agree on the core defect. Findings below are de-duplicated and prioritized.

---

**BLOCKER (WRONG) — `src/lib.rs:45-49`: the whole-map mutex is now held across the agent round-trip, serializing every session.**
The refactor takes `let mut sessions = self.sessions.lock().await` once, and the guard (plus the `&mut session` borrowed from it) stays live across `self.agent.run_turn(...).await` on line 48. This directly violates the stated contract ("the map lock … must NOT be held across the agent round-trip"). Concrete failure: `run_turn` is documented to take "many seconds"; while session A's turn runs, every concurrent `dispatch(...)` **and** every `open(...)` blocks on `self.sessions.lock().await`, even though sessions are independent. Throughput collapses from concurrent to fully serial — the entire point of warm sessions is defeated. Both reviewers flag this identically; it is unambiguous. **Fix:** restore the drop-before-turn ordering — lock the map only long enough to confirm/obtain the session, drop the guard, run the turn, then re-lock briefly to bump `turns`. Preferred structural fix: store values as `Arc<Mutex<Session>>` (or `Arc<Session>` + interior counter), lock the map only to clone the handle, drop it, then lock/await on the per-session handle (avoids the double lookup the author was trying to eliminate).

**MAJOR (SMELL) — design gap: no per-session synchronization seam, so the map lock is doing double duty (map integrity *and* per-session turn recording).**
There is no per-session lock, so the only unit available to guard "record the turn against the right session" is the coarse map lock — which is exactly why the refactor reached for holding it across the turn. Until an `Arc<Mutex<Session>>` in the map or a per-id lock table exists, any correct "record in place" implementation is forced to choose between the old code's TOCTOU and this code's serialization. This is the structural root cause; fix it alongside the BLOCKER or the same regression recurs next time someone touches session state around a turn.

**MAJOR (SMELL) — latent deadlock / re-entrancy hazard.**
`tokio::sync::Mutex` is not reentrant and is now held across an arbitrary `await`. If `run_turn` ever gains a callback into the same `SessionManager` (a tool that opens/looks up a sibling session, or any reachable `self.sessions.lock()`), it self-deadlocks — no panic, the task just hangs forever. The drop-before-turn ordering is immune to this; the new shape makes the whole agent call graph a lock-ordering hazard.

**MAJOR (SMELL) — no concurrency regression coverage for the lock scope.**
The fixture has no test that would fail when `run_turn` is awaited while the global lock is held, so this exact contract regression can land unnoticed. Add a test with two opened sessions and a controllably delayed agent turn, asserting the second session can enter/run while the first is still pending.

**MINOR (SMELL) — `src/lib.rs:47`: the comment and commit subject document the defect as intent.**
The line-47 comment ("Run the turn while we hold the session so we can record it in place") and the commit subject ("Simplify … record the turn in place") frame a concurrency-semantics change as a cosmetic simplification; a reviewer skimming would not know throughput characteristics changed. Rewrite the comment to state *why the lock must be dropped before the turn*, tying it to the invariant rather than blessing the violation.

**MINOR (SMELL, pre-existing) — no failure channel on the turn.**
`run_turn` returns `String` and `session.turns += 1` runs unconditionally, so an errored turn (once errors are possible) still increments the counter. Not introduced by this diff, but this refactor is the moment the "record the turn" seam is being cemented — give it an error-aware shape (`Result`) before the counter can mean anything.

---

**Verdict: REJECT / do not merge — one BLOCKER (map lock held across `run_turn` serializes all sessions and violates the explicit no-lock-across-round-trip contract). Restore drop-before-turn ordering, ideally via a per-session `Arc<Mutex<Session>>` seam, and add a concurrency regression test.**