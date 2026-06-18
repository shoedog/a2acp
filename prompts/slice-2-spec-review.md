You are reviewing a DESIGN SPEC for Slice 2 (Usage telemetry) of the a2a-bridge orchestration work, grounded
against the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. Judge **intent, not verbatim**. Severity-tag findings **BLOCKER/MAJOR/MINOR**. Be a
co-architect — give concrete fixes with file:line.

Slice 0 (warm sessions: `SessionManager`, warm Local dispatch, `SessionSpecFingerprint`, the minimal
`OrchEvent`/`OrchResult`/`UsageSnapshot` DTOs + the `Update::Usage` port variant) and Slice 1 (config
reconcile + `AgentSessionCaps` in `session/status`) are SHIPPED on main. Slice 2 plumbs the ACP `usage_update`
notification — TODAY RECEIVED AND DROPPED — end-to-end: map → `Update::Usage` → surface through the translator
event stream → record on the warm handle → expose in `session/status` → a pre-task threshold warn. The spec is
below.

{{input}}

READ FOR GROUND TRUTH:
- `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (**Slice 2 row = authoritative scope/DoD/deps**;
  note telemetry was SPLIT from reset/clear/compact/watchdog — those are S3/S4/S7, NOT Slice 2).
- `docs/superpowers/specs/2026-06-17-orchestration-architecture.md` — CORRECTION-2 / SPIKE B (telemetry
  feasible, codex emits used+size), P-4 (`max_tokens` is the threshold failure mode), UPDATE-MINIMAL invariant
  (`Update` grows ONLY `Usage`), and the SUPERSEDED-slicing note (the doc's "Slice 2 = telemetry + reset +
  watchdog + session/close" is the BACKED-INTO order; the slicing spec governs scope).
- `docs/superpowers/specs/2026-06-17-slice-0-live-session-core.md` (the shipped DTOs + `Update::Usage` variant +
  the `session/status` shape this slice extends; "usage added Slice 2").
- `crates/bridge-acp/src/acp_backend.rs` — `map_session_update` (~1622, the single drop site that returns
  `None` for non-text), `stop_reason_str` (~1636), the **corpus-replay seams** (~1648-1673) +
  `tests/corpus/codex-acp.jsonl` (the `usage_update` frame); confirm `map_session_update` is the production
  mapping the corpus feeds (purity/determinism constraint).
- `crates/bridge-core/src/orch.rs` — `UsageSnapshot`/`UsageCost`/`OrchResult.usage` (32-100); `ports.rs:24`
  `Update::Usage`.
- `crates/bridge-core/src/translator.rs` — `Event`/`EventKind` (~31-75) + the `Ok(Update::Usage(_)) =>
  continue;` drop (~157). (This slice proposes WIDENING `Event` with `usage: Option<UsageSnapshot>` +
  `EventKind::Usage`.)
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `WarmHandle`/`SessionStatusInfo`/`checkout_turn`/
  `finish_turn`/`status` (the handle this slice adds `usage` to + the threshold check at checkout).
- `crates/bridge-a2a-inbound/src/server.rs` — `warm_local_dispatch` (~553), `WarmTurnGuard`+Drop→`finish_turn`
  (~452-465), `spawn_local_producer` event loop (~1126-1200), `unary_message` (~2266), `session_status` JSON
  (~2826-2862). (The producers — NOT the translator — hold the `sm`+`ctx`; the recording seam lives here.)
- SDK shape: `agent-client-protocol-schema-0.13.2/src/v1/client.rs` `UsageUpdate{used:u64,size:u64,
  cost:Option<Cost{amount,currency}>}` gated `unstable_session_usage` (enabled in `crates/bridge-acp/Cargo.toml`).

REVIEW (ground each in code with file:line):
1. **Faithfulness to the slicing-spec Slice-2 row** — telemetry ONLY (map usage; start/end snapshot;
   queryable `session/status`; pre-task threshold warn; per-backend degrade)? Any **scope creep** (does it pull
   in reset/clear/compact, `session/close`, the rich journal/`OrchEvent`-on-wire, watchdog, executor keep-warm)
   or **gap** (a Slice-2 requirement missing)?
2. **The map un-drop** — is the `SessionUpdate::UsageUpdate → Update::Usage(UsageSnapshot)` branch correct
   against the real SDK shape + the `map_session_update` structure? Is keeping the fn **clock-free** (emit
   `at_ms:0`, stamp downstream) actually required for corpus-replay determinism, and is the downstream-stamp
   plan coherent? Is the `#[cfg(feature="unstable_session_usage")]` gating right?
3. **The recording seam (the load-bearing decision)** — is WIDENING the core `Event` with one
   `Option<UsageSnapshot>` + `EventKind::Usage` the right way to carry usage from the translator to the inbound
   producer (which holds `sm`+`ctx`), vs a sink/closure threaded into `Translator::run`? **Blast radius:** audit
   every `match EventKind` / Event-consuming site (translator, both producers, fan-out synth, delegate,
   workflow projection) — will `Usage` compile + be a correct no-op there? Most important: can usage **leak
   onto the A2A wire** (SSE frame / unary artifact) and break the existing contract (DoD-5)? Is "record iff
   warm, never forward" actually enforceable at both producer sites?
4. **`record_usage` semantics** — latest-wins (multi-`usage_update` per turn → last = end snapshot), `at_ms`
   stamp, idle-refresh-on-usage, no-op on a removed/Released handle, start-vs-end snapshot via the carried
   field — all correct + race-safe under the Slice-0 lock model? Where does `OrchResult.usage` get its end
   snapshot?
5. **`session/status` exposure + degrade** — additive `usage` block + derived `windowFraction`; codex exact /
   claude cost-only (fraction null) / api empty all handled by the `Option` fields with no panic and no false
   `overThreshold`? Is the JSON shape coherent with the Slice-1 caps block?
6. **Pre-task threshold warn** — computed at `checkout_turn` from the CARRIED usage, level-triggered,
   advisory/non-blocking, never mid-turn, never fires without `used`+`size`+`size>0` (no degrade false-positive)?
   Is the chosen surface (serve WARN log + `usage.overThreshold` in status, **no new A2A wire frame**) adequate
   to satisfy the DoD "threshold crossing emits a pre-task warn", or does the DoD demand an orchestrator-visible
   emission this design under-delivers? Config knob (`warm_usage_warn_fraction`, default-disabled) sensible?
7. **No-regression + DoD live-gate provability** — does the change keep Slice-0/1 green (warm continue,
   isolation, release, idle reap, reconcile, caps)? Is each DoD provable on real codex via `submit --context`
   + `session status` + serve-log grep? Is degrade (claude/api) correctly relegated to unit/fixture (codex
   advertises used+size → not live-reachable)?
8. **Ambiguities** that would trip a codex/sonnet implementor: the exact `Event`-widening edits + every match
   site to touch; the producer tap points; the `at_ms` stamp location; the `windowFraction`/`overThreshold`
   null semantics; the threshold config plumbing onto `SessionManager`.

OUTPUT: findings by severity (file:line + fix); scope verdict (in-bounds/creep/gap); no-regression verdict;
live-gate-provability verdict. End: `SPEC VERDICT: ship | fix-then-ship | redesign`.
