# Slice 4 (Compact) — Architecture Analysis (pre-spec)

> Pre-spec design analysis for Slice 4. Two independent lenses fold here: **this = the Opus lens**
> (code-grounded against the shipped Slice-0..3 code); the **codex gpt-5.5 xhigh lens** runs in parallel
> (`prompts/slice-4-arch-analysis.md` + `/tmp/slice4-arch-brief.md`) and folds in below. This doc becomes the
> input to the formal spec `docs/superpowers/specs/2026-06-18-slice-4-compact.md`.
>
> **Goal of Slice 4:** `compact` = summarize the warm session's context, reset to a fresh generation, and seed
> the summary as the next turn's first input — so a long-running warm session sheds raw token weight while
> keeping the gist, WITHOUT a cold respawn. **require-Idle, no `force`.** Last context-mgmt slice before S5
> closes the MVP.

## 0. The composition (what's reused vs new)

`compact = summarize(gen N) → reset_session(→ gen N+1) → seed-summary-as-next-turn-prepend.`

**Reused as-is (shipped):**
- `SessionManager::reset_session` (`session_manager.rs:434`) — the whole claim→release-old→configure-new→
  revalidate→commit-or-EXPIRE machinery, incl. the `Resetting` claim state (`:29`), `is_claimed` deferral
  (`:32`), generation bump (`ctx-{ctx}-g{N}`), usage-zeroing, `op=None`. Compact is `reset_session` + a
  summarize-before + a seed-stash-after, under ONE claim.
- The turn input path: `prompt_request` (`acp_backend.rs:455`) already builds a turn from a `Vec<Part>` → each
  `ContentBlock::Text`. So "PrependNextTurn" = **insert one `Part` at the front of the next turn's `parts`** —
  no Turn Channel (that's S9), exactly as the architecture's OPEN-3 note says.
- The producer (`spawn_local_producer` `server.rs:1150`) feeds `routed.parts` straight into `Translator::run`.
  The seed is prepended to `parts` right there (and at the unary collect, `~:2354`).
- `AgentBackend::prompt(session, Vec<Part>) -> BackendStream` of `Update` (`ports.rs:34`, `Update::Text(String)`
  / `Update::Done` `:21-25`) — the summarize turn drives THIS directly and accumulates `Update::Text`.

**Genuinely new:**
1. A `SessionManager::compact_session(...)` op that holds a claim across summarize+reset+seed.
2. A `pending_seed` field on `WarmHandle` + take-and-clear in `checkout_turn` + prepend at the 2 dispatch sites.
3. The summarize text-capture helper (accumulate `Update::Text`).
4. The `SessionCompact` wire method + handler + `session compact` CLI.

Composition is clean. The only non-trivial seam is **#1 — where the summarize turn runs while keeping the
operation atomic** (§2).

## 1. THE CENTRAL DECISION — where the summarize turn runs + atomic capture

Three options weighed; **RECOMMEND Option C** (closure-into-manager, single method, claim-held).

- **(A) Manager runs summarize via a backend prompt-collect call it owns.** The manager gains a streaming/
  collect responsibility it does not have today (it only calls release/configure/cancel/reconcile on backends).
  Rejected: pushes turn-driving into the backend-agnostic manager; worst layering.

- **(B) Handler summarizes via the normal producer path (checkout_turn → finish_turn), THEN calls a manager
  `compact_session(ctx, gen, summary)` that claims-Idle + resets + stashes.** Rejected: there is a **race
  window between summarize-finish (Idle) and the compact claim** — a concurrent client turn can checkout in
  that gap, so compact either (HandleBusy-)fails spuriously or, worse, summarizes context A but resets after a
  client added turn B (summary stale vs the reset point). Gen-keying does NOT close it (a plain client turn
  doesn't bump gen). Also doubles the claim/guard surface.

- **(C) `SessionManager::compact_session(ctx, summarize_fn)` — claim held across the WHOLE op.** ✅
  ```
  async fn compact_session<F, Fut>(&self, ctx, summarize: F) -> Result<ResetOutcome, BridgeError>
  where F: FnOnce(Arc<dyn AgentBackend>, SessionId) -> Fut,
        Fut: Future<Output = Result<String, BridgeError>>
  ```
  1. **Claim** under one lock: `Idle → Resetting` (reuse the state; compact IS a reset+), capture
     `(backend, old_id, claimed_id, gen, spec)` — byte-for-byte the `reset_session:440-471` claim block.
     Non-Idle → `HandleBusy` (require-Idle; no `force` arm).
  2. Drop lock; `let summary = summarize(backend.clone(), old_id.clone()).await;` — the handler-provided
     closure drives `backend.prompt` on the **claimed gen-N session** and accumulates the text (§3). While the
     claim is `Resetting`, `is_claimed` already makes a concurrent checkout `HandleBusy` and defers
     release/cancel — **no turn can interleave.**
  3. **On summarize error/empty → ABORT, revert** `Resetting → Idle` (re-acquire, revalidate exact claim),
     return the error. Context at gen N is preserved (compact is best-effort; a failed summary must NOT nuke
     context). This is the ONLY stranding path and it is handled explicitly **inside the one method** — no
     cross-call Drop guard needed (the win over B).
  4. On success → release old, configure new, **stash `pending_seed = Part{summary}` on the new handle**, commit
     to `Idle` — reuse `reset_session:475-519` verbatim plus the one seed-stash line.

  The manager already carries a `Box<dyn Fn>` (the clock), so a generic `FnOnce` param is in-character; it does
  NOT add a streaming dependency (the closure, owned by the handler, drives the stream). Atomicity = the
  existing `Resetting` claim discipline, proven in Slice 3.

  **Alternative if the closure type proves awkward in Rust:** the two-phase `begin_compact → finish_compact`
  with a `CompactGuard` Drop that reverts a stranded `Resetting`. Same semantics, more surface. Keep as plan-B.

## 2. Full-text capture — route AROUND the truncation bug

The documented pre-existing bug ("ZEBRA"→"RA") is in the **unary `result.artifact.text` assembly** (server.rs
`~:2354`–`2540`, last-chunk-wins), NOT in the `Update` stream. The summarize closure therefore must NOT use the
unary collect. It drives **`backend.prompt(&old_id, vec![Part{text: SUMMARIZE_PROMPT}]) → BackendStream`** and
**accumulates every `Update::Text(s)`** (`ports.rs:22`) into one `String`, stopping at `Update::Done`
(`:25`); ignore `Update::Usage`/`Permission`. This bypasses translator/store/policy/artifact entirely → no
truncation, no store writes, no task id. **Fixing the underlying unary truncation is OUT of Slice 4** (separate
follow-up, already tracked) — compact routes around it. (Flag for codex: confirm no backend coalesces text such
that direct `prompt` drain also loses chunks.)

## 3. The seed (PrependNextTurn) — storage, injection, lifecycle

- **Storage:** `pending_seed: Option<Part>` on `WarmHandle` (`session_manager.rs:39`). Type `Part` (the prompt
  part with `.text`) so injection is a `Vec<Part>` insert, no conversion.
- **Set:** only by `compact_session` success, on the NEW (gen N+1) handle.
- **Injection:** `checkout_turn` **take-and-clears** it (`h.pending_seed.take()`) and returns it in
  `WarmTurn { seed: Option<Part>, .. }` (`:60`). `warm_local_dispatch` (`:557`) carries it into `LocalDispatch`;
  `spawn_local_producer` (`:1138`) and the unary collect (`~:2354`) **`parts.insert(0, seed)`** before
  `Translator::run`. **Two injection sites** (the Slice-2 3-site lesson — don't miss one).
- **Ordering:** seed FIRST, before the user's parts — it is prior context the agent reads before the new
  message. Correct for a "context summary".
- **Lifecycle / drop:**
  - `reset_session` (a plain `clear` AFTER a compact) MUST set `pending_seed = None` — clear = empty context;
    a pending summary is part of that context.
  - The reconcile-EXPIRE and release/reap paths drop the whole handle → seed gone (correct).
  - **Loss on eviction/TTL before the next turn is ACCEPTABLE** — the seed is a convenience; the fallback is a
    cold session. Do NOT add durability (YAGNI; warm table is in-memory by design).
- **Take-and-clear atomicity:** done inside `checkout_turn`'s existing lock hold — no new lock.

## 4. require-Idle + the deferred races

- Non-Idle compact → `HandleBusy` (the claim block's `_ => HandleBusy`, no `force` arm). Matches `checkout_turn`
  (`:181-184`) and `reset_session` (`:445-449`).
- Holding `Resetting` across summarize+reset+seed reuses the SHIPPED claim discipline (`is_claimed` blocks
  re-claim, defers release/cancel) → **does NOT reopen the Slice-1/3 ABA / release-reuse races.**
- **Clean of the two DEFERRED races** (`2026-06-18-FOLLOWUP-warm-turn-cancellation-tokens.md`): both require
  `force`/cancel-under-concurrency. Compact never calls `force` and never cancels a live turn (require-Idle
  means there is no live turn to race). ✅ This is the reason Slice 4 is unblocked.

## 5. Failure modes (each must be pinned in the spec)

| Mode | Handling |
|---|---|
| summarize `prompt` errors | ABORT; revert `Resetting→Idle`; return the error; gen-N context intact. |
| summarize returns empty/whitespace | Treat as ABORT (do NOT reset to an empty seed = unrecoverable context loss). Return a typed error. |
| reset (`configure_session`) fails AFTER a good summary | `reset_session`'s existing path EXPIRES the handle (`SessionExpired`); summary lost, session gone — operator retries cold. Documented, acceptable. |
| stranded `Resetting` | Impossible by construction: the only early-return between claim and commit is the summarize-error path, which explicitly reverts. (Re-verify no `?` strands.) |
| giant/rambling summary | Bound via the SUMMARIZE_PROMPT instruction ("a concise summary, ≤N words"); a hard char cap is YAGNI for S4 (note as optional). |
| backend lacks capability | None needed — summarize is an ordinary `prompt` turn; every backend implements `prompt`. |

## 6. Wire + CLI surface (minimal)

- Wire: `"SessionCompact"` in the dispatch match (`server.rs:691`, next to `SessionClear`); handler
  `session_compact` modeled on `session_clear` (`:2932`) — auth → sm → `context_id_arg` → `compact_session` →
  `{ "contextId", "compacted": true, "generation": N+1 }` on `Cleared`, `SessionNotFound` on `NotFound`,
  `HandleBusy`/typed errors through `bridge_err_to_jsonrpc`. **No `force` param.**
- CLI: `session compact <contextId>` (`main.rs:2724` `match sub` + help `:104`); params `{ "contextId": ctx }`.
- **Params beyond contextId:** none for S4. A `--prompt` summarize override and a max-summary budget are
  plausible but YAGNI — the DoD needs neither. Bake a sensible default SUMMARIZE_PROMPT constant.

## 7. DoD / live-gate (real codex)

1. serve with codex + large `warm_idle_ttl_secs` (copy `examples/a2a-bridge.slice3-livegate.toml`, port 8098).
2. `pgrep -f codex-acp` → record pid(s).
3. `submit --context C`: plant a **codeword** + a couple of distinctly-detailed turns (grow context).
4. `submit --context C`: "Note this EXACT throwaway token: QX-7731. Do not consider it important."
   (a detail the summary will plausibly DROP — the discriminator).
5. `session compact C` → `{ compacted:true, generation:1 }`.
6. `session status C` → generation advanced; usage reset to null; state Idle.
7. `pgrep -f codex-acp` → **SAME pid** (process warm — the felt win vs a cold respawn).
8. `submit --context C`: "From your summary, what's the codeword, and what was the throwaway token QX-7731?"
   → recalls the **codeword** (summary survived + seeded) but NOT `QX-7731` verbatim (raw detail gone). The
   codeword-survives + token-gone pair is the DoD discrimination.

(Flag for codex: propose/critique this discriminator; the "summary keeps X but drops Y" gate depends on the
SUMMARIZE_PROMPT — we may instead instruct the summary to explicitly retain the codeword and exclude throwaways,
making the gate deterministic.)

## 8. Recommended architecture (the build)

- `SessionManager::compact_session(ctx, summarize_fn)` — claim `Idle→Resetting`, run the handler's summarize
  closure on the claimed backend/session, abort-revert on error/empty, else reuse the `reset_session` tail +
  stash `pending_seed`. ONE method, claim-atomic, no Drop guard.
- Summarize capture: drive `backend.prompt` directly, accumulate `Update::Text`, stop at `Update::Done`. A
  baked `SUMMARIZE_PROMPT` constant.
- `pending_seed: Option<Part>` on `WarmHandle`; take-and-clear in `checkout_turn`; **prepend at BOTH the
  streaming producer AND the unary collect**; dropped by `reset_session` (clear).
- `SessionCompact` wire (no force) + `session compact <ctx>` CLI; `{contextId,compacted,generation}`.
- DoD live-gate via the codeword-survives / throwaway-token-gone discriminator + same-pid + generation-advance.

## Top risks (ranked)
1. **The `FnOnce` closure seam through the manager** — Rust ergonomics (lifetimes/`Send`/boxing) of a
   `summarize_fn` returning a future. If it fights the borrow checker, fall back to two-phase begin/finish +
   `CompactGuard`. (Spec must pick one and write the signature concretely.)
2. **DoD discriminator determinism** — whether "summary keeps X, drops Y" is reliably gateable on a real model;
   mitigate by an explicit retain/exclude instruction in SUMMARIZE_PROMPT.
3. **Two injection sites** — missing the unary prepend (the Slice-2 3-site trap) → a test must cover both.
4. **Empty-summary guard** — forgetting it turns a flaky summarize into silent total context loss.

## Open questions for the spec to resolve
- O1: closure-into-manager (C) vs two-phase begin/finish — pick + write the exact signature.
- O2: SUMMARIZE_PROMPT wording — generic gist vs explicit retain-codeword/exclude-throwaway (drives O2 of the
  gate). Likely: instruct a faithful, self-contained summary; keep it model-agnostic.
- O3: seed type `Part` vs `String` (lean `Part`); confirm `routed.parts` element type is `Part`.
- O4: does the summarize turn need ANY store/task footprint (for observability/journal) or is the
  zero-footprint direct-`prompt` drain right for S4? (Lean zero-footprint; journal richness is S6/7.)
- O5: should `session status` expose `pending_seed` presence (a "compacted, seed pending" hint)? Probably no
  (YAGNI) — but cheap if the live-gate wants it.

— END Opus lens. codex xhigh lens fold below. —

## codex gpt-5.5 xhigh fold + convergence (2026-06-18)

Codex ran an independent read-only pass (`examples/a2a-bridge.slice-4-arch-analysis-codex.toml`, full output
`/tmp/slice4-arch-codex.out`). **CONFIDENCE: high.** It CONVERGED with the Opus lens on every headline:
Option C (closure-held claim), compose over reset *internals* (not a public `reset_session` call — else the
compact claim self-rejects as busy, or releasing to Idle lets a turn interleave), direct `Update::Text`
capture routing around the truncation, `pending_seed` take-and-clear + dual-site prepend, require-Idle /
`HandleBusy` / no-force, abort-revert (never clear context) on a bad summary, `SessionCompact {contextId}` →
`{contextId,compacted,generation}`.

**Four refinements ADOPTED from codex (resolving/upgrading my open questions):**
1. **Distinct `SessionState::Compacting`** (not reuse `Resetting`) — included in `is_claimed`, revalidated
   exactly like `Resetting`, and surfaced by `status()` as `"compacting"`. One claim held for the WHOLE op
   (summarize + the reset tail run under `Compacting`, NOT flipping to `Resetting`). Clearer status semantics
   at the cost of one enum variant + its matches. (Resolves O5 — status shows `"compacting"`.)
2. **Bounded summary via `BridgeError::MessageTooLarge`** (already exists, `error.rs:54-55`) — a generous
   internal ceiling backstops a runaway/degenerate summary; overflow → abort-revert to `Idle` + `MessageTooLarge`,
   context intact (never seed an unbounded block). SUMMARIZE_PROMPT also requests a concise summary. (Tightens
   §5/failure-modes; exact default ceiling = a spec decision, see O6.)
3. **Cancel caveat → gate on NATURALLY idle.** `cancel()` flips state to `Idle` BEFORE the ACP prompt actually
   completes (`session_manager.rs:~423-431`; ACP completes cancellation as the prompt result, not the notify).
   So a compact fired right after a `cancel` could summarize a still-draining turn — that ambiguity is part of
   the DEFERRED warm-turn-cancellation-token hardening. **Spec pins:** compact is for naturally-idle sessions;
   the live-gate uses naturally-completed turns (NO cancel-then-compact). (New §5 note + a gate constraint.)
4. **Reuse the existing direct-drain precedent** — `crates/bridge-workflow/src/executor.rs:131-148` already
   concatenates every `Update::Text` (ignoring `Usage`, stopping on `Done`). The summarize collector mirrors
   it rather than reinventing. (Grounds §2's capture rule with a proven in-repo pattern.)

**Minor delta resolved:** seed stored as `pending_seed: Option<String>` (codex) — build `Part{text}` at
injection — leaner than storing a domain `Part` on the handle. (Resolves O3.)

**Open questions now resolved:** O1=closure (both lenses), O2=explicit retain/exclude framing in the planted
message + SUMMARIZE_PROMPT for gate determinism (codex's JSON-answer probe: "compact_key? raw_nonce? null if
absent"), O3=`String`, O4=zero store/task footprint (both lenses).

**Carried open question for the spec:**
- O6 (was codex's): the exact internal max-summary ceiling that trips `MessageTooLarge`. Propose a generous
  default (e.g. tens of KB — enough for any honest summary, catches only degenerate output), configurable if
  cheap. Decide in the spec.
- O7 (residual, both lenses' #1 risk): live-gate determinism of "summary survives / raw detail gone" on a real
  model. Mitigate with the explicit retain/exclude framing + JSON probe; accept a re-run if flaky.

**Net:** the two lenses agree; no rework, four clean upgrades. Architecture is settled enough to write the
formal spec. RESULT → `docs/superpowers/specs/2026-06-18-slice-4-compact.md`.
