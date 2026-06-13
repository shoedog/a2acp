# Richer Review — Code-Nav Tooling + Adaptive Depth — Design

**Date:** 2026-06-13
**Status:** Draft (for review)
**Goal:** Make the bridge's self-hosted reviews (code / spec / plan / implement-review) *find more, faster* by
(1) giving every reviewer a consistent, read-only **code-navigation toolset** — prism whole-repo nav, prism
diff-slicing into reference files, and git archaeology — and (2) scaling **how many** review passes run to the
size of the artifact (**adaptive depth**), without ever diluting the per-reviewer rigor.

**Owner principle (load-bearing):** every reviewer ALWAYS does a thorough, human-style line-by-line reading and
analysis of the artifact, *regardless of size*. Adaptive depth scales the **number** of independent
readings / passes / pre-steps — never how carefully any single reviewer reads.

---

## Context & current state

The bridge already self-hosts four reviews, each a workflow on the static workflow-DAG executor (W1, ADR-0009):

| Review | Where it runs | Today's structure | Artifact |
|---|---|---|---|
| **code-review** | `run-workflow` / serve (workflow) | correctness (codex) + architecture (claude) → synth | a diff |
| **spec-review** | `run-workflow` (workflow) | rigordraft + soundnessdraft → rigor + soundness (refine) → synth | a spec doc |
| **plan-review** | `run-workflow` (workflow) | execdraft + coveragedraft → exec + coverage (refine) → synth | a plan doc |
| **implement-review** | the `implement` loop, via `review.rs` glue running the `implement-review` workflow | 2 diverse reviewers (codex+claude) → synth `VERDICT:` (ADR-0022, Topology B) | the committed diff in the clone |

**Reviewers run read-only.** Reviewers have shifted host-side (read-only ⇒ low risk): `spec-review` and
`plan-review` configs run agents host-side **with prism wired** (`[[agents.mcp]] prism` → `prism-mcp --repo
{cwd}`), trading egress-lockdown for prism + full nav depth + no container overhead (documented in
`examples/a2a-bridge.slicing-*.toml`). `code-review` keeps one sandboxed agent. **prism is the owner's own
`~/code/slicing` project** (`prism-mcp` nav server + `prism` slicing CLI, shared `libprism` + CPG cache).

**Two real gaps today:**
1. **Inconsistent code-nav.** The code/spec/plan review prompts already carry a "prism (if available)" block
   and a read-only-tools contract. **`implement-review` does NOT** — `prompts/review-implement.md` and the
   inline prompt built in `review.rs` say only "read-only git/grep/read," with no prism guidance — even though
   it is the most-run review (every `implement`). And the **prism slicing CLI is unused in *every* review**
   (only the `nav_*` MCP is mentioned).
2. **No adaptive depth anywhere.** Every review pays its full node count regardless of a 1-line vs 1,000-line
   change. Most `implement` reviews are small diffs, so this is pure waste on the common path.

**Building blocks that already exist (reuse, don't reinvent):**
- `review.rs` PURE glue: `build_review_input(task, base_sha, head_sha)`, `parse_verdict(synth)`,
  `reduce(events)`, `outcome_suffix`. The diff is NOT inlined — reviewers run `git diff` in the clone.
- The per-agent MCP delivery (ADR-0028): `prism-mcp` wired via `[[agents.mcp]]`, `{cwd}`-substituted per
  session; `--session-cwd` re-points it at the target repo (session-cwd increment, ADR-0014).
- The prism slicing CLI: `prism --repo <root> --diff <unified-diff-file> --format review` → a defect-focused
  review slice (also `text`/`json`/`mermaid`).
- The verify pre-step pattern (ADR-0020): a bridge-deterministic host-side step around the agent work.

---

## Design

### 1. The uniform reviewer contract

Every reviewer prompt (all four reviews) carries two clauses, made uniform:

- **Line-by-line directive (non-negotiable):** *"Do a thorough, human-style line-by-line reading and analysis
  of the artifact, regardless of its size. Depth selection never licenses a shallower read."*
- **Read-only code-nav contract** (superset of today's): read/list/grep + `git diff`/`log`/`show` **plus the
  additions in §2**. No writes, builds, installs, test runs, or network beyond read-only git/search. If a tool
  is denied, continue.

This is prompt-only and applies identically across code/spec/plan/implement reviewers.

### 2. Code-nav toolset (uniform across all four reviews)

**(a) prism whole-repo nav (MCP) — make it consistent.** Add the existing prism block (the one
code/spec/plan prompts already have) to **`prompts/review-implement.md` AND the inline prompt in `review.rs`**
— the implement-review gap. The block names the tools (`mcp__prism__nav_*` for claude/codex, bare `nav_*` for
kiro) and frames them as *verify-the-artifact-against-real-code*, not wandering. Where reviews run on a config
that does not wire prism, the operator wires `[[agents.mcp]] prism` (host-side; the `slicing-*` configs are the
reference). **prism availability is config-driven and the prompt block is conditional ("if available"), so a
config without prism degrades to git/grep — no hard dependency.**

**(b) prism diff-slice → reference file** — for the two **diff-based** reviews (code-review +
implement-review). A host-side **slice prep step** (see §4) runs `prism --repo <repo> --diff <diff> --format
review` and writes the slice to a **reference file** at `<repo>/.a2a-review/slice-<runid>.md`. The reviewer
prompt is given the file PATH and read instructions — **the slice is never inlined** (slices can be large; a
reference file keeps the prompt small and lets the reviewer read on demand). spec/plan reviews are doc-based
(no diff) → no slice; they rely on prism nav + git to fact-check the doc's code anchors.

**(c) git archaeology** — expand the read-only contract in all four prompts to explicitly permit
`git blame`, `git log -L <range>:<file>` (line history), and `git log -S/-G` (pickaxe: when a string/regex was
introduced). Near-zero cost (read-only git is already allowed); strong for the correctness/regression lens
("why is this here / when did this change").

**Out of scope (this slice): LSP.** rust-analyzer measured **~106 s cold index + ~2.9 GB RAM** on this repo —
too heavy to spin up cold per review (and the clone-based implement-review is always cold). LSP is a separate
deferred capability (see Deferred).

### 3. Adaptive depth — Approach 3

Depth has three tiers; each tier scales the **number of passes**, never per-reviewer rigor:

| Tier | Reviewer lenses | Refine pass | Slice prep | When (default) |
|---|---|---|---|---|
| **light** | 1 thorough lens | no | no | tiny artifact |
| **standard** | 2 diverse lenses + synth | no | yes (diff reviews) | default |
| **thorough** | 2 diverse lenses + refine + synth | yes | yes (diff reviews) | large artifact |

**implement-review (auto-adaptive, this slice).** `review.rs` computes `git diff --stat` on the committed diff
→ a **pure tier-selection function** `select_tier(files_changed, lines_changed, cfg) -> Tier` (the coverage
keystone) → review.rs (1) runs the slice prep and (2) **selects which review-workflow variant to run** —
`implement-review-light` / `implement-review` / `implement-review-thorough` (config-defined workflows that
share the reviewer prompts, differing only in node count). An optional `--depth light|standard|thorough` on
`implement` **overrides** the auto choice. *Mechanism note:* this is bridge-driven **variant selection**, not a
change to the executor — the static DAG runs unchanged; review.rs just picks the graph.

**Standalone code/spec/plan reviews (this slice).** They get the full §2 toolset at their **existing fixed
workflow structure**. **Auto/forced depth-variation for standalone reviews is DEFERRED** — varying DAG node
count by size for the `run-workflow` path cleanly needs the executor "depth-gate" primitive (see Deferred); a
`--depth` flag without it would be inert, so it is not added here. (Standalone reviews are run deliberately by
the operator on whole docs/diffs that are rarely "tiny," so auto-sizing pays far less there than on the
per-implement loop.)

### 4. The slice-prep seam

A small host-side step, invoked by `review.rs` before the implement-review workflow runs (and reusable by a
future code-review prep):

1. Materialize the unified diff for `base_sha..head_sha` in the clone (`git diff`).
2. Run `prism --repo <clone> --diff <diff-file> --format review` (the `prism` slicing binary; path from
   `[review].slice_cmd`, default `~/code/slicing/target/release/prism`).
3. Write stdout to `<clone>/.a2a-review/slice-<runid>.md`; pass the path into `build_review_input` so the
   prompt references it.
4. **Degrade gracefully:** if `prism` is absent / errors / times out, log a warning and run the review WITHOUT
   the slice (the reviewers still have prism nav + git). The slice is an accelerant, never a hard dependency.
   Bounded by a timeout; the `.a2a-review/` artifact is gitignored-by-convention and lives in the throwaway
   clone.

### 5. Config additions

A `[review]` section gains (all optional, with defaults):
- `slice_cmd` — path to the prism slicing binary (default `~/code/slicing/target/release/prism`).
- `light_max_changed_lines` / `thorough_min_changed_lines` (+ optional `*_files` companions) — the tier
  thresholds `select_tier` reads (sensible defaults, e.g. light ≤ ~15 lines, thorough ≥ ~400 lines).
- The three implement-review variant workflow ids are conventional (`implement-review{,-light,-thorough}`);
  absence of a variant falls back to `implement-review` (standard) so a minimal config still works.

### 6. Components & files

- **Prompts:** `review-implement.md` (+prism block, +git archaeology, +line-by-line); `review-correctness.md`
  / `review-architecture.md` (+git archaeology, +line-by-line, +slice-ref read instructions); `spec-review-*`
  / `plan-review-*` (+git archaeology, +line-by-line). prism block already present in the latter.
- **`bin/a2a-bridge/src/review.rs`:** add prism guidance to the inline reviewer prompt; `select_tier` (pure);
  thread the chosen variant id + slice path through; `git diff --stat` parse (pure).
- **New seam `slice` (host-side prep):** materialize diff → run `prism` → ref file; injectable command runner
  for unit-testing without prism; pure `slice_ref_path(runid)`.
- **`bin/a2a-bridge/src/config.rs`:** `[review]` `slice_cmd` + thresholds.
- **Example configs:** add the `implement-review-{light,thorough}` workflow variants + `[review]` thresholds to
  `examples/a2a-bridge.containerized.toml`; document prism wiring for the canonical review configs.

### 7. Error handling

- prism slice missing/failing/timeout → warn + proceed sliceless (degrade-per-tool, mirrors verify's
  reported-not-gating posture).
- Unknown `--depth` value → usage error before any agent runs.
- A missing implement-review variant id → fall back to standard (`implement-review`) + warn (never fail the
  loop on a config gap).
- The line-by-line + git-archaeology prompt clauses are asserted by a prompt-contract test so a prompt edit
  can't silently drop them.

## Testing

- **Pure / unit:** `select_tier` across the light/standard/thorough boundaries + the `--depth` override;
  `git diff --stat` parse (files + lines); `slice_ref_path`; the slice-prep seam with an injected fake runner
  (writes a ref file; degrades to none on runner error); prompt-contract assertions (every reviewer prompt
  carries the line-by-line + git-archaeology clauses; implement-review now carries the prism block).
- **Live (DoD):** an `implement` run on a tiny diff selects **light** (1 lens, no slice) and on a large diff
  selects **thorough** (2 lenses + refine + slice ref file present and referenced); a code-review with prism
  wired shows the reviewer using `mcp__prism__nav_*` and reading the slice ref file; degrade path (prism absent
  → review still completes). Dogfood the design itself through the bridge's spec/plan reviews.
- **Floors:** keep ci.yml coverage floors; new pure code (`select_tier`, diff-stat parse, slice_ref_path) is
  high-coverage by construction.

## Deferred (captured, own specs)

1. **Executor "depth-gate" primitive (Approach 1)** — first-class auto/forced adaptive depth for the
   **standalone** `run-workflow` reviews: nodes carry a `tier`; a sizing-driven gate runs only nodes at-or-below
   the selected depth, so one graph per review serves all depths (vs. this slice's bridge-driven variant
   selection for implement-review only). Needs analysis of conditional execution in the streaming DAG +
   threading depth through the run context. **Reason deferred:** standalone auto-sizing has low payoff and the
   executor change carries real risk; revisit if standalone depth control is wanted.
2. **L3 — warm multi-language LSP capability** — a persistent, reused LSP index (rust-analyzer/gopls/pyright/
   tsserver via an LSP-over-MCP shim) serving **both** review (thorough tier) and **implement** across the
   owner's stable repo set. Open questions that gate it: per-language cold-index costs (only rust-analyzer
   measured: ~106 s / ~2.9 GB) and **whether a clone can reuse a warm index** (the crux of whether L3 helps the
   clone-based implementor at all). **Leaning L3**; its own spec.

## Scope guard (non-goals)

- **No** LSP in this slice; **no** executor changes (adaptive depth = bridge-driven variant selection for
  implement-review only); **no** standalone-review depth variation; **no** new "providers"/security-scan
  (semgrep/CodeQL) augmentation; **no** change to the verdict/tweak-loop contract.

## Open questions (for review)

1. **Variant proliferation vs. a 4th tier.** Three implement-review variants (light/standard/thorough) — is
   `light` worth a distinct workflow, or should `light` just be "standard minus the slice" (2 lenses always,
   slice gated)? (Trade: fewer variants vs. true single-lens speed on trivial diffs.)
2. **Slice format.** `--format review` (defect-targeted) vs. `text`/`json` — which serves the reviewer best as
   a reference file? Pick one default; make it `[review].slice_format` if it varies by review.
3. **Should standalone code-review get the slice always-on now** (it's diff-based, the prep seam exists) even
   though its *depth* variation is deferred? Cheap win or scope creep?
