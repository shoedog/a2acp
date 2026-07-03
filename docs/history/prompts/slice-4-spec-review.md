You are reviewing the SPEC (design, not the implementation plan) for Slice 4 (Compact) of a2a-bridge, grounded
against the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git log/show/diff`; do
NOT edit/build/test. Judge whether this spec, if planned + implemented faithfully, produces correct, compiling,
spec-faithful code that meets the DoD — and whether the design is sound. Severity-tag BLOCKER / MAJOR / MINOR.
Give concrete fixes (exact code/spec edits, file:line). The spec was drafted from a dual-lens architecture
analysis (Opus + codex-xhigh CONVERGED); your job is to stress it, not rubber-stamp it.

Slice 4 = `compact` = summarize a warm session's context → `reset` to a new generation → seed the summary as
the next turn's first input (`PrependNextTurn`). **require-Idle, NO `force`.** Composition over the SHIPPED
Slice-3 `clear`/`reset_session` primitive. The spec is below.

{{input}}

READ FOR GROUND TRUTH (cite file:line):
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `reset_session` (`:434`, esp. the claim block `:440-471`
  and the release/configure/revalidate/commit-or-EXPIRE tail `:475-519`); `SessionState` (`:19`) + `is_claimed`
  (`:32`); `WarmHandle` (`:39`); `WarmTurn` (`:60`); `checkout_turn` (`:168`, the clean-diff success `:193-205`
  AND the post-reconcile clean success `:261-275`, and the mint path `:295-335`); `finish_turn` (`:341`, the
  `gen && op && Running` guard); `record_usage` (`:375`); `status` (`:352`); `release` (`:391`); `cancel`
  (`:411`, NOTE it sets Idle before prompt completion); the `FakeBackend` + `ManualClock` test harness
  (`:~600+` — does it have a SCRIPTABLE `prompt` returning `Update::Text`? that's needed for the summarize
  tests).
- `crates/bridge-core/src/ports.rs` — `AgentBackend::prompt` (`:34`, returns `BackendStream`); the `Update`
  enum (`:21`, `Text`/`Usage`/`Done`/`Permission`).
- `crates/bridge-workflow/src/executor.rs:~131-148` — the SHIPPED precedent that drains `Update::Text` directly
  (the spec says the summarize collector mirrors this). VERIFY it does what the spec claims.
- `crates/bridge-core/src/translator.rs` — the text path (`~:136-143` last_text; `~:189-195` artifact) — the
  ROOT of the documented unary `result.artifact.text` last-chunk truncation. Confirm the spec's claim that
  driving `backend.prompt` directly ROUTES AROUND it.
- `crates/bridge-acp/src/acp_backend.rs` — `prompt_request` (`:455-463`, `Vec<Part>` → ordered
  `ContentBlock::Text`); lazy re-mint on prompt (`~:2048-2057`); the turn lock.
- `crates/bridge-a2a-inbound/src/server.rs` — dispatch match (`:691`); `session_clear` handler (`:2932`) +
  `context_id_arg` (`~:2856`); `warm_local_dispatch` (`:557`) → `LocalDispatch`/`WarmTurnGuard` (`:452`);
  `spawn_local_producer` (`:1128`, the `parts`→`Translator::run` at `:1150`); the UNARY collect (`~:2292-2540`,
  esp. the artifact pick `~:2516`); `RoutedCall.parts` type.
- `crates/bridge-core/src/domain.rs` — `Part` (`:7`, text-only?); `crates/bridge-core/src/error.rs` —
  `MessageTooLarge` (`:54`), `HandleBusy`, `SessionExpired`, `SessionNotFound` dispositions.
- `bin/a2a-bridge/src/main.rs` — `session_cmd` (`:2724`, the `match sub` + params), help (`:104`).

REVIEW DIMENSIONS (ground each in code):
1. **Spec faithfulness / scope.** Does it implement summarize→reset→seed, require-Idle, no-force, the MVP cut?
   Any scope creep (force, cancel-under-concurrency, journal, MCP, auto-compaction, fixing the unary
   truncation) that belongs in a later slice or the deferred follow-up? Any GAP vs the slicing-spec row-4 DoD?
2. **Composition correctness — the central design.** Is composing over `reset_session` INTERNALS (under one
   `Compacting` claim) correct, vs the rejected naive public `reset_session()` call (self-rejects as busy) and
   the rejected release-to-Idle-then-reset (interleaving)? Is the claim discipline sound: `Compacting` in
   `is_claimed`; revalidate the EXACT claim `(id, Compacting)` after the async summarize AND after configure;
   honor a deferred release/cancel; NO `?` that strands `Compacting`? Walk the happy path AND each early-return
   and confirm no stranded claim / no double-release / no lost handle.
3. **Summarize capture.** Is the direct `backend.prompt` drain (concatenate `Update::Text`, ignore
   `Usage`/`Permission`, stop on `Done`) correct and truncation-free? Is the `executor.rs` precedent real +
   applicable? Should the summarize bypass the policy/store (zero-footprint) — any consequence (permission
   prompts mid-summarize? the summarize turn needs a TaskId anywhere)?
4. **Seed mechanism.** Is take-and-clear at BOTH `checkout_turn` success returns (`:193-205` AND `:261-275`)
   and NOWHERE else (a HandleBusy/reseed/Err return must NOT consume the seed)? Are BOTH prepend sites
   (streaming `:1150` + unary `~:2354`) covered (the Slice-2 multi-site trap)? Is dropping the seed in
   `reset_session` (plain clear) correct + does the spec actually wire it? Ordering (seed first)? Lifecycle on
   eviction acceptable?
5. **Concurrency / races.** require-Idle→`HandleBusy`; does `Compacting` correctly defer release/cancel (no
   reopened Slice-1/3 ABA)? Is the spec genuinely CLEAN of the two DEFERRED races
   (`2026-06-18-FOLLOWUP-warm-turn-cancellation-tokens.md`)? Is the cancel caveat (compact-after-cancel) handled
   right (naturally-idle-only + the live-gate constraint)?
6. **Failure modes.** abort-revert (Err/empty/oversize) preserving gen-N context; reset-fail-after-good-summary
   → EXPIRE (never report success); the `MessageTooLarge` bound. Any mode missing or mishandled?
7. **Wire/CLI.** `SessionCompact {contextId}` (no force) in dispatch; handler shape vs `session_clear`; result
   `{contextId,compacted,generation}`; `NotFound`→`SessionNotFound`; CLI `compact`→`SessionCompact` + help. Right?
8. **TDD realizability.** Do the listed tests lean on harness that EXISTS, or need new helpers (esp. a
   SCRIPTABLE `FakeBackend::prompt` emitting `Update::Text` for the summarize path — does it exist or must the
   plan add it)? Is `compact_bad_summary_preserves_context` (the keystone) actually writable + meaningful?
9. **The OPEN QUESTIONS O6–O9** (§10): give a concrete recommendation for each (the `MAX_SUMMARY_BYTES` default;
   live-gate determinism; `Part` text-only; the seed-consume sites).

OUTPUT: findings by severity (dimension #, file:line, concrete fix); spec-faithfulness verdict; design-soundness
verdict; recommendations for O6–O9. End with exactly: `SPEC VERDICT: ready-to-plan | fix-then-plan | rework`.
