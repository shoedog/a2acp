# Richer Review — Code-Nav Tooling + Adaptive Depth — Design

**Date:** 2026-06-13
**Status:** Draft (revised after two codex gpt-5.5/xhigh spec-reviews)
**Goal:** Make the bridge's self-hosted reviews (code / spec / plan / implement-review) *find more, faster* by
(1) giving every reviewer a consistent, read-only **code-navigation toolset** — prism whole-repo nav, prism
diff-slicing into reference files, and git archaeology — and (2) scaling **how many** review passes run to the
size of the artifact (**adaptive depth**), without ever diluting per-reviewer rigor.

**Owner principle (load-bearing):** every reviewer ALWAYS does a thorough, human-style line-by-line reading and
analysis of the artifact, *regardless of size*. Adaptive depth scales the **number** of independent
readings / pre-steps — never how carefully any single reviewer reads.

---

## Context & current state

The bridge self-hosts four reviews, each a workflow on the static workflow-DAG executor (W1, ADR-0009):

| Review | Where it runs | Today's structure | Artifact |
|---|---|---|---|
| **code-review** | `run-workflow` / serve | correctness (codex) + architecture (claude) → synth | a diff |
| **spec-review** | `run-workflow` | rigordraft + soundnessdraft → rigor + soundness (refine) → synth | a spec |
| **plan-review** | `run-workflow` | execdraft + coveragedraft → exec + coverage (refine) → synth | a plan |
| **implement-review** | the `implement` loop, via **`main.rs::run_review_step`** (`main.rs:920`), which runs the configured `[review].workflow` (default `implement-review`) on the executor and reads the **terminal node's** output | 2 diverse reviewers (codex+claude) → synth `VERDICT:` (ADR-0022) | the committed diff in the clone |

**Ground-truth constraints (verified):**
- `review.rs` is PURE (`build_review_input`, `parse_verdict`, `reduce`, `outcome_suffix`); the IMPURE run is
  `main.rs::run_review_step`, which loads exactly the configured `[review].workflow`.
- The executor requires **exactly one terminal node and returns its output**; `parse_verdict` reads it.
- Reviewer prompts (`review-implement.md:18`) say the reviewer must **NOT** emit `VERDICT`; only **synth** does.
- The workflow template renderer (`template.rs:19`) leaves **unknown placeholders verbatim** — so a synth
  prompt with `{{reviewer_claude}}` fed only one reviewer would emit that token literally.
- The implement loop runs **`git reset --hard && git clean -fdq`** before each verify/review attempt
  (`tweak.rs:190`); `implement --resume` refuses a **dirty** clone (`implement_resume.rs:156`,
  `implement.rs:183`). ⇒ **untracked files in the worktree are destroyed and/or block resume.**
- Registry `validate` rejects an ACP agent whose `cmd` is not in `allowed_cmds` (`registry.rs:160`).

**Reviewers have shifted host-side** (read-only ⇒ low risk): `spec-review`/`plan-review` configs run agents
host-side **with prism wired** (`[[agents.mcp]] prism` → `prism-mcp --repo {cwd}`). **prism is the owner's own
`~/code/slicing` project**: `prism-mcp` (nav server) + `prism` (slicing CLI: `prism --repo <root> --diff
<unified-diff> --format review`), sharing a CPG cache keyed by repo path.

**Gaps:** (1) `implement-review` has **no prism guidance** and the slicing CLI is **unused in every review**;
(2) **no adaptive depth** — every review pays its full node count regardless of diff size.

---

## Design

### 1. The uniform reviewer contract (prompt-only)

Every **reviewer** prompt (NOT synth prompts) carries two clauses:
- **Line-by-line (non-negotiable):** *"Do a thorough, human-style line-by-line reading and analysis of the
  artifact, regardless of its size. Depth selection never licenses a shallower read."*
- **Read-only code-nav contract:** read/list/grep + `git diff`/`log`/`show` + the §2 additions; no
  writes/builds/installs/test-runs/network beyond read-only git/search; if a tool is denied, continue.

**Exact prompt files covered** (a prompt-contract test asserts both clauses in each, and asserts the prism
block in `review-implement.md`): `review-implement.md`, `review-correctness.md`, `review-architecture.md`,
`spec-review-rigor.md`, `spec-review-rigor-refine.md`, `spec-review-soundness.md`,
`spec-review-soundness-refine.md`, `plan-review-exec.md`, `plan-review-exec-refine.md`,
`plan-review-coverage.md`, `plan-review-coverage-refine.md`. **Excluded** (synthesizers): `review-synth.md`,
`review-implement-synth.md`, `implement-review-light-synth.md` (new, §3), `spec-review-synth.md`,
`plan-review-synth.md`.

### 2. Code-nav toolset

**(a) prism whole-repo nav (MCP) — consistent + available for implement-review.** Add the prism block the
other three prompts have to **`review-implement.md`**. Per owner decision, the **implement-review reviewers run
HOST-SIDE with prism** (matching the reviewer host-side shift). Concretely, in the implement config
(`containerized.toml` + its podman twin): the existing **`codex` / `claude` reviewer agents drop their
`[agents.sandbox]`** (host-side) and gain **`[[agents.mcp]] prism`** (`prism-mcp --repo {cwd}`, `{cwd}` = the
clone). Read-only is preserved per agent: **codex** reviewers add `-c sandbox_mode="read-only"` (a hard
guarantee even without a container); **claude** reviewers are read-only by prompt. **`allowed_cmds` gains
`codex-acp` and `claude-agent-acp`** (registry rejects un-allowlisted ACP cmds); `docker` stays for the still-
containerized `impl` agent. *Cost note:* prism's CPG cache is keyed by repo path → a fresh clone is a cold CPG
build per implement; prism is light (CPG, not type inference) and degrades gracefully (a stable-origin
optimization is out of scope).

**(b) prism diff-slice → reference file — implement-review ONLY.** A host-side **slice-prep step** (§4) runs
`prism --repo <clone> --diff <diff> --format review` and writes the slice to
**`<clone>/.git/a2a-bridge/review-slices/slice-<runid>.md`** — inside `.git/` (the same `.git/a2a-bridge` area
the resume checkpoint uses, ADR-0026), so it **survives `reset --hard`/`clean -fdq`, never makes the worktree
dirty, never blocks resume, and is never visible to the write-capable fix turn**. The reviewer prompt is given
the PATH + read instructions — **never inlined**. **code-review slicing is DEFERRED** (it runs via
`run-workflow`, needing a separate prep seam) — code-review still gets prism nav + git this slice. spec/plan
reviews are doc-based → no slice.

**(c) git archaeology.** Expand the reviewer prompts' read-only contract to permit `git blame`,
`git log -L <range>:<file>`, and `git log -S/-G` (pickaxe).

**Out of scope: LSP** (rust-analyzer measured ~106 s cold / ~2.9 GB — see Deferred).

### 3. Adaptive depth — implement-review only, TWO tiers (Approach 3)

`thorough` is **deferred** (it needs a *refine* pass — new prompts + a draft→refine→synth DAG that does not
exist for implement-review; see Deferred). This slice ships **light** and **standard**. The **synth node is
always the single terminal and the only node that emits `VERDICT`**; reviewer nodes never emit one.

| Tier | Reviewer lenses | Slice prep | Terminal |
|---|---|---|---|
| **light** | **1** thorough lens | no | a dedicated `implement-review-light-synth` (single `{{reviewer}}` input) |
| **standard** (default) | 2 diverse lenses | yes | the existing 2-input synth |

*Light is "1 reviewer + synth," not a terminal reviewer* (reviewer prompts forbid `VERDICT`; the executor
returns the terminal's output). It uses a **dedicated `implement-review-light-synth.md`** prompt taking a
single `{{reviewer}}` input — NOT the 2-input `review-implement-synth.md` (whose `{{reviewer_claude}}` would
render verbatim). Precedent for 1-reviewer+synth: `slicing-implement-fast.toml:176`.

**Selection (`select_tier`, pure — the coverage keystone).** `run_review_step` computes
`git diff --numstat base..head` → `(files_changed, lines_changed = Σ(added+deleted)` over non-binary rows;
binary/rename rows count toward `files_changed` only). **`select_tier`: light iff
`lines_changed ≤ light_max_lines` AND `files_changed ≤ light_max_files`; else standard.** A `--depth
light|standard` flag on `implement` **overrides** auto (`--depth thorough` → a clear "not yet supported"
usage error while thorough is deferred). **If `git diff --numstat` or its parse fails → warn + standard tier**
(safe fallback; post-commit loop code never hard-fails here).

**Variant resolution.** `[review].workflow` (default `implement-review`) is the **standard** id; light resolves
to `<workflow>-light`. **A missing `-light` variant falls back to the standard workflow + warns** (never fails
the loop). The **slice decision follows the TIER**, independent of which workflow runs (light ⇒ no slice even
on fallback).

**Depth × tweak loop / resume.** The loop reviews **every** attempt and the committed diff changes as tweaks
amend, so **auto depth is RECOMPUTED each attempt**. A **forced** `--depth` applies to **all** attempts and is
**stored in the resume checkpoint** (a new optional field; `#[serde(default)]` ⇒ pre-existing checkpoints read
as `auto`). `implement --resume <id>` uses the stored depth; an explicit `--depth` on the resume command
**overrides and rewrites** the checkpoint.

### 4. The slice-prep seam (host-side, impure — owned by `main.rs`)

Invoked by `run_review_step` **after tier selection, only when the tier wants a slice** (standard), **before**
the workflow runs:
1. Materialize the unified diff for `base_sha..head_sha` to a temp file.
2. Run `prism --repo <clone> --diff <diff-file> --format review` (`[review].slice_cmd`, default
   `~/code/slicing/target/release/prism`, `~` expanded), bounded by `[review].slice_timeout_secs`.
3. Write stdout (truncated at `[review].slice_max_bytes`, keeping head+tail) to
   `<clone>/.git/a2a-bridge/review-slices/slice-<runid>.md`; pass the path into `build_review_input`.
4. **Degrade:** prism absent / nonzero / timeout → warn + review WITHOUT the slice (reviewers still have prism
   nav + git). The slice is an accelerant, never a hard dependency. Under `.git/` ⇒ no worktree/cleanup
   interaction; reaped with the clone.

**Placement:** the subprocess runner is an injectable seam (small new module / `run_review_step` helper) so
tests drive a fake runner with no prism. PURE pieces — `select_tier`, the `numstat` parse, `slice_ref_path` —
live in `review.rs` (mirroring verify.rs's pure/impure split).

### 5. Config additions (`[review]`, optional, defaulted)

- `slice_cmd` (default `~/code/slicing/target/release/prism`), `slice_timeout_secs` (e.g. 60),
  `slice_max_bytes` (truncation cap).
- `light_max_lines`, `light_max_files` — the single `select_tier` boundary (defaults e.g. ≤ 15 lines & ≤ 2
  files). Validated `> 0` at config load. (No thorough thresholds — thorough is deferred.)
- Variant id is conventional (`<workflow>-light`) — derived from `[review].workflow`; no new id fields.

### 6. Components & files

- **Prompts:** the 11 reviewer prompts (§1) gain the line-by-line + git-archaeology clauses;
  `review-implement.md` also gains the prism block + slice-ref read instruction. New
  **`implement-review-light-synth.md`** (single-input verdict synth).
- **`review.rs` (pure):** `select_tier`, `parse_numstat`, `slice_ref_path`; `build_review_input` optionally
  references the slice file.
- **`main.rs::run_review_step` (impure):** diff-stat → tier (or `--depth`) → slice-prep (if standard) →
  resolve+run the variant → existing `reduce`/`parse_verdict`.
- **New seam:** the slice subprocess runner (injectable).
- **`config.rs`:** `[review]` fields; `implement` arg parse gains `--depth` (light|standard).
- **`implement_resume.rs`:** checkpoint stores forced-depth (`#[serde(default)]`).
- **Example configs:** `containerized.toml` (+ podman twin) — host-side `codex`/`claude` reviewers + prism +
  `allowed_cmds` + the `implement-review-light` workflow + `[review]` thresholds.

### 7. Error handling

- prism slice missing/failing/timeout → warn + sliceless (reported-not-gating).
- `git diff --numstat` failure → warn + standard tier.
- Unknown/`thorough` `--depth` → usage error before any agent runs.
- Missing `-light` variant → fall back to standard + warn (tier's slice decision still honored).
- `light_max_*` ≤ 0 → `ConfigInvalid` at load.
- A prompt-contract test asserts the line-by-line + git-archaeology clauses (and the implement-review prism
  block) so an edit can't silently drop them.

## Testing

- **Pure / unit:** `select_tier` across the light/standard boundary + `--depth` override + the
  numstat-failure→standard fallback; `parse_numstat` (added+deleted sum; binary/rename → files only);
  `slice_ref_path` (under `.git/a2a-bridge/review-slices/`); the slice-prep seam with an injected fake runner
  (writes ref file; degrades on error/timeout); prompt-contract assertions over the exact 11 reviewer files +
  synth exclusion; variant-id resolution + fallback; checkpoint round-trips forced depth (+ `serde(default)`
  for old checkpoints); `light_max_* ≤ 0` rejected.
- **Live (DoD):** a tiny-diff `implement` selects **light** (1 lens, no slice, light-synth verdict); a normal
  diff selects **standard** (2 lenses + slice ref present under `.git/` and referenced). **Observability:** the
  reviewers' prism use + slice read are verified in the agent **session transcript / `docker logs` /
  `agent_stderr`** (the bridge's drained child stderr) showing `mcp__prism__nav_*` calls — named artifacts, not
  hand-waving. Degrade path (prism absent → review still completes + verdicts). `implement --resume` after a
  crash mid-review does not refuse the clone (slice is under `.git/`). Dogfood this design through the bridge's
  own spec/plan reviews.
- **Floors:** keep ci.yml coverage floors; new pure code is high-coverage by construction.

## Deferred (captured, own specs)

1. **`thorough` tier (implement-review refine pass)** — a draft→refine→synth shape for implement-review (new
   refine prompts + node wiring) for large diffs; deferred because it's a new review pattern and large
   implement diffs are uncommon.
2. **code-review diff-slice** via `run-workflow` — needs a standalone slice-prep seam (input plumbing +
   reference-file injection outside the implement loop).
3. **Executor "depth-gate" primitive (Approach 1)** — first-class auto/forced depth for the **standalone**
   `run-workflow` reviews (tiered nodes + a sizing gate), avoiding variant proliferation; needs conditional
   execution in the streaming DAG.
4. **L3 — warm multi-language LSP capability** (rust-analyzer/gopls/pyright/tsserver via an LSP-over-MCP shim)
   for both review (a future thorough tier) and **implement** across the owner's stable repos. Gated on
   per-language cold-index costs (rust-analyzer: ~106 s / ~2.9 GB) and **whether a clone reuses a warm index**.
   **Leaning L3**; its own spec.

## Scope guard (non-goals)

- **No** LSP; **no** executor changes (adaptive depth = bridge-driven variant selection, implement-review
  ONLY, two tiers); **no** `thorough` tier; **no** standalone-review depth; **no** code-review slice; **no**
  semgrep/CodeQL augmentation; **no** change to the verdict/tweak-loop contract (synth stays the sole verdict
  terminal).

## Resolved decisions (were open)

- Slice format: **`--format review`** (defect-targeted).
- Light's synth: a **dedicated `implement-review-light-synth.md`** (single input) — not a tolerant rewrite of
  the 2-input synth (the renderer leaves `{{reviewer_claude}}` verbatim).
- Slice location: **`<clone>/.git/a2a-bridge/review-slices/`** (survives reset/clean; no worktree/resume
  interaction).
