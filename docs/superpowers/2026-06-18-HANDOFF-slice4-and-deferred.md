# Handoff — post-Slice-3: NEXT = Slice 4 (compact); deferred = warm-turn cancellation tokens

> Written 2026-06-18 at the end of the Slice-3 session (context near compaction). Single resume doc for the
> immediately-next work. The canonical roadmap status lives in `docs/superpowers/2026-06-17-orchestration-HANDOFF.md`
> (already updated: Slices 0–3 ✅, NEXT = Slice 4). Read that first for the whole picture; this doc is the
> focused next-step plan.

## State (2026-06-18)

- **Slices 0–3 SHIPPED + MERGED + PUSHED** to `main` (tip `89ace4f`; `main` == `origin/main`). MVP target = S0–S5.
- **Slice 3 (clear/reset)** = `SessionClear`/`reset_session` (new bridge `SessionId` per generation,
  DIVERGENCE-1; `Resetting` claim; `is_claimed` deferral; GENERATION-MONOTONICITY guard `gen==generation &&
  op==Some(op) && state==Running`; CLI `session clear [--force]`). Live-gated on real codex (codeword forgotten
  across clear; same warm process; gen 0→1; usage null). See `[[slice-3-clear-reset-shipped]]` memory +
  `docs/superpowers/specs/2026-06-18-slice-3-clear-reset.md`.

## Two tracks — RECOMMENDATION: do Slice 4 first

### ◀ NEXT (recommended): Slice 4 — Compact
`compact` = **summarize gen N → `reset_session` (to gen N+1) → seed the summary as the next turn's input
(`PrependNextTurn`)**. Composition over the shipped Slice-3 clear primitive (architecture OPEN-4: "compact =
composition"). **Require-Idle** (summarize an idle session, then reset) — so it does NOT use `force`, which
means **the deferred hardening below does NOT block it.** This is the last context-management slice before the
serve-CLI (S5) closes the MVP.

Scope sketch (spec it properly via the proven loop):
- A summarize step: prompt the warm agent (its OWN context) to produce a compact summary of the conversation
  so far → capture the summary text.
- `reset_session(ctx, ResetOpts{force:false})` (the Slice-3 primitive) → fresh generation.
- Seed: queue the summary as the FIRST input of the next turn. ACP has no system-message channel, so the seed
  rides as a `PrependNextTurn` content block (the architecture's settled approach — see the architecture doc's
  OPEN-3 Turn-Channel note; Slice 4 needs only the prepend, not the full Turn Channel which is S9).
- Surface: `SessionCompact {contextId}` wire (CamelCase, sibling to `SessionClear`) + CLI `session compact`.
- DoD/live-gate: a long-context summary survives `compact`; raw prior detail (outside the summary) is GONE;
  same process warm (pgrep unchanged); generation advances. Gate vs real codex.
- Key reuse: everything Slice 3 built (`reset_session`, the generation+op guard, `Resetting`). The NEW bit is
  the summarize-then-seed orchestration + the prepend-next-turn input path.

### Deferred (do when a `force`/cancel-under-concurrency feature needs it): "warm-turn cancellation tokens"
The whole-branch codex-xhigh review (run vs `main` AFTER the per-increment reviews — it caught what they
missed) found two **PRE-EXISTING** races (since Slice 0/1/2) that `force` surfaces. Full detail in the spec's
**"## Deferred hardening"** section. Summary:
1. **Cancel→next-turn op collision (FIX-12 is PARTIAL).** The guard now requires `op`, but `op` is
   **task-derived** at the server edge (`op-{taskId}`, `"task-1"` fallback when omitted —
   `server.rs:732/2321/3158`). A `SessionCancel` + same-context send with the SAME/OMITTED `taskId` reuses the
   op, so the old producer's late `finish_turn`/`record_usage` can still clobber the new same-generation turn.
   **Fix:** mint a UNIQUE per-checkout op nonce in `SessionManager` (independent of client `taskId`); the guard
   keys on it.
2. **`clear --force` vs producer start.** `checkout_turn` marks `Running`, but the streaming/unary handlers
   await `store.put` BEFORE spawning the producer (`server.rs:749/2340`). A concurrent `--force` claims +
   releases the old id in that gap; the original handler then prompts the released session, which ACP **lazy
   re-mints** (`acp_backend.rs:2052` → `translator.rs:133`) — resurrecting the force-cleared context.
   **Fix:** a per-turn ABORT token owned by the manager, cancelled under the reset claim during `force`; the
   producer/translator `select!`s on it BEFORE and WHILE entering `backend.prompt`.

**Recommended follow-up slice:** "warm-turn cancellation tokens" — manager-minted unique op + per-turn abort
token wired through the producer/translator. Closes BOTH + makes `force` truly abortive. **Sequence it before
any feature that relies on `force`/cancel under concurrency.** (Slice 4 does not.)

## How to resume (the proven loop — unchanged across Slices 0–3)

1. **Spec** (`docs/superpowers/specs/2026-06-18-slice-4-compact.md`) → **dual spec-review** (codex-xhigh via
   `run-workflow spec-review` + an Opus lens via the Agent tool) → fold to v2.
2. **Plan** (bite-sized TDD tasks) → **dual plan-review** → fold. (User likes iterating the codex plan-review
   to `ready-to-execute` — it found real bugs each round on Slice 3.)
3. **Implement per task:** codex gpt-5.5 **high** host implementor via
   `./target/release/a2a-bridge run-workflow slice0-impl --config examples/a2a-bridge.slice0-impl-codex.toml
   --input /tmp/<task>.md --session-cwd <repo>`. Codex writes test+impl together, leaves UNCOMMITTED. **The
   controller (you) verifies (`cargo test`/build/fmt/clippy) + commits** — codex hits the `_dyld_start` flake
   running freshly-built test binaries.
4. **Per-increment review:** codex-xhigh on `git show HEAD` via `run-workflow increment-review --config
   examples/a2a-bridge.slice-3-increment-review-codex.toml` (or a slice-4 copy). Serialize (don't start the
   next task's impl before the prior review reads HEAD).
5. **Gate:** `cargo test --workspace --no-run` (catch enum/sig exhaustiveness), then fmt + clippy + `cargo test
   --workspace` (CAPTURE THE REAL EXIT CODE — redirect to a file, do NOT pipe to `tail`, which masks cargo's
   exit; this bit us in Slice 2).
6. **Live-gate:** real serve via a slice-4 livegate config (copy `examples/a2a-bridge.slice3-livegate.toml`,
   codex, large TTL). `serve --config <f>` (port 8097), `submit --url http://127.0.0.1:8097 --context C
   --agent codex --input <f>`, `session compact C --url ...`, `session status C`. Prove the DoD.
7. **WHOLE-BRANCH REVIEW before merge (new this session, HIGH VALUE):** codex-xhigh on `git diff main...HEAD`
   via `run-workflow branch-review --config examples/a2a-bridge.slice-3-branch-review-codex.toml` — it catches
   CROSS-TASK races the per-increment reviews (commits-in-isolation) miss. Run it; fold/iterate to `merge`.
8. **Merge:** FF to `main` + push (user authorizes the merge/push explicitly — CONFIRM before the outward
   step); update `2026-06-17-orchestration-HANDOFF.md` + memory.

## Reusable scaffolding (tracked on `main`)
- Implementor: `examples/a2a-bridge.slice0-impl-codex.toml` + `prompts/slice0-impl-node.md` (codex gpt-5.5/high).
- Reviews: `prompts/slice-3-{spec,plan,increment,branch}-review.md` + matching `examples/*.toml` (codex-xhigh,
  read-only). Copy for slice-4 (bump the `[server].addr` port to avoid collision; slice-3 used 8105/8106/8107/8108).
- Live-gate: `examples/a2a-bridge.slice3-livegate.toml`.
- Opus lens: dispatch a `general-purpose` Agent (inherits Opus 4.8) with the review prompt + ground-truth
  anchors; weight it toward architecture (it caught the `TurnEvent` un-drop in Slice 2, the reset seam in S3).

## Carried gotchas
- Wire methods are CamelCase, slash-free (`SessionClear`/`SessionCompact`, NOT `session/clear`).
- `serve --config` reads the config; `submit`/`session` take `--url` (default :8080; livegate uses :8097).
- `_dyld_start` flake: codex can't run freshly-built test binaries → controller runs them. `--no-run` is safe.
- Pre-existing unary `result.artifact.text` last-chunk truncation (`PONG`→`ONG`); relates to C1 typed result.
- codex implements at `high`; reviews at `xhigh`; Opus architects/controls. Each increment codex-xhigh reviewed;
  the WHOLE-BRANCH xhigh review before merge is the new high-value step.
