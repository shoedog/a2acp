You are doing a WHOLE-BRANCH code review (read-only) of the Slice-7b "E9 watchdog" implementation — the ENTIRE
`main...HEAD` diff on `feat/slice-7b-watchdog`, as ONE coherent change. This is the cross-task safety net:
per-task tests passed, but cross-task / concurrency / race / lifecycle bugs spanning commits only surface here. Be
rigorous, adversarial, decisive. READ-ONLY: read files, grep, `git diff main...HEAD`; do NOT edit/build/test.

Binding spec `docs/superpowers/specs/2026-06-20-slice-7b-watchdog.md` (FIX-1..12) + plan `…/plans/2026-06-20-slice-7b-
watchdog.md` (PFIX-A..M). The input below is context; GROUND every finding in real code with `file:line`.

{{input}}

REVIEW THE WHOLE BRANCH (`git diff main...HEAD`) for:
1. **The driver-arm RACE-FREEDOM (the load-bearing correctness property).** Trace the driver's outer `tokio::select!`
   (`acp_backend.rs:~1971`): with the new `watchdog_fired` arm, is a `prompt_fut`-completion (`Ok(resp)`) and a
   watchdog fire MUTUALLY EXCLUSIVE → can a NATURAL completion EVER be relabeled `AgentTimedOut`? Is `timed_out
   _local` a plain driver-local set ONLY in the watchdog arm (no shared atomic leaking into the terminal)? Does the
   watchdog arm DISCARD its inner cancel outcome (always `Err(())`, even if the agent honors cancel → `Done{cancelled}`
   within grace)? Any `&mut prompt_fut` borrow conflict between the new arm + the existing arms?
2. **The watchdog task LIFECYCLE (no leak / no fire-after-end).** Is the task `'static` (captures only Arc/Notify/
   Duration/oneshot — no `&self`/`cx`)? Is it torn down on EVERY driver exit (Ok/Err/kill/consumer-drop) via
   `drop(done_tx)` at the all-exit cleanup (`:2002`)? Could it leak if the turn errors before the task spawns, or
   FIRE `watchdog_fired` AFTER the turn already completed + `map.remove`d (a late notify on a freed turn — is it
   harmless)? Does the `sleep_until` + re-derive loop TERMINATE? Thousands of concurrent per-turn tasks — cost?
3. **The disabled path (`watchdog=None`) byte-identity.** With no config: is NO watchdog task spawned, the select
   arm a `Pending` future (never fires), the handler bump a no-op (`watch=None`)? Are the existing acp turn/corpus/
   usage tests byte-identical? Any allocation/spawn on the disabled path?
4. **The activity tap.** Is the handler bump NON-BLOCKING (short `StdMutex`, no `.await` under the lock)? UNCONDITIONAL
   (before the `map_session_update` drop → unmodeled events count)? Does the `RequestPermissionRequest` handler
   actually bump (it had to capture the registry)? Could the bump RACE the driver `map.remove` (a bump after the
   turn ended — harmless?)?
5. **The timeout SEMANTICS + math.** idle-only-after-first-event (la==0 → wall-clock only)? `saturating` math (the
   handler advances `la` mid-check)? the `la_instant = turn_start + (la-1)ms` round-trip? `tokio::time::Instant::
   from_std` for `sleep_until` (not a std/tokio Instant mismatch)? Does the FN-1 long tool_call (steady updates)
   reset idle → never trip?
6. **The outcome.** `AgentTimedOut` → A2A `Failed` (disposition `_` default, NOT `Canceled`) + `Fatal` in
   `classify_death` (NOT retried)? Does the terminal emit `AgentTimedOut` (not the generic `AgentCrashed`) ONLY when
   `timed_out_local`? Does the executor/translator then mark the node `Failed`?
7. **Cross-cutting:** the config ripple (every `AgentEntry`/`ContainerRwConfig` literal; the 2 `main.rs` forwards;
   the `>0` validation; the container composition); the `escalate_terminate` blast-radius on a SHARED backend (the
   watchdog now auto-fires it — concurrent escalation with the external `cancel()` grace-watcher is take-once-safe?);
   any `unwrap`/panic on the watchdog path; the warm/dispatcher path UNTOUCHED.

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` anchor + a CONCRETE
fix. Focus on cross-task / race / lifecycle bugs a single-file review misses. End with one line:
`BRANCH VERDICT: approve | approve-with-nits | changes-required`. Do NOT edit files.
