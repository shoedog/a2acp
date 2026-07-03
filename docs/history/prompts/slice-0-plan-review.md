You are reviewing an IMPLEMENTATION PLAN (not the design) for Slice 0 of the a2a-bridge orchestration work,
grounded against the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. Judge whether a sonnet implementor following this plan task-by-task would produce correct,
compiling, spec-faithful code. Severity-tag every finding **BLOCKER / MAJOR / MINOR**. Be a co-architect —
give concrete fixes (exact code / exact task edits), not just gaps.

The plan is below. The DESIGN is already dual-reviewed + frozen — do NOT re-litigate the design; review the
PLAN's faithfulness, task ordering, code correctness, and executability.

{{input}}

READ FOR GROUND TRUTH (in the repo):
- `docs/superpowers/specs/2026-06-17-slice-0-live-session-core.md` (the spec the plan must implement — v2).
- The real code the plan's snippets target: `crates/bridge-core/src/{ids.rs,orch.rs(new),error.rs,ports.rs,
  domain.rs,session_fingerprint.rs(new),translator.rs}`; `crates/bridge-acp/src/acp_backend.rs`;
  `crates/bridge-container/src/lib.rs`; `crates/bridge-api/src/backend.rs`; `crates/bridge-a2a-inbound/src/
  {server.rs,session_manager.rs(new),lib.rs}`; `bin/a2a-bridge/src/main.rs`.

REVIEW DIMENSIONS (ground each finding in code with file:line):

1. **Spec faithfulness.** Does every Slice-0 spec IN item map to a task that actually implements it (not just
   names it)? Any spec requirement with no task, or a task that drifts beyond Slice 0 (creep) or under-builds
   (gap)? Check the tricky ones: release≠reap-shared-backend; ContainerRw per-session `release_warm`;
   `guard=None` warm dispatch; SEQ-AUTHORITY (reject contextId on non-Local + intra-manager); typed
   `ConfigMismatch`/`SessionExpired`; `Update::Usage` variant-now/plumbing-later.

2. **Task ordering / dependency integrity.** Does any task use a type/fn/field defined only in a LATER task?
   (e.g. does T2's `orch` depend on T1 ids — ok; does anything depend on `SessionManager` before T8; does T10
   reference `LocalDispatch.warm_session` it must itself add; does T4's `Update::Usage` break a match the plan
   doesn't fix in T4?) Verify each task COMPILES and its tests PASS at its own end (TDD integrity) — flag any
   task that leaves the tree non-compiling.

3. **Code correctness against real signatures.** Do the plan's snippets match the ACTUAL code shapes?
   Specifically verify: (a) `resolve_configure_bind`/`LocalDispatch` shape + the two Local arms
   (`unary_message` ~2194, `stream_message` ~622) + `spawn_local_producer` — does the plan's
   `warm_local_dispatch` + `warm_session` threading actually wire through to the `Translator::run(...,
   &session, ...)` call sites correctly? (b) the method `match` (server.rs ~589) + the auth/`jsonrpc_ok`/
   `bridge_err_to_jsonrpc`/`inbound_from` helpers the new handlers call — do those exist by those names? (c)
   `InboundServer` field/builder/`new()` edits. (d) ACP `release_session` (cancel + remove from
   `sessions`/`session_cfg` — lock types: `sessions`=tokio Mutex, `session_cfg`=StdMutex). (e) ContainerRw
   `release_warm` (the `retire_warm` per-entry recipe; `reap_once` signature). (f) `Update::Usage` — find
   EVERY non-wildcard `match` on `Update` (translator + acp_backend + anywhere) and confirm the plan's T4
   "add a no-op arm" covers all of them. (g) the `ids.rs` macro is String-only (SessionGeneration hand-written
   — correct?). (h) `EffectiveConfig` derives needed for the fingerprint.

4. **TDD test realizability.** Are the test snippets runnable as written, or do they lean on helpers that
   don't exist (`AcpBackend::new_for_test`, `warm_test_backend`, `seed_warm_entry`, the SessionManager
   `mgr()`/`advance_clock`, the `unary_message` mock harness)? For each, does an equivalent test double /
   constructor exist in the file to mirror, or must the implementor build one (flag the effort)? Is the
   `unimplemented!()` scaffold in T8 acceptable (flagged) or a trap?

5. **Integration completeness.** The warm path must dispatch the prompt against the warm `backend_session`
   (NOT `routed.session`) in BOTH the unary and streaming arms, and must NOT create a `BindingGuard`. Does
   the plan fully thread this (T10), or is there a hole where the legacy `session-{task}` still leaks in? Does
   the streaming arm's `spawn_local_producer(&srv, routed, dispatch, tx)` carry the warm session? Does
   `store.put(task, session)` (the SessionStore mapping) need handling on the warm path?

6. **Live-gate executability.** Are the Task-14 DoD commands actually runnable against `serve` as written
   (does `submit` reach the Local route with `--agent` and no skill; does serve read `warm_idle_ttl_secs`;
   is the `pgrep`/`docker ps` observation valid given release≠process-reap)? Flag any DoD step that won't
   demonstrate what it claims.

OUTPUT: findings by severity (BLOCKER/MAJOR/MINOR) each with the task #, file:line, and a concrete fix
(exact code or exact task-step edit); a spec-faithfulness verdict; a task-ordering verdict; a
code-correctness verdict. End with one line: `PLAN VERDICT: ready-to-execute | fix-then-execute | rework`.
