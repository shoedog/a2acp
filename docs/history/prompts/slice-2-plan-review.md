You are reviewing an IMPLEMENTATION PLAN (not the design) for Slice 2 (Usage telemetry) of a2a-bridge,
grounded against the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. Judge whether a codex/sonnet implementor following this plan task-by-task produces correct,
compiling, spec-faithful code. Severity-tag BLOCKER/MAJOR/MINOR. Give concrete fixes (exact code/task edits),
not just gaps.

The DESIGN is dual-reviewed + frozen (spec v2, FIX-1..10 binding). Slice 2 plumbs the ACP `usage_update` —
today dropped at TWO gates — to a warm-handle usage snapshot + `session/status` + a pre-task threshold warn,
WITHOUT putting usage on the A2A wire. The plan is below.

{{input}}

READ FOR GROUND TRUTH (cite file:line):
- `docs/superpowers/specs/2026-06-18-slice-2-usage-telemetry.md` (v2 spec — esp. the "## v2 — dual spec-review
  fixes folded" section: FIX-1 TurnEvent pipeline, FIX-2 wire-leak filter, FIX-3 unary loop, FIX-4 degrade
  reframe, FIX-5 overThreshold computed, FIX-6 config plumbing, FIX-7 no idle-bump, FIX-8 by-value match +
  accessor, FIX-9 all WarmTurn sites, FIX-10 corpus shape).
- `crates/bridge-acp/src/acp_backend.rs` — `map_session_update` (1622-1629), `enum TurnEvent` (211-223), the
  notification handler that forwards only `Update::Text` (973-981), the prompt-stream `unfold` (1834-1848),
  the scripted-agent test harness `enum ScriptedUpdate` (2416-2423) + its `SessionUpdate` builder (2950-2964),
  the corpus-replay test (`tests/corpus_replay.rs`) + frames (`tests/corpus/codex-acp.jsonl`,
  `tests/corpus/claude-agent-acp.jsonl`); `crates/bridge-acp/Cargo.toml` (`unstable_session_usage` enabled);
  SDK `UsageUpdate`/`Cost` (`agent-client-protocol-schema-0.13.2/src/v1/client.rs:271-334`).
- `crates/bridge-core/src/ports.rs:24` (`Update::Usage`), `orch.rs:31-43` (`UsageSnapshot`/`UsageCost`),
  `translator.rs` (`EventKind` 24-29, `Event` struct+ctors 39-91, the `Ok(Update::Usage(_)) => continue;`
  drop 157), `executor.rs:143` (already ignores usage).
- `crates/bridge-a2a-inbound/src/sse.rs` (exhaustive `event_to_streamresponse` 46-108, `event_to_sse` 113-123
  + their tests), `crates/bridge-a2a-inbound/src/server.rs` (`spawn_local_producer` 1126-1200 incl. `tx.send`
  1177; the unary Local `.collect().await` 2318-2329; `local_kiro_source` 1376-1388; `WarmTurnGuard{sm,ctx}`
  452-465; `warm_local_dispatch` 553-579; `session_status` JSON 2842-2858; the SSE stream wrapper that maps
  events via `event_to_sse` ~2249), `session_manager.rs` (`WarmHandle` 31-48, `WarmTurn` 51-54,
  `SessionStatusInfo` 57-63, `checkout_turn` 95-251 incl. the THREE WarmTurn returns at ~124/~190/~229,
  `status` 262-276, the ~8 `SessionManager::new` call sites), `workflow_sink.rs:61` (`now_ms`).
- `bin/a2a-bridge/src/config.rs:44-50` (`ServerConfig`), `bin/a2a-bridge/src/main.rs:3657-3662` (serve wiring).

REVIEW DIMENSIONS (ground each in code, file:line):
1. **Spec faithfulness** — does each FIX-1..10 map to a task that IMPLEMENTS it (TurnEvent pipeline T2;
   wire-leak filter T5; unary loop T5; degrade corpus T1; overThreshold computed T6/T7; config T7; no
   idle-bump T4; by-value match + accessor T1/T3; all WarmTurn sites T7; corpus shape T1)? Any scope creep
   (reset/clear/compact/`OrchResult.usage`/journal/watchdog) or gap?
2. **Task ordering / dependency integrity** — does any task use a type/fn defined only later? Does each task
   COMPILE and its tests pass at its OWN end (T1 is claimed a behavior no-op — is that true given the handler
   still drops?)? Will `EventKind::Usage` / `TurnEvent::Usage` / the `Update::Usage` flow break ANY exhaustive
   `match` under `--all-targets` that no task fixes (enumerate the at-risk sites: sse.rs, both producers,
   fan-out synth, delegate, workflow projection, reattach)?
3. **T2 — the TurnEvent pipeline (highest risk, the un-drop).** Verify against the REAL code: are the three
   edits (TurnEvent::Usage carrier; handler routing the mapped `Update::Usage` to `TurnEvent::Usage`; `unfold`
   yielding `Ok(Update::Usage)` NON-terminal) correct + sufficient? Does the proposed prompt-stream test
   actually exercise the live route (not just the pure mapper)? Is the `ScriptedUpdate::Usage` harness
   extension wired to a real `SessionUpdate::UsageUpdate`? Any `#[cfg(feature)]` mismatch (variant vs arm vs
   test) that breaks a feature-off build?
4. **T5 — the wire-leak filter (highest risk for no-regression / DoD-5).** Is `event_to_sse -> Option<SseEvent>`
   + the `unreachable!` in `event_to_streamresponse` + the SSE-stream `filter_map` change complete and
   correct (all call sites updated: sse.rs tests, the server SSE wrapper)? Do BOTH producers (streaming
   `spawn_local_producer`, unary `.collect()→loop`) record usage iff warm AND exclude it from output? Is
   `WarmTurnGuard{sm,ctx}` actually reachable from the producer loop (privacy/borrow — is `warm` still live,
   not moved to `_warm`)? Does `record_usage(&w.ctx, snap.clone())` borrow-check inside the loop? Does
   `local_kiro_source` swallow Usage? Could usage still reach the wire via any path the plan misses (delegate,
   reattach replay, fan-out synth)?
5. **T3 — Event widening.** Are ALL `Event` constructors updated with `usage: None` (status/artifact/terminal
   — any others)? Is the `usage_snapshot()` accessor correct? Does adding `EventKind::Usage` force edits the
   plan omits?
6. **T4 — record_usage.** latest-wins + `now_ms()` stamp (is `crate::workflow_sink::now_ms` `pub(crate)`/
   reachable from session_manager.rs?) + NO `last_used` bump (FIX-7) + no-op on missing handle + default
   `usage` at mint — all correct and race-safe under the existing `by_context` lock?
7. **T6/T7 — status + threshold.** Is the `usage` JSON block additive (Slice-1 caps unaffected)? Is
   `window_fraction`/`over_threshold` the FIX-5 computed `Option<bool>` tri-state (None unconfigured/unknown;
   Some(false) below; Some(true) at/above)? Does `with_warn_fraction` avoid breaking the ~8 `SessionManager::
   new` call sites? Is `usage_warning` set on ALL THREE `WarmTurn` returns (fast-resume, post-reconcile,
   mint=None) per FIX-9? Config field + `main.rs` wiring + serve WARN log all named with correct edit sites?
8. **TDD realizability** — do the scripted-agent prompt-stream test (T2), the corpus assertion (T1), the
   translator/sse/producer tests lean on harness helpers that EXIST (name them) or must be built? Is the
   DoD-5 "SSE frame list unchanged" assertion actually writable against the current test surface?
9. **Live-gate provability** — DoD-1 (usage in status), DoD-2 (used rises), DoD-3 (pre-task warn + still
   runs), DoD-5 (no wire regression) provable on real codex via `submit --context`/`session status` + a
   serve-log grep? Is DoD-4 correctly unit/fixture-gated (ACP always carries used+size)?

OUTPUT: findings by severity (task #, file:line, fix); spec-faithfulness verdict; task-ordering verdict;
code-correctness verdict. End: `PLAN VERDICT: ready-to-execute | fix-then-execute | rework`.
