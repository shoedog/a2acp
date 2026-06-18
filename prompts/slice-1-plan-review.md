You are reviewing an IMPLEMENTATION PLAN (not the design) for Slice 1 of a2a-bridge, grounded against the
ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT edit/build/test. Judge
whether a codex/sonnet implementor following this plan task-by-task produces correct, compiling, spec-faithful
code. Severity-tag BLOCKER/MAJOR/MINOR. Concrete fixes (exact code/task edits), not just gaps.

The DESIGN is dual-reviewed + frozen (spec v2). Slice 1 upgrades warm-continue from "reject" to "reconcile
model/effort when advertised; cwd→reject; mode→reseed" + records agent caps. The plan is below.

{{input}}

READ FOR GROUND TRUTH:
- `docs/superpowers/specs/2026-06-17-slice-1-config-reconcile.md` (v2 spec, esp. the "v2 fixes folded" section).
- The shipped Slice-0 code this modifies: `crates/bridge-a2a-inbound/src/session_manager.rs` (`checkout_turn`
  90-115, `WarmHandle`, `SessionStatusInfo`, the `by_context` tokio::Mutex), `crates/bridge-core/src/
  {orch.rs,error.rs,session_fingerprint.rs,ports.rs,domain.rs}`.
- The ACP lift target: `crates/bridge-acp/src/acp_backend.rs` — `AgentSession` (266-310), the mint closure
  (1184-1290; `opts0`/`models0`/`refreshed_opts`), `configure_model_option` (524-584), `apply_effort_walkdown`
  (622-710), `set_config_option` (480-495, returns refreshed opts), `set_model` (605-620), `agent_capabilities`
  (1058-1068), `ensure_session` signature; `crates/bridge-acp/src/model_effort.rs` helpers; the SDK
  `AgentCapabilities`/`SessionCapabilities`/`NewSessionResponse` shapes; `crates/bridge-acp/Cargo.toml` features.
- `crates/bridge-a2a-inbound/src/server.rs` (`session_status` ~2842).

REVIEW DIMENSIONS (ground each in code, file:line):
1. **Spec faithfulness** — does each v2-spec fix map to a task that IMPLEMENTS it (full-mismatch routing;
   surface cache; AgentSessionCaps rename/trim/delete=false; helper returns ReconcileOutcome; concurrency;
   live agent_session_id; fieldless Rejected; disposition wiring; status JSON; RPC-fired proof)? Any creep/gap?
2. **Task ordering / dependency integrity** — does any task use a type/fn defined only later? Does each task
   COMPILE + tests pass at its own end? Will the `ReconcileOutcome`/`ConfigReseedRequired` additions break
   any `match` under `--all-targets` that a task doesn't fix?
3. **T4 — the LIFT (highest risk).** Is extracting `apply_model_effort` from the mint closure (1225-1288)
   sound + mint-parity-preserving? Verify against the REAL closure: can model+effort be factored into a helper
   that takes a cached `ConfigSurface{opts,models}` AND be called at mint with the freshly-minted surface
   WITHOUT changing the hard-fail-at-mint semantics (config_invalid/agent_crashed) or the effort walk-down
   fallback? Is caching `opts0`/`models0`/`refreshed_opts` on `AgentSession` (a `StdMutex<Option<ConfigSurface>>`)
   correct? Does `set_config_option` returning refreshed opts keep the cache fresh? Any borrow/lifetime/`'static`
   issue moving this out of the `get_or_try_init` closure? Does the `NotAdvertised`-vs-`Rejected` mapping
   actually derive from `configure_model_option`'s `Err(config_invalid)` vs `Err(agent_crashed)`?
4. **T5 — reconcile_config.** Does `ensure_session(session)` correctly return the live `AgentSessionId` + NOT
   re-mint a warm session? Is NOT calling `configure_session` (to avoid the `minted_cwd` guard) right? Does it
   reach `entry.config_surface` correctly?
5. **T6 — concurrency (highest risk).** Is the drop-lock-across-await dance correct: claim `state=Running`
   BEFORE `drop(tab)`, call `reconcile_config`, re-acquire, re-check the handle still exists, advance
   `fingerprint` only on `Applied`, reset `Idle` otherwise? Any TOCTOU/deadlock/double-borrow? Does claiming
   `Running` correctly block a concurrent `checkout` (HandleBusy)? Does the diff-routing (frozen→reject,
   mode→reseed, ⊆{model,effort}→reconcile) handle the empty-diff and multi-field cases right? Does dropping
   `tab` (a `MutexGuard`) then re-binding compile/borrow-check?
6. **T7/status + caps recording** — `WarmHandle.caps` field + `SessionStatusInfo.capabilities` + the manual
   `json!` field: all edit sites named? `AgentSessionCaps` derives needed (Clone/Serialize)?
7. **TDD realizability** — do the fake-backend tests (configurable ReconcileOutcome/caps) + recording-transport
   acp tests lean on helpers that exist or must be built? Is the mint-parity regression test specified?
8. **Live-gate provability** — DoD-1 (reconcile applies + the `set_config_option` RPC fired) provable on real
   codex via `submit --context --effort`? DoD-2/3 typed errors? DoD-4 correctly unit-test-gated?

OUTPUT: findings by severity (task #, file:line, fix); spec-faithfulness verdict; task-ordering verdict;
code-correctness verdict. End: `PLAN VERDICT: ready-to-execute | fix-then-execute | rework`.
