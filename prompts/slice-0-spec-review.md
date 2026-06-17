You are reviewing a DESIGN SPEC for a single implementation slice, grounded against the ACTUAL a2a-bridge
code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT edit/build/test. Be
rigorous and decisive. Judge **intent, not verbatim wording** (the spec guides a sonnet implementor; small
naming/shape drift in implementation is fine if intent holds). Severity-tag every finding
**BLOCKER / MAJOR / MINOR**.

The spec under review is **Slice 0 — Live Session Core (warm continue)**, the FIRST slice of the
orchestration roadmap. Its job: make a bridge-driven agent warm across tasks (a 2nd A2A `message/send` on the
same `contextId` reuses the same warm ACP session; no `contextId` = today's forget-after), shipping ONLY the
warm `SessionManager` + a minimal real result/event schema — NOT the rich journal, reconcile, clear/compact,
or telemetry (those are later slices). The spec is below.

{{input}}

CONTEXT TO READ (in the repo): `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (Slice 0 row —
the authoritative scope/DoD/deps this spec must honor), `docs/superpowers/specs/2026-06-17-orchestration-
architecture.md` (the converged S1 Session Resource design + the minimal S2 types this slice realizes),
`docs/references/acp-protocol-v1.md`. The architecture/slicing are CONVERGED — do NOT re-litigate them; check
that THIS slice spec is a correct, complete, minimal realization of Slice 0.

REVIEW (ground each point in code with file:line):

1. **Correctness of the code-grounding claims.** Verify against the real code: (a) the inbound `contextId`
   field actually exists + deserializes on the `message/send` path (`a2a-lf-0.3.0` `Message`, and how
   `gate()` parses params `server.rs:~327/347`); (b) `forget_session` only drops the config stash, so a real
   `release_session` removing BOTH `session_cfg` AND `sessions[id]` is needed (`acp_backend.rs:~1805`); (c)
   the registry lease pins the backend (`ports.rs:132`, `registry.rs:~248`); (d) `AcpBackend.sessions` +
   lazy `ensure_session` + `turn_lock` support warm reuse (`acp_backend.rs:337/1184/1578`); (e) the
   `Update::Usage` addition is the only backend-port growth and is consistent with UPDATE-MINIMAL. Flag any
   claim that's wrong or overstated.

2. **Scope discipline.** Does the spec stay WITHIN Slice 0 per the slicing spec — does anything in the IN
   list actually belong to S1–S9 (scope creep), or is anything REQUIRED for a coherent, live-gateable Slice 0
   missing (scope gap)? Is the `Update::Usage`-variant-now / plumbing-later split clean? Is deferring the
   journal/reconcile/clear correct, or does Slice 0 secretly need one of them to gate?

3. **No-redesign-forcing fit with the converged architecture + later slices.** Does the minimal
   `OrchEvent`/`OrchResult` schema + the `SessionManager`/`WarmHandle` shape + the contextId-only identity
   match the architecture's S1/S2 and NOT force a re-cut at S1 (reconcile), S2 (telemetry), S3 (reset —
   note the spec's `backend_session = "ctx-{contextId}-g0"` generation-in-id), S6 (journal dual-store /
   shared seq), or S8 (MCP surface over contextId-only identity)? Is the versioned/`flatten` envelope
   genuinely additive for the deferred Plan/ToolCall variants?

4. **SEQ-AUTHORITY mechanism + concurrency.** Is the mutual-exclusion guard (refuse handle-create on a
   `Working` contextId; refuse detached submit on a live-handle contextId) sufficient + correctly placed?
   Any race between warm-turn stamping and the existing detached TaskStore stamping? Does keep-warm (no
   forget) interact safely with the W3b cancel/drain invariant (`executor.rs:152/321`) — note Slice 0 is the
   single-turn A2A path, NOT the executor (that's S5)?

5. **Lease/leak/lifecycle.** Does a held warm lease deadlock or wrongly block config-reconcile retirement
   (`registry.rs:248`)? Is the TTL/idle reap + `session/release` + `tasks/cancel`-keeps-warm distinction
   correct? `ContainerRwBackend.release_session` reaping (`bridge-container/src/lib.rs:~410`) — real?

6. **Live-gate provability.** For EACH DoD (1–7): is it actually provable on a real serve + codex with the
   stated method (`submit --context` + a `docker ps`/`pgrep` watcher)? Is any DoD unfalsifiable or missing a
   gate (e.g. the latency claim, the reaper→0, the config-mismatch typed error)? Does `submit` need a
   `--context` flag added (is it in scope)?

7. **Ambiguities / under-specification** that would trip a sonnet implementor: the contextId→`SessionId`
   derivation, the config-fingerprint definition (what fields, how compared), the `session/status` response
   shape, where `SessionManager` is instantiated in `serve` and how `gate()`/dispatch consult it, the
   metadata fallback. Name anything that needs pinning before planning.

OUTPUT: findings by severity (BLOCKER/MAJOR/MINOR) each with file:line + a concrete fix; a scope verdict
(in-bounds / creep / gap); a no-redesign verdict; a live-gate-provability verdict. End with one line:
`SPEC VERDICT: ship | fix-then-ship | redesign`. Be a co-architect — propose concrete fixes, not just gaps.
