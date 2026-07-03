You are reviewing a DESIGN SPEC for Slice 1 of the a2a-bridge orchestration work, grounded against the ACTUAL
code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT edit/build/test. Judge
**intent, not verbatim**. Severity-tag findings **BLOCKER/MAJOR/MINOR**. Be a co-architect — concrete fixes.

Slice 0 (warm sessions: `SessionManager`, warm Local dispatch, `SessionSpecFingerprint`) is SHIPPED on main.
Slice 1 upgrades the warm-continue mismatch path from "reject" to "reconcile model/effort when the agent
advertises it; cwd→reject; mode→reseed-required" + records agent capabilities. The spec is below.

{{input}}

READ FOR GROUND TRUTH:
- `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (Slice 1 row = authoritative scope/DoD/deps).
- `docs/superpowers/specs/2026-06-17-slice-0-live-session-core.md` + `crates/bridge-a2a-inbound/src/
  session_manager.rs` (the SHIPPED Slice-0 `checkout_turn`/`WarmHandle`/fingerprint this slice modifies).
- `crates/bridge-acp/src/acp_backend.rs` — the config-application sites the spec proposes to LIFT:
  `configure_model_option` (~524), the effort walk-down (~649), `set_mode_request`/`set_config_option_request`/
  `set_config_option` (~459/468/480), `ensure_session`'s init closure (~1197-1411), `agent_capabilities()`
  (~1060), `set_session_model` (~602).
- `crates/bridge-core/src/{ports.rs,error.rs,domain.rs,session_fingerprint.rs}`.

REVIEW (ground each in code with file:line):
1. **Spec faithfulness to the slicing-spec Slice-1 row** — does it implement reconcile (model/effort apply
   when advertised; cwd reject; mode reseed-required) + capability RECORDING (not actions)? Any scope creep
   (does it pull in clear/compact/telemetry/load-resume actions) or gap (missing a Slice-1 requirement)?
2. **The `reconcile_config` LIFT** — is extracting `apply_model_effort` from `ensure_session`'s init closure
   sound? Verify the real init-closure structure (~1197-1411): can the model+effort application actually be
   factored out + called on a live `agent_session_id` WITHOUT changing mint behavior or the effort walk-down
   fallback semantics? Does `reconcile_config` correctly require an already-minted session (ensure_session
   first)? Any hazard in re-applying config to a live session mid-life (codex/claude `config_options` vs kiro
   `session/set_model`)? Is `Applied`/`NotAdvertised`/`Rejected` mapping derivable from the existing helpers?
3. **The `checkout_turn` reconcile path** — is the per-field routing (agent/cwd→ConfigMismatch;
   mode→ConfigReseedRequired; model/effort→reconcile→Applied|ConfigReseedRequired) correct + safe under the
   Slice-0 `HandleBusy`/lock model? Is advancing `handle.fingerprint` only-on-Applied, under the lock,
   race-safe? Does it preserve Slice-0 mint + back-compat?
4. **`AgentCaps`** — bridge-owned (not raw SDK) correct? Does the `agent_capabilities()` mapping cover the
   ACP `AgentCapabilities`/`sessionCapabilities` shape? Is recording-only (no actions) the right Slice-1 line?
5. **`ConfigReseedRequired` vs `ConfigMismatch`** — is the distinction (reseed-able later via clear/compact vs
   truly frozen agent/cwd) coherent + worth a new variant, or over-engineered? Disposition/client_message ok?
6. **No-regression + DoD live-gate provability** — does the change keep Slice-0 green? Is each DoD (esp.
   model/effort reconcile applies live; mode→reseed; caps surfaced) provable on real codex via
   `submit --context --effort` + `session status`? Is the NotAdvertised path gate-able (codex may advertise
   everything → unit-test it)?
7. **Ambiguities** that would trip a sonnet/codex implementor: the exact lift seam; how reconcile finds the
   live agent_session_id; the fingerprint-advance ordering; the AgentCaps mapping fields; the status-field
   addition.

OUTPUT: findings by severity (file:line + fix); scope verdict (in-bounds/creep/gap); no-regression verdict;
live-gate-provability verdict. End: `SPEC VERDICT: ship | fix-then-ship | redesign`.
