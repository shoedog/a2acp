You are an independent SOFTWARE ARCHITECT doing a pre-spec ARCHITECTURE ANALYSIS for Slice 4 (Compact) of the
a2a-bridge orchestration roadmap. This is NOT a code review of existing code and NOT an implementation plan —
it is a design-space analysis that will be folded against a parallel Opus analysis into the Slice 4 spec.
session-cwd = the bridge repo. READ-ONLY: read files, grep, `git log/show/diff`; do NOT edit/build/test.
Ground EVERY claim in the actual code (cite `file:line`). Where the code contradicts the brief, say so — the
code is ground truth.

The Slice 4 scope + grounding pointers + the specific questions to analyze are below.

{{input}}

YOUR JOB — analyze the design space and RECOMMEND, grounded in code:

1. **Composition soundness.** Does `compact = summarize(gen N) → reset_session(N+1) → seed-as-next-turn-prepend`
   actually compose over the SHIPPED Slice-3 `reset_session` (`session_manager.rs:434`) and the turn machinery?
   Cite what it reuses and what is genuinely new. Flag any place the composition is NOT clean.

2. **THE CENTRAL DECISION — where does the summarize turn run, and how is its full text captured ATOMICALLY?**
   Weigh at least these options (add your own if better), with code-grounded pros/cons and a RECOMMENDATION:
   - (A) The `SessionManager` runs the summarize turn itself via a backend prompt-collect call (manager gains a
     streaming/collect responsibility it does not have today).
   - (B) The handler runs the summarize via the normal producer/translator path (checkout_turn → finish_turn),
     THEN calls a manager `compact_session(ctx, gen, summary)` that claims-Idle + resets + stashes the seed —
     analyze the race window between summarize-finish and the compact claim, and whether gen/op/usage keying
     closes it.
   - (C) A manager `compact_session(ctx, opts, summarize_fn)` that CLAIMS the handle (Idle→a claim state,
     mirroring `Resetting`), invokes a handler-provided closure to run the summarize turn + collect text on the
     CLAIMED backend/session, then resets + stashes the seed under the same claim (atomic; reuses the
     translator; manager stays backend-agnostic like its existing `now: Box<dyn Fn>` seam).
   Which gives atomicity (no concurrent turn interleaving summarize/reset) with the least new surface?

3. **Full-text capture / the truncation trap.** The roadmap documents a PRE-EXISTING unary
   `result.artifact.text` last-chunk truncation (multi-chunk reply → last chunk only, e.g. "ZEBRA"→"RA").
   Read the translator text path (`crates/bridge-core/src/translator.rs`, the `EventKind`/text events) and the
   unary collect (`server.rs` ~2292+) and the producer text path. Does the truncation affect the summarize
   capture? Specify EXACTLY how compact must collect the full summary text (accumulate which event(s)) to avoid
   it. Is fixing the underlying truncation in-scope or should compact route around it?

4. **The seed (PrependNextTurn) mechanism.** `prompt_request` (`acp_backend.rs:455`) builds the turn from a
   `Vec` of parts → `ContentBlock::Text`. Analyze: (a) WHERE the seed is stored (a `pending_seed` on
   `WarmHandle`, `session_manager.rs:39`?); (b) HOW it is injected on the next turn (returned by `checkout_turn`
   in `WarmTurn` then prepended by the producer/unary path to `routed` parts? or elsewhere?); (c) its TYPE
   (String / one MessagePart / `Vec<ContentBlock>`); (d) its LIFECYCLE — must a `clear`/`reset_session` DROP a
   pending seed (clear = empty context)? must `checkout_turn` atomically take-and-clear it? what if the next
   turn never arrives (eviction/TTL) — is losing the seed acceptable? (e) Does the seed ride the FIRST content
   block (before the user's parts) and is that the right ordering for a "context summary"?

5. **require-Idle + concurrency.** Slice 4 is require-Idle (NO `force` — that path has the deferred warm-turn-
   cancellation-token races, see the brief). Confirm compact must reject on `Running` (which error —
   `HandleBusy`?). Does holding a claim across summarize+reset+seed avoid reopening any Slice-1/3 ABA/
   release-reuse race? Does compact interact at all with the two DEFERRED races, or is it cleanly clear of them?

6. **Failure modes (enumerate + specify handling).** summarize prompt errors or returns empty; reset fails
   AFTER a successful summarize (is the summary lost? is that OK?); the claim is stranded if an early `?`
   returns; the agent produces a useless/giant summary; backend lacks the capability. What must the spec pin so
   no path strands a claim or silently loses context?

7. **Wire + CLI surface.** `SessionCompact {contextId}` (CamelCase, sibling to `SessionClear`
   `server.rs:2932` + dispatch `:691`) + CLI `session compact` (`main.rs:2724`). Any params beyond contextId
   (e.g. a summarize-prompt override, a max-summary budget)? Keep it minimal per YAGNI, but call out anything
   the DoD needs.

8. **DoD / live-gate realizability.** Is "a long-context summary survives compact; raw prior detail (outside
   the summary) is GONE; same process warm (pgrep/pid unchanged); generation advances" provable on real codex
   via `submit`/`session compact`/`session status`? Propose the precise live-gate script shape.

OUTPUT: a structured analysis — for each of 1–8, a code-grounded finding + a recommendation. Then:
- **RECOMMENDED ARCHITECTURE** (the option you'd build, in 5–10 bullet points: the manager method shape, the
  seed storage+injection, the text-capture rule, the claim discipline, the wire/CLI).
- **TOP RISKS** (ranked) + **OPEN QUESTIONS** the spec must resolve.
- End with: `ARCH-ANALYSIS CONFIDENCE: high | medium | low` + one line on the single biggest unknown.
