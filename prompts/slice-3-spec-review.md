You are reviewing a DESIGN SPEC for Slice 3 (Clear / reset) of the a2a-bridge orchestration work, grounded
against the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. Judge **intent, not verbatim**. Severity-tag findings **BLOCKER/MAJOR/MINOR**. Be a
co-architect — concrete fixes with file:line.

Slices 0–2 are SHIPPED on `main` (warm `SessionManager` keyed by contextId; `release_session`; config
reconcile with the `Reconciling`/`Expiring` claim discipline; usage telemetry with `record_usage` +
`WarmTurn`). Slice 3 adds **`clear`** = reset a warm session's CONTEXT to empty while keeping the PROCESS warm,
via a NEW bridge `SessionId` per generation (DIVERGENCE-1) + a GENERATION-MONOTONICITY stale-write guard. The
spec is below.

{{input}}

READ FOR GROUND TRUTH:
- `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (**Slice 3 row** = authoritative scope/DoD/deps:
  `reset_session` new-SessionId-per-generation + GENERATION-MONOTONICITY + `clear`, require Idle unless
  `force_cancel`; OUT = compact S4).
- `docs/superpowers/specs/2026-06-17-orchestration-architecture.md` — **OPEN-4** (clear/compact reset
  primitive), **DIVERGENCE-1** (new bridge `SessionId` per generation + release old, OnceCell-safe; the
  PASS-3 UNANIMOUS ruling), the **GENERATION-MONOTONICITY** invariant, and "compact = composition".
- `crates/bridge-a2a-inbound/src/session_manager.rs` — the SHIPPED `checkout_turn` (the mint branch
  `~:271-309` building `ctx-{ctx}-g0` + `generation: 0`; the `Reconciling`/`Expiring` claim-across-await
  discipline `~:154-268`), `WarmHandle` (`:31`, has `generation`/`backend_session`/`usage`), `WarmTurn` (`:52`),
  `finish_turn` (`:313`, ctx-keyed), `record_usage` (`:343`, ctx-keyed), `release` (`~:360`), `status` (`:321`),
  `SessionState` (`:18`).
- `crates/bridge-core/src/ports.rs` — `AgentBackend::release_session` (`:55`, drops per-session state) +
  `configure_session` (`:42`, stashes spec for the lazy mint); `crates/bridge-core/src/ids.rs`
  `SessionGeneration(u64)` (`:41`).
- `crates/bridge-acp/src/acp_backend.rs` — `AgentSession.agent_id`/`minted_cwd` OnceCells (`:275/283`, the
  non-resettable identity DIVERGENCE-1 hinges on); `ensure_session`'s `get_or_try_init` `session/new` lazy
  mint; the `AcpBackend::release_session` override (removes `session_cfg`+`sessions[id]`).
- `crates/bridge-container/src/lib.rs` — `release_session` (`:553`) / `release_warm` (`:424`) (per-session
  container reap).
- `crates/bridge-a2a-inbound/src/server.rs` — `WarmTurnGuard{sm,ctx}` (`:452`, Drop→`finish_turn`), the warm
  producers (`spawn_local_producer` ~`:1126`, unary `~:2318`) that hold the turn, the `session_status`/
  `release`/`cancel` JSON-RPC handlers (`~:2826`); `bin/a2a-bridge/src/main.rs` `session` CLI subcommand.

REVIEW (ground each in code with file:line):
1. **Faithfulness to the slicing-spec Slice-3 row + DIVERGENCE-1.** Does it implement clear as
   new-SessionId-per-generation + release-old (NOT an in-place reset)? Is the OnceCell-non-resettable
   reasoning correct against the real `AgentSession`? Scope creep (compact, journal, MCP, force beyond
   cancel-then-reset) or gap (a Slice-3 requirement missing)?
2. **The reset mechanism (D1).** Is SessionManager composition (`release_session(old)` + `configure_session(new)`
   + generation bump + lazy mint) actually correct + sufficient against the real code — does the RESUME path
   after clear dispatch against the new id and lazily mint a fresh `session/new`? Does it really need
   `configure_session(new)` (the resume path skips configure today)? Is reconstructing `SessionSpec` from the
   `fingerprint` (incl. `cwd: Option<String> → SessionCwd::parse`) sound? Is the two-call (release→configure)
   window genuinely safe under the `Resetting` claim, or is a dedicated atomic backend `reset_session` method
   warranted (the architecture's OPEN-4 proposed one)?
3. **The `Resetting` claim discipline.** Does reusing the Slice-1 `Reconciling`/`Expiring` pattern (claim →
   drop lock → async release+configure → re-acquire → revalidate exact claim → commit or EXPIRE) correctly
   block concurrent `checkout` (`HandleBusy`) and defer concurrent `cancel`/`release` (`expire_after_reconcile`)?
   Any TOCTOU/ABA/deadlock? Is EXPIRE-on-non-clean right?
4. **GENERATION-MONOTONICITY (the load-bearing guard, D3).** Is threading the captured `generation` through
   `WarmTurn`→`WarmTurnGuard`→`finish_turn(ctx,gen)`/`record_usage(ctx,gen,..)` (no-op on stale gen) correct +
   complete? Verify the real producer sites (`spawn_local_producer`, unary) thread it. Could a mis-thread drop
   a LIVE turn's completion (hang/leak) or fail to drop a stale one (corruption)? Does it actually satisfy the
   DoD "a stale old-generation in-flight turn does NOT advance the live handle's seq"? Is `usage` reset-to-zero
   on clear safe against a late stale `record_usage`?
5. **`force` scope (D2).** Is cancel-then-reset (cancel the Running turn, then reset) the right Slice-3 line,
   and is the ordering (cancel → confirm torn down → Resetting → release) race-safe? Or should `force` defer to
   S4? If deferred, is the generation guard still load-bearing (is DoD-4 reachable)?
6. **Surfaces.** `session/clear {contextId,force?}` JSON-RPC + CLI `session clear` — coherent with the shipped
   status/release/cancel? Is `clear` the right public verb (vs reset)? Return shape `{cleared,generation}` ok?
7. **No-regression + DoD live-gate provability.** Does clear keep the process/lease/handle warm (DoD-2: pgrep
   shows no respawn; ContainerRw `docker ps` old→0/new→1)? Is DoD-1 (recall=none after clear) provable on real
   codex? Is DoD-4's race correctly unit-gated (the live force path being end-to-end)? Slice 0/1/2 no-regression?
8. **Ambiguities** that would trip a codex/sonnet implementor: the exact `Resetting` transitions; the
   generation-thread edit sites; the `SessionSpec`-from-fingerprint reconstruction; the `force` cancel ordering;
   the usage-reset placement.

Also adjudicate the spec's **D1–D4 open decisions** explicitly.

OUTPUT: findings by severity (file:line + fix); scope verdict (in-bounds/creep/gap); no-regression verdict;
live-gate-provability verdict; D1–D4 rulings. End: `SPEC VERDICT: ship | fix-then-ship | redesign`.
