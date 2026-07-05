I'll ground this review in the actual fixture code, then analyze the diff line-by-line.Confirmed against `src/lib.rs`: `sessions` is a `tokio::sync::Mutex<HashMap<..>>`, and `dispatch` now holds its guard across the agent round-trip.

---

**BLOCKER — `src/lib.rs:45-49` — map lock held across `run_turn().await` (WRONG).**
`let mut sessions = self.sessions.lock().await` (45) acquires the guard, and both that `MutexGuard` and the `&mut session` borrow into the map (46) stay alive across `self.agent.run_turn(id, prompt).await` (48). Because `tokio::sync::Mutex` keeps the guard held across the suspension point, every other concurrent `dispatch` — for *any* session id — blocks at line 45 for the full duration of the in-flight turn (the doc comment says "many seconds"). Concrete scenario: session A dispatches a slow turn; session B's dispatch cannot proceed until A's ACP round-trip completes. That is a global serialization of all sessions and a verbatim violation of the stated contract ("the lock… must NOT be held across the agent round-trip, so one slow turn cannot block every other session"). **Fix:** restore the two-phase structure the diff removed — capture what you need (or confirm existence) under the lock, drop the guard *before* `run_turn`, then re-lock briefly to bump `turns`.

**MAJOR — same site — re-entrancy / liveness (WRONG under a plausible future).**
The lock scope now spans foreign code (`self.agent.run_turn`). `tokio::sync::Mutex` is not reentrant, so the moment the agent needs to call back into `SessionManager` (open a sub-session, read/update session state, register a child turn) the task self-deadlocks. Even absent re-entrancy, `open()` (line 32) contends on the same lock, so no new session can be created while *any* turn is in flight. The refactor has silently given the map lock a second responsibility — serializing turn execution — beyond "protect the map."

**MAJOR — missing seam (absence check, SMELL).**
The design needs the map's lifetime and a turn's lifetime to be independent; the diff collapsed the two locks that provided exactly that separation. The right unit of protection is per-session (a `Mutex`/`Arc` handle cloned out from under the map lock), so map integrity and turn concurrency don't share a critical section. Without that seam, this class of regression will keep recurring whenever someone "simplifies" the locking.

**MINOR — `src/lib.rs:47` comment entrenches the bug (SMELL).**
"Run the turn while we hold the session so we can record it in place" documents the lock-across-await as intentional. A future reviewer will read it as a deliberate, safe choice, making the regression stickier and harder to catch.

**Design note (not a defect):** the old code had a benign gap — a session removed mid-turn made the post-turn `get_mut(id)?` return `None` and discard the reply. The new code closes that gap, which is likely what motivated the change, but it "pays" with the entire concurrency model. If that consistency actually matters, re-lock and handle the "session vanished during turn" case explicitly; don't hold the lock.

**Verdict: REJECT — the refactor holds the sessions map `Mutex` across the agent round-trip, serializing all sessions (and risking re-entrant deadlock), which directly violates the module's stated non-blocking contract.**