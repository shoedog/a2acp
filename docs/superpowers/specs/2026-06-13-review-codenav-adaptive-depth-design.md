# Richer Review — Code-Nav Tooling + Adaptive Depth — Design

**Date:** 2026-06-13
**Status:** Draft (revised after a codex gpt-5.5/xhigh spec-review)
**Goal:** Make the bridge's self-hosted reviews (code / spec / plan / implement-review) *find more, faster* by
(1) giving every reviewer a consistent, read-only **code-navigation toolset** — prism whole-repo nav, prism
diff-slicing into reference files, and git archaeology — and (2) scaling **how many** review passes run to the
size of the artifact (**adaptive depth**), without ever diluting per-reviewer rigor.

**Owner principle (load-bearing):** every reviewer ALWAYS does a thorough, human-style line-by-line reading and
analysis of the artifact, *regardless of size*. Adaptive depth scales the **number** of independent
readings / passes / pre-steps — never how carefully any single reviewer reads.

---

## Context & current state

The bridge self-hosts four reviews, each a workflow on the static workflow-DAG executor (W1, ADR-0009):

| Review | Where it runs | Today's structure | Artifact |
|---|---|---|---|
| **code-review** | `run-workflow` / serve | correctness (codex) + architecture (claude) → synth | a diff |
| **spec-review** | `run-workflow` | rigordraft + soundnessdraft → rigor + soundness (refine) → synth | a spec doc |
| **plan-review** | `run-workflow` | execdraft + coveragedraft → exec + coverage (refine) → synth | a plan doc |
| **implement-review** | the `implement` loop, orchestrated by **`main.rs::run_review_step`** (`main.rs:920`), which runs the configured `[review].workflow` (default `implement-review`); `review.rs` holds only the PURE glue | 2 diverse reviewers (codex+claude) → synth `VERDICT:` (ADR-0022) | the committed diff in the clone |

**Module split (ground truth — corrected from the first draft):** `review.rs` is PURE
(`build_review_input(task, base_sha, head_sha)`, `parse_verdict(synth)`, `reduce(events)`, `outcome_suffix`).
The IMPURE workflow run is `main.rs::run_review_step`; it loads exactly the configured `[review].workflow`,
builds one input string, runs the workflow on the executor, and reads the **terminal node's** output. The
executor requires **exactly one terminal node and returns that terminal's output**; `parse_verdict` reads it.
Reviewer prompts (`prompts/review-implement.md:18`) explicitly say the reviewer must **NOT** emit `VERDICT` —
only the **synth** node does. These three facts constrain the depth design (below).

**Reviewers have shifted host-side** (read-only ⇒ low risk): `spec-review`/`plan-review` configs run agents
host-side **with prism wired** (`[[agents.mcp]] prism` → `prism-mcp --repo {cwd}`), trading egress-lockdown for
prism + full nav + no container overhead. **prism is the owner's own `~/code/slicing` project**: `prism-mcp`
(nav server) + `prism` (slicing CLI: `prism --repo <root> --diff <unified-diff> --format review`), sharing
`libprism` + a CPG cache keyed by repo path.

**Gaps today:** (1) `implement-review` has **no prism guidance** (prompt + the input built in `run_review_step`
say only git/grep/read), and the prism **slicing CLI is unused in every review**; (2) **no adaptive depth** —
every review pays its full node count regardless of a 1-line vs 1,000-line change.

---

## Design

### 1. The uniform reviewer contract (prompt-only)

Every **reviewer** prompt (NOT synth prompts) carries two clauses, made uniform:
- **Line-by-line (non-negotiable):** *"Do a thorough, human-style line-by-line reading and analysis of the
  artifact, regardless of its size. Depth selection never licenses a shallower read."*
- **Read-only code-nav contract** (superset of today's): read/list/grep + `git diff`/`log`/`show` + the §2
  additions. No writes/builds/installs/test-runs/network beyond read-only git/search; if a tool is denied,
  continue.

**Exact prompt files covered** (the prompt-contract test asserts both clauses in each):
`review-implement.md`, `review-correctness.md`, `review-architecture.md`, `spec-review-rigor.md`,
`spec-review-rigor-refine.md`, `spec-review-soundness.md`, `spec-review-soundness-refine.md`,
`plan-review-exec.md`, `plan-review-exec-refine.md`, `plan-review-coverage.md`,
`plan-review-coverage-refine.md`. **Excluded** (synthesizers, not reviewers): `review-synth.md`,
`review-implement-synth.md`, `spec-review-synth.md`, `plan-review-synth.md`.

### 2. Code-nav toolset

**(a) prism whole-repo nav (MCP) — consistent + actually available for implement-review.** Add the prism block
the other three prompts already have to **`review-implement.md`** (the gap). Per owner decision, the
**implement-review reviewers run HOST-SIDE with prism wired** (no sandbox; matches the reviewer host-side
shift): the implement config gains host-side review agents with `[[agents.mcp]] prism` pointed at the **clone**
(`--session-cwd`/`{cwd}` = the clone path). Read-only is preserved per agent: **codex reviewers use codex's own
`sandbox_mode="read-only"`** (a hard guarantee even without a container); **claude reviewers are read-only by
prompt** (no tool flags). *Cost note:* prism's CPG cache is keyed by repo path, so a fresh clone is a cold CPG
build per implement; prism is light (CPG, not type inference) and degrades gracefully, but this is a known
per-clone cost (an optimization — pointing nav at the stable origin — is out of scope).

**(b) prism diff-slice → reference file — implement-review ONLY (this slice).** A host-side **slice-prep step**
(see §4) runs `prism --repo <clone> --diff <diff> --format review` and writes the slice to a **reference file**
`<clone>/.a2a-review/slice-<runid>.md`; the reviewer prompt is given the PATH + read instructions — **never
inlined** (slices can be large). **code-review slicing is DEFERRED** (it runs via `run-workflow`, needing a
separate prep seam; bundled with the deferred standalone work) — code-review still gets prism nav + git this
slice. spec/plan reviews are doc-based → no slice.

**(c) git archaeology.** Expand the read-only contract in the reviewer prompts (the §1 list) to explicitly
permit `git blame`, `git log -L <range>:<file>`, and `git log -S/-G` (pickaxe) — "why is this here / when
introduced."

**Out of scope: LSP** (rust-analyzer measured ~106 s cold / ~2.9 GB — see Deferred).

### 3. Adaptive depth — implement-review only (Approach 3)

Three tiers; each scales the **number of passes**, never per-reviewer rigor. **The synth node is ALWAYS the
single terminal and is the ONLY node that emits `VERDICT`** (preserving the existing contract); reviewer nodes
never emit a verdict at any tier.

| Tier | Reviewer lenses | Refine | Slice prep | Terminal |
|---|---|---|---|---|
| **light** | **1** thorough lens | no | no | synth (1 reviewer input) |
| **standard** (default) | 2 diverse lenses | no | yes | synth (2 inputs) |
| **thorough** | 2 diverse lenses | yes | yes | synth (2 inputs) |

*Light is "1 reviewer + synth," not a terminal reviewer* — because reviewer prompts forbid `VERDICT` and the
executor returns the terminal's output. Precedent: `examples/a2a-bridge.slicing-implement-fast.toml:177` (a
1-reviewer+synth fast config). The light variant's synth accepts a single reviewer input.

**Selection (`select_tier`, pure — the coverage keystone).** `main.rs::run_review_step` computes
`git diff --numstat base..head` on the committed diff → `(files_changed, lines_changed)` where
`lines_changed = Σ(added + deleted)` over non-binary files (binary/rename rows contribute to `files_changed`
only; exact parse edge-cases are plan-level). Then:
`select_tier`: **light** iff `lines_changed ≤ light_max_lines` **AND** `files_changed ≤ light_max_files`;
**thorough** iff `lines_changed ≥ thorough_min_lines` **OR** `files_changed ≥ thorough_min_files`; else
**standard**. A `--depth light|standard|thorough` flag on `implement` **overrides** the auto choice.

**Variant resolution.** `[review].workflow` (default `implement-review`) is the **standard** id; light/thorough
resolve to `<workflow>-light` / `<workflow>-thorough` by convention. **A missing variant falls back to the
standard workflow + warns** (never fails the loop). The **slice decision follows the TIER, independent of which
workflow actually runs** — i.e., a tier that says "no slice" skips prep even if its variant fell back to
standard, and vice-versa.

**Depth × the tweak loop / resume (`tweak.rs:197`, `implement_resume.rs:33`).** The loop reviews **every**
attempt; the committed diff changes as tweaks amend, so **auto depth is RECOMPUTED each attempt**. A **forced**
`--depth` applies to **all** attempts and is **stored in the resume checkpoint** alongside the existing
edit/fix workflow ids + attempt counter, so `--resume` preserves it (auto stays auto on resume = recompute).

### 4. The slice-prep seam (host-side, impure — owned by `main.rs`)

Invoked by `run_review_step` **after tier selection, before the workflow runs** (so light skips it entirely):
1. Materialize the unified diff for `base_sha..head_sha` in the clone to a temp file.
2. Run `prism --repo <clone> --diff <diff-file> --format review` (binary path from `[review].slice_cmd`,
   default `~/code/slicing/target/release/prism`, with `~` expanded), **bounded by `[review].slice_timeout_secs`**.
3. Write stdout (truncated at a max size, keeping head+tail) to `<clone>/.a2a-review/slice-<runid>.md`; pass
   the path into `build_review_input` so the prompt references it.
4. **Degrade gracefully:** prism absent / nonzero exit / timeout → warn + run the review WITHOUT the slice
   (reviewers still have prism nav + git). The slice is an accelerant, never a hard dependency. The
   `.a2a-review/` artifact lives in the throwaway clone (no cleanup needed; clone is reaped).

**Placement:** the subprocess runner is an injectable seam (a small new module or a `run_review_step` helper)
so tests drive a fake runner with no prism. The PURE pieces — `select_tier`, the `numstat` parse,
`slice_ref_path(runid)` — live in `review.rs` (mirroring verify.rs's pure/impure split).

### 5. Config additions (`[review]`, all optional, defaulted)

- `slice_cmd` — prism slicing binary path (default `~/code/slicing/target/release/prism`).
- `slice_timeout_secs` (default e.g. 60), `slice_max_bytes` (truncation cap).
- `light_max_lines` / `light_max_files` / `thorough_min_lines` / `thorough_min_files` — `select_tier`
  thresholds (sensible defaults, e.g. light ≤ 15 lines & ≤ 2 files; thorough ≥ 400 lines or ≥ 10 files).
- Variant ids are conventional (`<workflow>{,-light,-thorough}`); no new id fields — derived from
  `[review].workflow`.

### 6. Components & files

- **Prompts:** the 11 reviewer prompts in §1 gain the line-by-line + git-archaeology clauses;
  `review-implement.md` additionally gains the prism block + the slice-ref read instruction. Synth prompts
  untouched (except the light variant's synth handling a single input — a new `review-implement-synth` variant
  or a tolerant existing one).
- **`bin/a2a-bridge/src/review.rs` (pure):** `select_tier`, `parse_numstat`, `slice_ref_path`; extend
  `build_review_input` to optionally reference the slice file.
- **`bin/a2a-bridge/src/main.rs` (`run_review_step`, impure):** compute diff-stat → tier (or `--depth`
  override) → run slice-prep (if tier wants it) → resolve+run the variant workflow → existing `reduce` /
  `parse_verdict`.
- **New seam:** the slice subprocess runner (injectable).
- **`config.rs`:** `[review]` fields above; `implement` arg parse gains `--depth`.
- **`implement_resume.rs`:** checkpoint stores the forced-depth (if any).
- **Example configs:** `examples/a2a-bridge.containerized.toml` (+ its podman twin) gain host-side
  implement-review reviewers with prism + the `implement-review-{light,thorough}` variants + `[review]`
  thresholds.

### 7. Error handling

- prism slice missing/failing/timeout → warn + proceed sliceless (reported-not-gating, mirrors verify).
- Unknown `--depth` value → usage error before any agent runs.
- Missing variant workflow id → fall back to standard + warn (tier's slice decision still honored).
- The line-by-line + git-archaeology clauses (and the implement-review prism block) are asserted by a
  prompt-contract test so an edit can't silently drop them.

## Testing

- **Pure / unit:** `select_tier` across the light/standard/thorough boundaries + `--depth` override +
  AND/OR threshold semantics; `parse_numstat` (added+deleted sum; binary/rename rows → files only);
  `slice_ref_path`; the slice-prep seam with an injected fake runner (writes ref file; degrades to none on
  error/timeout); prompt-contract assertions over the exact 11 reviewer files (and synth-exclusion);
  variant-id resolution + fallback; checkpoint round-trips the forced depth.
- **Live (DoD):** a tiny-diff `implement` selects **light** (1 lens, no slice, synth verdict) and a large-diff
  one selects **thorough** (2 lenses + refine + slice ref present and referenced); the host-side implement
  reviewers demonstrably call `mcp__prism__nav_*` (real prism use, not just degradation) and read the slice
  ref file; degrade path (prism absent → review still completes + verdicts). Dogfood this design through the
  bridge's own spec/plan reviews.
- **Floors:** keep ci.yml coverage floors; new pure code is high-coverage by construction.

## Deferred (captured, own specs)

1. **code-review diff-slice** via the `run-workflow` path — needs a standalone slice-prep seam (input plumbing
   + reference-file injection outside the implement loop). Bundled with #2.
2. **Executor "depth-gate" primitive (Approach 1)** — first-class auto/forced adaptive depth for the
   **standalone** `run-workflow` reviews (nodes carry a `tier`; a sizing-driven gate runs only nodes at-or-below
   the depth). Avoids variant proliferation but needs conditional execution in the streaming DAG. Deferred:
   standalone auto-sizing has low payoff and the executor change carries real risk.
3. **L3 — warm multi-language LSP capability** (rust-analyzer/gopls/pyright/tsserver via an LSP-over-MCP shim),
   serving both review (thorough tier) and **implement** across the owner's stable repos. Gated on per-language
   cold-index costs (only rust-analyzer measured: ~106 s / ~2.9 GB) and **whether a clone can reuse a warm
   index** (the crux of whether L3 helps the clone-based implementor). **Leaning L3**; its own spec.

## Scope guard (non-goals)

- **No** LSP; **no** executor changes (adaptive depth = bridge-driven variant selection for implement-review
  ONLY); **no** standalone-review depth variation; **no** code-review slice this slice; **no** semgrep/CodeQL
  augmentation; **no** change to the verdict/tweak-loop contract (synth stays the sole verdict terminal).

## Open questions (for review)

1. **Slice format:** `--format review` (defect-targeted) vs `text`/`json` as the reference-file content — pick
   the default; make it `[review].slice_format` only if it needs to vary.
2. **Light's synth:** a dedicated `implement-review-light-synth` prompt (single input) vs making the existing
   synth tolerant of 1 input — pick during planning (precedent: `slicing-implement-fast.toml`).
