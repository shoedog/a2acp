# `thorough` review-depth tier — design

**Date:** 2026-06-13
**Status:** Approved (brainstorming) — ready for plan
**Predecessor:** `2026-06-13-review-codenav-adaptive-depth-design.md` (shipped the `light`/`standard` two-tier ladder, merged `main` `3468222`)

## Goal

Complete the `implement`-review depth ladder by adding a third tier, **`thorough`** — a draft→refine→synth double pass for large code changes — and make depth-sizing measure the *authored code/infra* change (excluding docs, tests, lockfiles, generated files, comments, and blank lines) rather than raw diff lines. Add a clean, deterministic depth-override seam exposed to the owner (config), caller (CLI), and constructor (programmatic).

## Context

The `implement` command (clone → warm `:rw` agent edits → host-commits → verify → **review-the-diff** → tweak loop → handoff) runs an adaptive-depth review after each commit attempt. Today:

- **light** (`implement-review-light`): 1 reviewer → light-synth, no prism slice. Auto-selected for tiny diffs (≤ `light_max_lines` AND ≤ `light_max_files`).
- **standard** (`implement-review`): 2 reviewers (codex + claude) in parallel → synth, with prism slice. The default for everything above light.

The spec/plan/design *review workflows* (run standalone via `run-workflow`) already use a **draft→refine→synth** topology — two reviewers each produce a draft, then refine their own draft, then a synth merges. `thorough` brings that same proven topology to the `implement`-review path for large diffs. Per the prior decision, **depth scales the number of passes, not per-reviewer rigor** — every reviewer does a thorough line-by-line read regardless of tier; thorough simply adds a second (refine) pass.

`thorough` ships **example-only** (in `examples/a2a-bridge.containerized.toml` + `.podman.toml`), exactly as `light` does — it is *not* added to the `init` scaffold and *not* embedded in the binary. A config that lacks the `-thorough` variant gracefully falls back to the standard workflow (with a warning), so `init`-scaffolded users are unaffected.

## Architecture

Two pure concerns live in `bin/a2a-bridge/src/review.rs` (already the pure verdict/sizing module); the impure orchestration stays in `main.rs::run_review_step`:

1. **Tier model + selection** — `Tier { Light, Standard, Thorough }`, the 3-way `select_tier`, the `Depth` resolver, and a `tier_workflow_suffix` helper that maps a tier to its workflow-id suffix.
2. **Diff sizing** — a small pure unified-diff parser, `parse_diff_for_depth`, backed by two pure classifiers (`counts_toward_depth` for file paths, `is_logical_line` for line content). This replaces `parse_numstat`.

The workflow topology (`implement-review-thorough`) and the new refine prompt are config/prompt-only. The depth-override seam threads a config default through the existing `Depth` type.

---

## Components

### 1. Tier model + selection (`review.rs`, pure)

```rust
pub enum Tier { Light, Standard, Thorough }
```

`select_tier` becomes three-way (params keep the existing signature order; new `thorough_min_*` appended):

```rust
pub fn select_tier(
    files: usize, lines: usize,
    light_max_lines: usize, light_max_files: usize,
    thorough_min_lines: usize, thorough_min_files: usize,
) -> Tier {
    if lines <= light_max_lines && files <= light_max_files {
        Tier::Light
    } else if lines >= thorough_min_lines || files >= thorough_min_files {
        Tier::Thorough
    } else {
        Tier::Standard
    }
}
```

- **light** iff `lines ≤ light_max_lines AND files ≤ light_max_files`
- **thorough** iff `lines ≥ thorough_min_lines OR files ≥ thorough_min_files`
- **standard** otherwise (the band between)

Light is checked first, then thorough, else standard. Config validation (§6) guarantees `thorough_min_* > light_max_*`, so the bands cannot overlap. `Depth::resolve` gains the two `thorough_min_*` params and forwards them to `select_tier`.

A new pure helper drives variant selection:

```rust
/// The workflow-id suffix for a tier: Light → "-light", Thorough → "-thorough", Standard → "" (base).
pub fn tier_workflow_suffix(tier: Tier) -> &'static str
```

### 2. Diff sizing (`review.rs`, pure) — replaces `parse_numstat`

`git diff --numstat` cannot see comments or blank lines *inside* a file, so sizing switches to parsing the actual `git diff` patch. One pure function:

```rust
/// PURE. Parse a unified-diff patch → (code_infra_files, lloc). Files excluded by `counts_toward_depth`
/// are skipped entirely; for surviving files, `+`/`-` content lines are counted only when `is_logical_line`.
/// A file counts toward `files` only if it contributed ≥1 logical changed line.
pub fn parse_diff_for_depth(patch: &str) -> (usize, usize)
```

Parser rules (standard unified diff):
- File boundary + path from `+++ b/<path>` when present and not `/dev/null`, else `--- a/<path>` (deletions). The `+++`/`---` paths are always clean single paths (not the numstat `{a => b}` rename form).
- A content line is `+...` (not `+++`) or `-...` (not `---`); `@@` headers and ` ` context lines are ignored.
- Rename detection is left **on** (default): a pure rename emits no hunks → contributes 0 lines (so renames are free, as with numstat).
- Binary files (`Binary files … differ`, no hunks) contribute 0.

#### 2a. `counts_toward_depth(path) -> bool` (pure)

Returns `false` (excluded from sizing) for:

- **Markdown/docs:** path ends `.md` or `.markdown`.
- **Tests:** a path component in `{tests, test, __tests__, __mocks__, testdata, fixtures}`, **or** a filename matching `*_test.*`, `*_tests.*`, `test_*`, `*.test.*`, `*.spec.*`, `*_spec.*` (covers Rust `tests/`, Go `_test.go`, Python `test_*`/`tests/`, JS/TS `*.test.*`/`*.spec.*`/`__tests__`).
- **Lockfiles:** filename in `{Cargo.lock, package-lock.json, yarn.lock, pnpm-lock.yaml, Gemfile.lock, poetry.lock, Pipfile.lock, composer.lock, go.sum}`, **or** any `*.lock`.
- **Generated:** ends `.min.js` / `.min.css`; matches `*.pb.go` / `*_pb2.py` / `*.generated.*` / `*_generated.*` / `*.g.dart`; ends `.snap`; **or** a path component in the always-build-output dirs `{node_modules, vendor, dist, target, .next}`.

Everything else counts — the residue is "code + infra files" (`.rs`, `.toml`, `.yaml`, Dockerfiles, CI configs, etc.).

**Bias rule:** when a path is *ambiguous*, it **counts**. A false-include nudges the tier up (more review = safe); a false-exclude nudges it down (less review = unsafe). This is why the generated-dir set is kept tight — `build` / `out` / `gen` are deliberately *not* excluded (too likely to be real source/feature names). All matching is ASCII-case-insensitive on `/`-separated components.

#### 2b. `is_logical_line(content) -> bool` (pure)

Given a `+`/`-` content line (the marker already stripped), returns `false` for:
- **Blank lines:** trimmed content is empty.
- **Comments:** trimmed content starts with `//`, `#`, `--`, `;`, `<!--`, or `/*`.

**Honest limits** (documented in code): this is a heuristic, not a per-language lexer.
- C-style block-comment *interior* lines (`* foo`) are **counted** on purpose — excluding a bare leading `*` would drop legitimate pointer-deref/assignment lines (`*ptr = x`).
- A comment marker *inside a string literal* is a rare false-drop.
- A C/C++ preprocessor directive (`#include`) is treated as a comment (minor undercount).

These biases are small and lean conservative; they are acceptable for *sizing* (they never change *what* gets reviewed — only which tier).

### 3. Workflow topology — `implement-review-thorough` (example-only)

Mirrors the spec/plan/design draft→refine→synth pattern; reuses the existing reviewer + synth prompts plus one new refine prompt:

```
codexdraft       (codex,  review-implement.md,         inputs = [])
claudedraft      (claude, review-implement.md,         inputs = [])
reviewer_codex   (codex,  review-implement-refine.md,  inputs = ["codexdraft"])
reviewer_claude  (claude, review-implement-refine.md,  inputs = ["claudedraft"])
synth            (claude, review-implement-synth.md,   inputs = ["reviewer_codex", "reviewer_claude"])
```

- Same 2-model diversity cap as standard (codex + claude). The two refine legs are the **terminal reviewer legs**; they keep the ids `reviewer_codex` / `reviewer_claude` so the synth contract and `reduce()`'s failed-reviewer accounting key on them. Drafts use the `*draft` suffix.
- The synth is `review-implement-synth.md` **unchanged** — same `VERDICT: APPROVE|REJECT` footer contract that `parse_verdict` already enforces.

### 4. Prompts

New `prompts/review-implement-refine.md` — a reviewer prompt (gets the same READ-ONLY + prism + git-archaeology + non-negotiable **line-by-line** clauses as `review-implement.md`). It takes the reviewer's own first-pass draft as input and instructs: re-read the diff line-by-line; re-verify each draft finding against the code (promote/demote/drop false positives); surface anything the first pass missed (the draft is a starting map, not a ceiling). It must **not** emit a `VERDICT` line (the synth owns the verdict). The prompt-contract test extends to assert the refine prompt has the line-by-line clause and no `VERDICT`.

`review-implement-synth.md` and `implement-review-light-synth.md` are unchanged.

### 5. Orchestration (`main.rs::run_review_step`)

Three edits in the block at `main.rs:1024–1070`:

- **Sizing call:** the git invocation drops `--numstat` and reads the full patch (still `--no-ext-diff --no-textconv`, still wrapped in `tokio::time::timeout(rcfg.slice_timeout, …)`; rename detection left on). The output goes to `parse_diff_for_depth`. A timeout/failure still degrades to `(usize::MAX, usize::MAX)` → standard.
- **Slice condition:** flips from `tier == Tier::Standard` to `tier != Tier::Light` — standard **and** thorough get the prism slice (large diffs benefit most). Slice presence follows the **tier**, not the resolved workflow, so a thorough run that falls back to the base workflow still carries the slice.
- **Variant selection:** the `light` and new `thorough` arms share one helper:

```rust
// Build "<base><suffix>"; if absent from wf_map, warn and fall back to the base standard workflow.
fn variant_or_fallback(wf_map, base: &WorkflowId, suffix: &str) -> WorkflowId
```

`Standard → base`, `Light → variant_or_fallback(.., "-light")`, `Thorough → variant_or_fallback(.., "-thorough")`.

### 6. Depth override seam (owner / caller / constructor)

A single deterministic precedence chain lets anyone override the heuristic up or down:

| Layer | Mechanism | Meaning |
|-------|-----------|---------|
| **caller** (CLI) | `--depth auto\|light\|standard\|thorough` | Per-invocation override. Absent → fall through to config. |
| **owner** (config) | `[review].default_depth = "auto"` (default) | The baseline `Depth` when no `--depth` is given. |
| **constructor** (programmatic) | `ImplementArgs.depth: Option<review::Depth>` | `None` → use config default; `Some(d)` → this call pins `d`. |

- **Precedence:** explicit per-call depth (`Some`) → else config `default_depth` → built-in `Auto`. Resolution happens at one point: `call_depth.unwrap_or(rcfg.default_depth)`.
- `parse_depth_flag` gains `"auto" → Depth::Auto` (so a caller can override a config default of, say, `thorough` back to auto). Its absence at the CLI maps to `None`, not `Auto`.
- `Depth::Forced(_)` is the deterministic override: `--depth standard` on a 500-LLOC diff runs standard (downgrade); `--depth thorough` on a 5-line diff runs thorough (upgrade). `default_depth = "thorough"` makes every `implement` run thorough by default.
- The **resolved** `Depth` is what gets persisted to the resume checkpoint (§8) — resume replays the exact resolved depth, independent of later config changes.

### 7. Config + validation (`config.rs`)

`ReviewToml` / `ReviewConfig` gain three fields:

```rust
thorough_min_lines: usize,   // default 150  (now LLOC-denominated, see note)
thorough_min_files: usize,   // default 6
default_depth: Depth,        // from string "auto"(default)|"light"|"standard"|"thorough"
```

`ReviewToml.default_depth` is a `String` (serde default `"auto"`); `to_config` parses it via the shared `parse_depth_flag` logic into a `review::Depth`, rejecting unknown values with `ConfigError::Registry`. Validation in `to_config` (all via `ConfigError::Registry`):
- `thorough_min_lines > 0` and `thorough_min_files > 0`;
- `thorough_min_lines > light_max_lines` and `thorough_min_files > light_max_files` (strictly ordered bands).

**Threshold is now LLOC-denominated.** The 150/6 defaults were originally chosen against physical diff lines; 150 *logical* lines ≈ ~180 physical (typical code is ~20% blank + comment). Keeping 150/6 means thorough trips on a slightly larger code change than before — intentional, and tunable via config.

### 8. Resume / checkpoint (`crates/bridge-core/src/implement_resume.rs`)

No struct change — `ImplementCheckpoint.forced_depth: Option<String>` already exists. Helpers extend:
- `parse_depth_flag`: add `"auto" → Auto` and `"thorough" → Forced(Thorough)`.
- `depth_from_checkpoint`: add `Some("thorough") → Forced(Thorough)`.
- `depth_to_forced_str`: add `Forced(Thorough) → Some("thorough")`.

`forced_depth = None` still means `Auto` (re-sizes each attempt). `--depth` on resume overrides and rewrites the checkpoint, as today.

### 9. `reduce()` failed-reviewer accounting (`review.rs`)

`reduce()` counts failed non-`synth` nodes as failed reviewer legs. With thorough's `*draft` + refine topology, a collapsed reviewer could emit two failures (draft + refine) and over-count. Fix: `reduce()` ignores failures from nodes whose id ends in `draft`, so only the terminal refine legs (`reviewer_codex`/`reviewer_claude`) count — a collapsed reviewer counts once. The plan must first confirm the executor's behavior when a node's input failed (emits a `NodeFinished{ok:false}` vs. skips) and choose the suffix filter accordingly; the `*draft` exclusion is correct under either (drafts never count; a skipped refine simply isn't reported).

---

## Data flow (one `implement` review attempt)

1. After commit + verify, `run_review_step` runs the bounded `git diff <base>..HEAD` patch and feeds it to `parse_diff_for_depth` → `(files, lloc)` over code/infra files only.
2. `Depth` (resolved from the override seam) → `resolve(files, lloc, light_max_lines, light_max_files, thorough_min_lines, thorough_min_files)` → `Tier`.
3. If `tier != Light`, prep the prism slice → write under `<clone>/.git/a2a-bridge/review-slices/slice-<task_id>-<attempt>.md`; degrade to sliceless on failure.
4. `variant_or_fallback` picks the workflow id by tier suffix; `build_review_input` assembles the reviewer input (task + base/head SHAs + optional slice-ref).
5. The workflow runs (bounded by `rcfg.timeout`); `drain_review` → `reduce()` → `parse_verdict` → `ReviewOutcome::Ran`.
6. The resolved `Depth` is persisted to the checkpoint for crash-exact resume.

## Error handling / degradation

- Patch read timeout/failure → standard tier (existing fail-safe).
- Slice unavailable / write failure → continue sliceless (existing).
- `-thorough` variant absent from the loaded config → warn + fall back to the base standard workflow, slice still per tier.
- A failed reviewer leg → surfaced in the hand-off suffix (`[N reviewer(s) failed]`); the synth still emits a verdict from the surviving leg(s).
- Verdict not parseable → `Verdict::Inconclusive` (never inferred Approve).

## Testing

**Unit (pure, in `review.rs` / `config.rs`):**
- `select_tier` three-way at every boundary (light/standard/thorough edges; thorough-by-files-only; thorough-by-lines-only).
- `tier_workflow_suffix` for all three tiers.
- `counts_toward_depth`: markdown, each test convention, each lockfile, each generated pattern, the always-build-output dirs, and the ambiguous-counts-in cases (`build/`, `gen/`, a plain `.rs`, a `.toml`).
- `is_logical_line`: blank, each comment prefix, a `*ptr` deref (counts), code lines.
- `parse_diff_for_depth`: a multi-file patch mixing code + md + test + lockfile + generated → only code/infra files and logical lines counted; a pure rename → 0; a deletion (path from `--- a/`); a binary file → 0; a comment-only file → 0 files.
- `parse_depth_flag` / `depth_from_checkpoint` / `depth_to_forced_str`: round-trip for `auto`/`light`/`standard`/`thorough`.
- Config validation: ordered-band rejection, `default_depth` parse + reject-unknown.
- Prompt-contract test: `review-implement-refine.md` has the line-by-line clause and emits no `VERDICT`.
- Podman parity test: `implement-review-thorough` + `[review]` thresholds mirrored structurally in `.podman.toml`.

**Live DoD gate (real codex + claude through the bridge):**
- A diff with ≥150 LLOC of code/infra auto-selects **thorough** → draft→refine→synth runs (4 reviewer nodes + synth, confirmed in logs) → slice present under `.git/` → APPROVE → `forced_depth` reflects the resolved depth in the checkpoint.
- `--depth thorough` on a small diff forces thorough (upgrade); `--depth standard` on a large diff forces standard (downgrade).
- A docs-only or test-file-only diff sizes to `(0,0)` → light, while the diff is still fully reviewed.

## Files touched

- `bin/a2a-bridge/src/review.rs` — `Tier::Thorough`, 3-way `select_tier`, `tier_workflow_suffix`, `parse_diff_for_depth` + `counts_toward_depth` + `is_logical_line` (replacing `parse_numstat`), `reduce()` draft-exclusion, `Depth::resolve` signature.
- `bin/a2a-bridge/src/main.rs` — sizing call (patch, not numstat), slice condition `!= Light`, `variant_or_fallback`, `parse_depth_flag` (`auto`/`thorough`), `depth_from_checkpoint`/`depth_to_forced_str`, the override-seam resolution + `Option<Depth>` on the args, usage strings, the example timeout bump.
- `bin/a2a-bridge/src/config.rs` — `thorough_min_lines`/`thorough_min_files`/`default_depth` + validation.
- `prompts/review-implement-refine.md` — **new** refine prompt.
- `examples/a2a-bridge.containerized.toml` + `examples/a2a-bridge.containerized.podman.toml` — `implement-review-thorough` workflow, `[review]` thresholds + `default_depth`, timeout 900→1800.

## Out of scope / deferred

- **Standalone `run-workflow` adaptive depth** (the "executor depth-gate" primitive) — thorough here is `implement`-review-only; the standalone review path remains un-tiered. Deferred (next-slice item #3).
- **Per-tier timeout** (`thorough_timeout_secs`) — kept a single `[review].timeout_secs`; the example default rises to 1800s and operators can raise further. Trivial to add later if 1800s proves tight.
- **Sub-file (language-aware) test/comment attribution** — sizing is per-file path + per-line heuristic; an inline `#[cfg(test)]` block in a code file still counts toward that file. Out of scope (would need a per-language lexer).
- **Configurable exclusion globs** — the `counts_toward_depth` heuristic is fixed (well-documented) rather than config-driven. YAGNI until a real target repo's convention is misclassified.
