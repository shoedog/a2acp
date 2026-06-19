# Slice 4 — Compact — Spec

> **Status:** v2 (dual spec-review folded). Drafted from the dual-lens analysis
> (`2026-06-18-slice-4-compact-ANALYSIS.md`), then dual spec-reviewed (codex-xhigh + Opus, both `fix-then-plan`)
> — FIX-1..14 below are BINDING. Next: plan → dual plan-review.
>
> **Roadmap:** Slice 4 of the orchestration MVP (Slices 0–5). Composition over the SHIPPED Slice-3 `clear`
> primitive. Authoritative slice scope: `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (row 4).

## v2 — dual spec-review fixes folded (BINDING)

Both reviewers returned `fix-then-plan`. The keystone (codex BLOCKER + Opus MAJOR): the summarize step is a
REAL `backend.prompt` on the LIVE old session — it **irreversibly mutates that context** (ACP `session/prompt`;
no rollback API). So the v1 "abort-revert preserves gen-N context" was wrong. The folded contract:

- **FIX-1 (keystone) — bad summary EXPIRES the handle, never restore-Idle.** On ANY post-claim summarize
  failure (closure `Err` / empty-or-whitespace / over `MAX_SUMMARY_BYTES` / timeout / a `Permission` update):
  re-acquire the lock, revalidate the exact claim, set `Expiring`, `release_session(old_id)`, remove the handle
  + drop the lease, return the original error. The old context is already polluted by the (failed) summarize
  exchange, so preserving it is not available without a snapshot/rollback primitive (OUT of the MVP cut).
- **FIX-2 — failure-expire also resolves the deferred-release/cancel-during-compact contradiction.** A
  `release`/`cancel` arriving during `Compacting` sets `expire_after_reconcile`; since EVERY compact failure
  now EXPIREs (removes the handle), the deferral is honored by construction. The happy path uses the reset
  tail's existing `deferred → EXPIRE` check. No "restore Idle" path survives that could silently drop a deferral.
- **FIX-3 — reset-failure error contract matches `reset_session`.** A `configure_session` error after a good
  summary returns the **configure error** (handle removed), per `session_manager.rs:496-508` — NOT
  `SessionExpired`. Only a deferred release/cancel WITH a successful configure returns `SessionExpired`.
- **FIX-4 — `Update::Permission` during summarize ⇒ compact FAILURE (→ EXPIRE).** The direct `prompt` drain
  bypasses the translator's policy enforcement (`translator.rs:151`); a permission update during summarize must
  NOT be silently ignored (that would be a policy bypass). `SUMMARIZE_PROMPT` also instructs **no tool use / no
  file reads**. (ACP resolves permission internally — its prompt stream yields only `Text`/`Usage`/`Done`/
  `Failed`, `acp_backend.rs:~1859` — so in practice this won't fire; the trait allows it, so we define it.)
- **FIX-5 — bounded summarize TIME.** Wrap the summarize await in `tokio::time::timeout`
  (`compact_summarize_timeout`, generous default e.g. 120 s); on elapse → a summarize failure → EXPIRE (FIX-1).
  Closes the never-`Done`-agent permanent-`Compacting` wedge (a `Compacting` handle is reaper-immune and blocks
  all checkout/release).
- **FIX-6 — SERVER test-fake extensions are required plan tasks.** (AMENDED at plan time — the closure design
  below supersedes the original "scriptable manager fake" requirement.) Because `compact_session` takes the
  summarize as an INJECTED closure (§4), the **manager** summarize tests pass a fake closure directly
  (`|_,_| async { Ok("S".into()) }` / `Err(..)` / `pending()`), so `session_manager.rs`'s `FakeBackend` needs
  NO scriptable `prompt`. Only the **server** fakes need work: a one-shot `ScriptedBackend` (emitting scripted
  `Update`s — `Update` is not `Clone`, so store `Option<Vec<Update>>`) for the `summarize_collect` tests; and a
  parts-RECORDING `prompt` on the WARM fake `WarmRecordingBackend` (`server.rs:5804`) so the seed-order tests
  assert the wrapped seed is the FIRST part of the LATEST turn (the warm-up turn is recorded first).
- **FIX-7 (O6) — `MAX_SUMMARY_BYTES = 32 * 1024`**, byte-counted (`String::len()`), enforced DURING the drain
  (stop + fail on exceed; don't accumulate unboundedly then check). Non-configurable const for the MVP.
- **FIX-8 (O8) — seed = `Option<String>`, WRAPPED at injection** with a prior-context framing prefix (e.g.
  `"[Summary of earlier context in this session]\n{summary}"`), then built into `Part { text }`. Text-only
  confirmed (`Part` is text-only; ACP preserves ordered text blocks `acp_backend.rs:455-463`).
- **FIX-9 (Mi1) — the fallible cwd parse happens BEFORE flipping to `Compacting`** (captured inside the claim
  block, as `reset_session:456-467`), else a `ConfigInvalid` strands `Compacting`.
- **FIX-10 (O9/Mi3) — seed take-and-clear ONLY at the two resume success returns** (`checkout_turn:193-205`
  clean-diff + `:261-275` post-reconcile-clean); explicitly NOT at the mint path (`:295-335`, no seed) nor any
  HandleBusy/reseed/`Err` return (the seed stays for the next SUCCESSFUL checkout).
- **FIX-11 (Mi2) — CLI:** add `"compact"` to the missing-subcommand error string (`main.rs:2728`) too.
- **FIX-12 (M3) — `MessageTooLarge` maps to `-32603 INTERNAL`** (its `disposition()` is `SetState(Failed)`,
  `error.rs:121`), NOT `-32600` like the `RejectRequest` errors. Acceptable (a server-side degenerate-output
  condition); documented, not implied-uniform.
- **FIX-13 (O7) — live-gate:** tighten the PID check (isolated serve process / before-after PID set); keep the
  explicit retain/exclude framing; accept residual model flake (re-run).
- **FIX-14 (Mi4) — note the summarize turn's usage is intentionally NOT recorded** (zero-footprint; the reset
  zeroes usage anyway). Not a telemetry gap.

## Goal

`compact` lets an operator shed a warm session's accumulated token weight WITHOUT a cold respawn: **summarize
the current context, reset to a fresh generation, and seed the summary as the next turn's first input.** The
process stays warm; the generation advances; raw prior detail (outside the summary) is gone.

`compact(ctx) = summarize(gen N) → reset(→ gen N+1) → seed-summary-as-next-turn-prepend`, **all under one
claim**, **require-Idle, no `force`.**

## 1. Scope

**IN:**
- `SessionState::Compacting` claim state (sibling to `Resetting`).
- `SessionManager::compact_session(ctx, summarize_fn)` — claim-held summarize → reset-internals → seed-stash.
- `pending_seed: Option<String>` on `WarmHandle`; take-and-clear in `checkout_turn`; **prepend at BOTH dispatch
  sites** (streaming producer + unary collect).
- A summarize text-capture helper (direct `AgentBackend::prompt` drain, concatenating `Update::Text`).
- `SessionCompact {contextId}` wire method + `session compact <contextId>` CLI.
- A bounded-summary backstop (`BridgeError::MessageTooLarge`).

**OUT (explicitly):**
- `force` / cancel-under-concurrency (→ the DEFERRED `warm-turn cancellation tokens` follow-up,
  `docs/superpowers/2026-06-18-FOLLOWUP-warm-turn-cancellation-tokens.md`). Compact is naturally-idle-only.
- Fixing the pre-existing unary `result.artifact.text` truncation (compact routes AROUND it; separate
  follow-up).
- True mid-turn injection / queued-inject / the full Turn Channel (Slice 9). Slice 4 needs only the single
  next-turn prepend.
- Journal richness / observability of the summarize turn (Slices 6/7). The summarize turn is zero-footprint
  (no store/task rows).
- Auto-compaction heuristics (deferred A3). Compact is a MANUAL operator lever.

## 2. Wire + CLI surface

**Wire** (CamelCase string dispatch, `server.rs:691`, sibling to `SessionClear`):
- Method `"SessionCompact"`, params `{ "contextId": <string> }` (parsed via `context_id_arg`, `server.rs:~2856`).
  **No `force`.**
- Handler `session_compact` (modeled on `session_clear` `server.rs:2932`): `authorize_headers` → require
  `session_manager` (else `METHOD_NOT_FOUND`) → `context_id_arg` → `sm.compact_session(...)`. Result mapping:
  - `Ok(ResetOutcome::Cleared { generation })` → `{ "contextId": C, "compacted": true, "generation": N }`.
  - `Ok(ResetOutcome::NotFound)` → `SessionNotFound` (`bridge_err_to_jsonrpc`).
  - `Err(HandleBusy)` (not Idle) / `Err(SessionExpired)` / `Err(ConfigInvalid)` / summarize error → typed
    JSON-RPC error via `bridge_err_to_jsonrpc` (the `RejectRequest` ones → `-32600`). `Err(MessageTooLarge)`
    maps to **`-32603 INTERNAL`** (its `disposition()` is `SetState(Failed)`, `error.rs:121`) — acceptable for
    a server-side degenerate-output condition; documented, not implied-uniform with the `-32600` rejections
    (FIX-12).

**CLI** (`bin/a2a-bridge/src/main.rs:2724`): add `"compact" => "SessionCompact"` to the `match sub`; params
`{ "contextId": ctx }` (no `--force`); update the help line (`:104`,
`status | release | cancel | clear | compact <contextId>`).

## 3. Data-model changes

- **`SessionState`** (`session_manager.rs:19`): add `Compacting`. Add to `is_claimed` (`:32`) so a concurrent
  `checkout_turn` → `HandleBusy` and `release`/`cancel` DEFER (set `expire_after_reconcile`) instead of
  mutating/removing the handle. `status()` (`:352`) maps `Compacting` → `"compacting"`.
  - **Exhaustiveness:** adding the variant breaks every exhaustive `match SessionState` (compiler-enforced under
    `cargo test --workspace --no-run`). The task that adds it fixes all arms.
- **`WarmHandle`** (`:39`): add `pending_seed: Option<String>` (default `None` at mint). Set ONLY by a
  successful `compact_session`. Dropped to `None` by `reset_session` (a plain `clear` after a compact = empty
  context, so a pending summary is part of what's cleared).
- **`WarmTurn`** (`:60`) + **`LocalDispatch`** (`server.rs`): add `seed: Option<String>` carrying the taken
  seed to the producer/unary path.
- **`SessionManager`** (`:108`): add `compact_summarize_timeout: Duration` (FIX-5), defaulted (e.g. 120 s) and
  wired from config like `idle_ttl` (a `[server].compact_summarize_timeout_secs` knob in `bin/.../config.rs`;
  the constructor gains the param, existing call sites pass the default). Module consts:
  `const MAX_SUMMARY_BYTES: usize = 32 * 1024;` (FIX-7) and the baked `SUMMARIZE_PROMPT: &str` (§5).

## 4. `compact_session` algorithm

```rust
pub async fn compact_session<F, Fut>(
    &self,
    ctx: &ContextId,
    summarize: F,
) -> Result<ResetOutcome, BridgeError>
where
    F: FnOnce(Arc<dyn AgentBackend>, SessionId) -> Fut,
    Fut: std::future::Future<Output = Result<String, BridgeError>>,
{ ... }
```

Steps (mirrors `reset_session:434-520`, with summarize inserted before the reset tail, all under `Compacting`):

1. **Claim under one lock** (`by_context.lock()`): `None` → `Ok(ResetOutcome::NotFound)`. State must be
   `Idle` (no `force` arm — `Running`/any claimed → `Err(HandleBusy)`). Capture
   `(backend, old_id, claimed_id, new_gen=gen+1, new_id="ctx-{ctx}-g{N}", spec)` exactly as
   `reset_session:450-467` — **incl. the fallible `SessionCwd::parse`→`ConfigInvalid`, which runs BEFORE the
   state flip (FIX-9)**, so a cwd-parse failure returns without stranding `Compacting`. Then set
   `state = Compacting`, `expire_after_reconcile = false`. Drop lock.
2. **Summarize (outside the lock, claim held), TIME-BOUNDED:**
   `let result = tokio::time::timeout(compact_summarize_timeout, summarize(backend.clone(), old_id.clone())).await;`
   The closure drives `backend.prompt(&old_id, vec![Part{text: SUMMARIZE_PROMPT}])` and concatenates every
   `Update::Text` (enforcing `MAX_SUMMARY_BYTES` DURING the drain), treats an `Update::Permission` as a failure,
   stops on `Update::Done` (§5). A `Compacting` handle is reaper-immune (`reap_idle` reaps only `Idle`), so the
   timeout (FIX-5) is what prevents a never-`Done` agent from wedging the handle.
3. **Bad summary ⇒ EXPIRE the handle (FIX-1/2/4/5), NOT restore-Idle.** A `summarize` failure — closure `Err`,
   empty/whitespace-only, over `MAX_SUMMARY_BYTES` (`MessageTooLarge`), `timeout` elapsed, or a `Permission`
   update — means the old gen-N context is already polluted by the (failed) summarize exchange and cannot be
   cleanly preserved (no rollback primitive). So: re-acquire the lock, revalidate the EXACT claim
   (`h.id == claimed_id && state == Compacting`); if still ours, set `state = Expiring`, drop lock,
   `release_session(old_id).await`, re-acquire, remove the handle + drop its lease, return the ORIGINAL error.
   (Mirrors the non-clean tail of `checkout_turn:276-292`.) This single path also honors any deferred
   release/cancel that arrived during the summarize (FIX-2) — the handle is removed either way.
4. **Good summary ⇒ reset tail (claim still `Compacting`)** — run `reset_session:475-519` verbatim against
   `Compacting` (release old → configure new → re-acquire → revalidate `(claimed_id, Compacting)` →
   commit-or-EXPIRE). On a **`configure_session` error** the handle is removed and the **configure error** is
   returned (FIX-3, per `reset_session:496-508`), NOT `SessionExpired`; only a deferred release/cancel WITH a
   successful configure returns `SessionExpired`. On commit, additionally set
   `h.pending_seed = Some(summary)`. Final state `Idle`, generation `N+1`, usage zeroed, `op = None`. Return
   `Ok(ResetOutcome::Cleared { generation: N+1 })`.

**Invariants:** no `?` between the claim and a resolved state — every early return explicitly EXPIREs (failure)
or commits (success); there is NO restore-to-`Idle` path after the summarize prompt is sent (FIX-1). The
summarize runs on the gen-N session while `Compacting` blocks all other turns (atomic). Reuse
`reset_session`/`checkout_turn`'s capture-then-EXPIRE tombstone discipline (no early `?` strands the handle).

## 5. Summarize capture + bound

- **Capture (route around truncation):** the summarize closure drives `AgentBackend::prompt` (`ports.rs:34`)
  directly and concatenates every `Update::Text(s)` (`ports.rs:22`), stopping at `Update::Done` (`:25`). It
  **ignores `Update::Usage`** (its tokens are intentionally NOT recorded — zero-footprint; the reset zeroes
  usage anyway — FIX-14) and **treats `Update::Permission` as a FAILURE** (FIX-4 — silently ignoring it would
  make compact a policy bypass; the handler must not approve tool use during a summary). This mirrors the
  SHIPPED `Update::Text`-drain precedent in `crates/bridge-workflow/src/executor.rs:131-148`. It does NOT touch
  the unary `result.artifact.text` assembly (`server.rs:~2516`), the source of the documented last-chunk
  truncation. Zero store/task/policy footprint — no `TaskId`.
- **SUMMARIZE_PROMPT** (a baked `const`): instruct a faithful, self-contained summary of the conversation so
  far that a fresh session could continue from — preserve durable facts/decisions/identifiers; exclude
  throwaway/scratch values explicitly marked temporary; **and explicitly: do NOT use tools, read files, or run
  commands — reply with the summary text only** (FIX-4). Model-agnostic wording.
- **Byte bound (FIX-7):** `MAX_SUMMARY_BYTES = 32 * 1024` (non-configurable const), byte-counted via
  `String::len()`, **enforced DURING the drain** — stop and fail the moment the accumulator would exceed it
  (do not accumulate unboundedly then check). Overflow → `BridgeError::MessageTooLarge` (`error.rs:54`) → the
  §4-step-3 EXPIRE path. The prompt's concision instruction is the primary control; the ceiling is the backstop.
- **Time bound (FIX-5):** the summarize await is wrapped in `tokio::time::timeout(compact_summarize_timeout, ..)`
  (generous default, e.g. 120 s); elapse → the §4-step-3 EXPIRE path. Prevents a never-`Done` agent from
  wedging the reaper-immune `Compacting` handle.

## 6. Seed injection (PrependNextTurn)

- **Take-and-clear (FIX-10):** in `checkout_turn` (`session_manager.rs:168`), at the TWO successful
  existing-handle `Running` transitions ONLY — the clean-diff path `:193-205` and the post-reconcile clean path
  `:261-275` — atomically `let seed = h.pending_seed.take();` and return it in `WarmTurn { seed, .. }` (inside
  the existing lock hold, no new lock). Do NOT take at: the MINT path (`:295-335` — a fresh handle has no seed),
  the `HandleBusy`/`ConfigMismatch`/`ConfigReseedRequired` early returns, or the reconcile-EXPIRE non-clean
  path. A non-successful checkout leaves the seed in place for the next SUCCESSFUL checkout.
- **Carry:** `warm_local_dispatch` (`server.rs:557`) puts `turn.seed` into `LocalDispatch.seed`.
- **Wrap + prepend at BOTH sites (FIX-8; the Slice-2 multi-site lesson — a test must cover each):** build the
  seed `Part` as `Part { text: format!("[Summary of earlier context in this session]\n{seed}") }` so the agent
  reads it as PRIOR context, then `parts.insert(0, seed_part)` BEFORE `Translator::run` at:
  - streaming `spawn_local_producer` (`server.rs:1138/:1150`), and
  - unary collect (`server.rs:~2354`).
- **Ordering:** seed is the FIRST part (prior context the agent reads before the user's new message).
- **Lifecycle:** set only by a successful `compact_session`; dropped to `None` by `reset_session` (a plain
  `clear` after a compact = empty context — the seed-drop line is added to `reset_session`'s commit block
  `:510-516`); lost on release/reap/eviction/expire (ACCEPTABLE — warm state is in-memory/volatile by design;
  the fallback is a cold session). Documented, not hardened.

## 7. Concurrency invariants

- **require-Idle:** non-`Idle` compact → `HandleBusy` (mirrors `reset_session(force:false)` `:445-448` and
  `checkout_turn` `:181-184`). `HandleBusy` is a request rejection (`error.rs:~38`), not a task failure.
- **No reopened races:** holding `Compacting` (in `is_claimed`) blocks re-claim and defers release/cancel — the
  SAME discipline that closed the Slice-1/3 ABA + release-reuse races. The generation guard on
  `finish_turn`/`record_usage` (`gen && op && Running`, `:341/:375`) is untouched.
- **Clean of the two DEFERRED races:** compact never calls `force` and never cancels a live turn (require-Idle
  ⇒ no live turn). It does not touch the cancel→next-turn op path or the force-clear-vs-producer-start gap.
- **Cancel caveat (codex #3):** `cancel()` flips state to `Idle` BEFORE the ACP prompt actually completes
  (`session_manager.rs:~423-431`; ACP reports cancellation completion as the prompt result, not the notify).
  So a compact fired immediately after a `cancel` could summarize a still-draining turn — that ambiguity is
  part of the deferred cancellation-token hardening. **Compact is specified for naturally-idle sessions; the
  live-gate uses naturally-completed turns (NO cancel-then-compact).**

## 8. Failure modes

All post-claim summarize failures EXPIRE the handle (FIX-1) — the old context is already mutated by the failed
summarize exchange and cannot be cleanly preserved without a rollback primitive (OUT of the MVP). The operator
gets a typed error + a cold next session; never a silent pollution or silent total wipe.

| Mode | Handling |
|---|---|
| summarize `prompt` errors | EXPIRE the handle; return the error. (Old context was already touched.) |
| summarize empty/whitespace-only | EXPIRE; typed error. (Do NOT reset to an empty seed.) |
| summary over `MAX_SUMMARY_BYTES` | EXPIRE; `MessageTooLarge` (→ `-32603`, FIX-12). Enforced during drain. |
| summarize `timeout` elapsed (FIX-5) | EXPIRE; timeout error. Closes the never-`Done` wedge. |
| `Update::Permission` during summarize (FIX-4) | EXPIRE; treat as failure (no silent policy bypass). |
| `configure_session` fails after a good summary (FIX-3) | reuse `reset_session`'s remove-and-return path → returns the **configure error** (e.g. `ConfigInvalid`), handle removed. NOT `SessionExpired`. Never report success. |
| deferred release/cancel arrives during compact (FIX-2) | the failure-EXPIRE (or the reset tail's `deferred→EXPIRE`) removes the handle → the deferral is honored; on a deferred-with-clean-configure the reset tail returns `SessionExpired`. |
| stranded `Compacting` | impossible — every early return EXPIREs or commits; the cwd-parse `?` is before the flip (FIX-9); no `?` between claim and resolution. |
| concurrent checkout/release/cancel during compact | `HandleBusy` (checkout) / deferred via `is_claimed` (release/cancel). |
| backend lacks capability | none needed — summarize is an ordinary `prompt` turn. |
| no `session_manager` configured | `METHOD_NOT_FOUND` (matches the other session methods). |

## 9. DoD + live-gate (real codex)

**DoD:** (1) a planted durable fact survives `compact` and is recalled on the next turn; (2) a planted
explicitly-throwaway token is GONE after compact; (3) the SAME agent process stays warm across the compact
(pid unchanged); (4) the generation advances and usage resets.

**Live-gate** (serve config = copy `examples/a2a-bridge.slice3-livegate.toml`, codex, large TTL, port 8098):
```
CTX=s4-<ts>; URL=http://127.0.0.1:8098; B=./target/release/a2a-bridge
# plant: a durable fact + an explicitly-throwaway token
$B submit --url $URL --agent codex --context $CTX --input plant.txt   # "Durable: compact-key ALPHA-7429.
#                                       Throwaway scratch RAW-9f3c (do NOT retain). Reply OK."
$B session status $CTX --url $URL                                     # generation 0, idle
PID0=$(pgrep -f 'codex.*acp' | sort)
$B session compact $CTX --url $URL                                    # {compacted:true, generation:1}
$B session status $CTX --url $URL                                     # generation 1, idle, usage null
PID1=$(pgrep -f 'codex.*acp' | sort); [ "$PID0" = "$PID1" ]           # SAME process (warm)
$B submit --url $URL --agent codex --context $CTX --input probe.txt   # "JSON: compact_key? raw_nonce?
#                                       null if absent from current context."
# expect: compact_key=ALPHA-7429 (summary survived + seeded), raw_nonce=null (raw detail gone)
```
The explicit "do NOT retain" framing on the throwaway + the JSON null-if-absent probe make the
"survives/gone" discrimination as deterministic as a real model allows (O7; re-run if flaky).

**Also unit-gated** (no live model): the claim→commit / claim→EXPIRE state transitions, the
**bad-summary-EXPIRES-the-handle** property (FIX-1), the seed take-and-clear + dual-site prepend, the timeout
EXPIRE, `MessageTooLarge`, `HandleBusy` on non-Idle — on the `FakeBackend`/`ManualClock` harness
(`session_manager.rs:~600+`, `server.rs` warm-test harness). **Harness extensions required (FIX-6, plan tasks):** (a) the manager summarize tests INJECT a fake closure
(`Ok("S")` / `Err(..)` / `pending()`) — NO scriptable manager fake needed (the closure design); (b) a one-shot
`ScriptedBackend` (`Option<Vec<Update>>`, `Update` is not `Clone`) for the `summarize_collect` tests; (c) the
WARM fake `WarmRecordingBackend` (`server.rs:5804`) gains `prompted_parts` recording + `clear_prompted_parts`
so `seed_prepended_streaming`/`seed_prepended_unary` assert
`prompted_parts().last()[0] == "[Summary of earlier context in this session]\n…"` (the warm-up turn is
recorded first).

## 10. Open questions — RESOLVED by the dual spec-review

- **O6 → RESOLVED (FIX-7):** `MAX_SUMMARY_BYTES = 32 * 1024`, non-configurable const, enforced during drain.
- **O7 → RESOLVED (FIX-13):** keep the retain/exclude framing + JSON null-if-absent probe; tighten the PID
  check (before/after PID set on an isolated serve); accept residual model flake (re-run).
- **O8 → RESOLVED (FIX-8):** text-only `String` (`Part` is text-only; ACP preserves ordered text blocks);
  WRAP it as prior-context at injection.
- **O9 → RESOLVED (FIX-10):** take-and-clear ONLY at the two `checkout_turn` resume success returns
  (`:193-205`, `:261-275`); NOT at mint / HandleBusy / reseed / Err.

No open questions remain — `SPEC VERDICT (both lenses): fix-then-plan`, all fixes folded → ready to plan.

## Test plan (TDD)

Harness first (FIX-6): a scriptable/queued `FakeBackend::prompt` (`Update::Text` chunks) in
`session_manager.rs` tests; a parts-RECORDING `FakeBackend::prompt` in `server.rs` tests.

- `compact_advances_generation_and_seeds` — scripted good summary → generation N+1, `pending_seed=Some(wrapped)`,
  `release_session(old)` + `configure_session(new)` called, state Idle.
- `compact_on_running_is_handle_busy` — require-Idle (no force).
- `compact_bad_summary_expires_handle` (keystone, FIX-1) — summarize `Err` OR empty/whitespace → handle REMOVED
  (`status()`==None), `release_session(old)` called, returns the error. (NOT restore-Idle, NOT a new generation.)
- `compact_summary_timeout_expires` (FIX-5) — a never-`Done` scripted prompt + a `ManualClock`/short timeout →
  EXPIRE.
- `compact_oversize_summary_message_too_large` (FIX-7) — drain exceeds 32 KiB → `MessageTooLarge` + EXPIRE.
- `compact_permission_during_summarize_expires` (FIX-4) — scripted `Update::Permission` → EXPIRE (failure).
- `compact_configure_failure_returns_configure_error` (FIX-3) — good summary, `configure_session` Err → handle
  removed, returns the configure error (not `SessionExpired`).
- `compact_unknown_ctx_is_not_found`.
- `checkout_consumes_seed_once` (FIX-10) — first resume-checkout takes it, second sees `None`; a `HandleBusy`/
  reseed checkout does NOT consume it.
- `clear_drops_pending_seed` — `reset_session` after a compact clears the seed.
- server: `session_compact_dispatch` (Cleared→`{compacted,generation}`), `session_compact_unknown_ctx_is_not_found`
  (→`SessionNotFound`), `seed_prepended_streaming` + `seed_prepended_unary` — both assert the recording fake's
  `recorded[0][0] == "[Summary of earlier context in this session]\n…"` (both injection sites, FIX-8).
