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

Light is checked first, then thorough, else standard. Config validation (§7) guarantees `thorough_min_* > light_max_*`, so the bands cannot overlap. Tier resolution from a `Depth` + an `Option<(files, lines)>` sizing is shown in §5 (`Forced` forces; `Auto + Some` → `select_tier`; `Auto + None` → `Standard`).

A new pure helper drives variant selection:

```rust
/// The workflow-id suffix for a tier: Light → "-light", Thorough → "-thorough", Standard → "" (base).
pub fn tier_workflow_suffix(tier: Tier) -> &'static str
```

### 2. Diff sizing (`review.rs`, pure) — replaces `parse_numstat`

`git diff --numstat` cannot see comments or blank lines *inside* a file, so sizing switches to parsing the actual `git diff` patch. One pure function:

```rust
/// PURE. Parse a unified-diff patch → (code_infra_files, lloc). Files excluded by `counts_toward_depth`
/// are skipped; for surviving files, added/removed lines are counted only when `is_logical_line`.
/// A file counts toward `files` only if it contributed ≥1 logical changed line.
pub fn parse_diff_for_depth(patch: &str) -> (usize, usize)
```

**The parser is HUNK-STATEFUL** — a naive "line starts with `+++`/`---` is a header, else content" rule is *wrong*: a removed content line whose text is `---` (YAML doc separator) renders as `----`, an added `+++` renders as `++++`, and an SQL/Lua `--` comment-or-decrement renders as `---…`; classifying those by prefix corrupts both the path detection and the line count. The parser tracks two phases per file:

- **Header phase** (after a `diff --git` line, before the first `@@`): recognize the file's paths from the `--- a/<old>` and `+++ b/<new>` header lines (and `rename from`/`rename to`, `copy from`/`copy to`). `/dev/null` on a side means add (old=∅) or delete (new=∅).
- **Hunk phase** (entered by `@@ -l,s +l,s @@`, which declares the added/removed line budget): classify each body line by its **leading byte** — `+`=added, `-`=removed, ` `=context (ignored). Stop consuming the hunk when the declared budget is exhausted or a `diff --git`/next `@@` appears. Lines `\ No newline at end of file` and `@@` headers are ignored.

**Path classification is side-aware** (handles rename/copy-with-edits): a file is counted if **either** its old or new path passes `counts_toward_depth`. Within an included file, an **added** (`+`) logical line is counted only if the **new** path is included, and a **removed** (`-`) logical line only if the **old** path is included. This prevents two bugs: (a) a code file renamed *into* `docs/` with edits still counts its deleted code; (b) a docs/test file renamed *into* code doesn't count its old-side removals as code churn.

**Path quoting:** the git diff command (§5) runs with `-c core.quotePath=false` so non-ASCII paths aren't octal-escaped; header paths wrapped in `"…"` (paths with spaces/tabs/quotes) are unquoted before classification. A pathological path that still can't be cleanly unquoted **fails open to "counts"** — safe under the bias rule (over-review, never under-review). Rename detection is left **on** (a pure rename emits no hunks → 0 lines); binary files (`Binary files … differ`, no hunks) contribute 0.

#### 2a. `counts_toward_depth(path) -> bool` (pure)

Returns `false` (excluded from sizing) for:

- **Markdown/docs:** path ends `.md` or `.markdown`.
- **Tests:** a path component in `{tests, test, __tests__, __mocks__, testdata, fixtures}`, **or** a filename matching `*_test.*`, `*_tests.*`, `test_*`, `*.test.*`, `*.spec.*`, `*_spec.*` (covers Rust `tests/`, Go `_test.go`, Python `test_*`/`tests/`, JS/TS `*.test.*`/`*.spec.*`/`__tests__`).
- **Lockfiles:** filename in `{Cargo.lock, package-lock.json, yarn.lock, pnpm-lock.yaml, Gemfile.lock, poetry.lock, Pipfile.lock, composer.lock, go.sum}`, **or** any `*.lock`.
- **Generated:** ends `.min.js` / `.min.css`; matches `*.pb.go` / `*_pb2.py` / `*.generated.*` / `*_generated.*` / `*.g.dart`; ends `.snap`; **or** a path component in the always-build-output dirs `{node_modules, vendor, dist, target, .next}`.

Everything else counts — the residue is "code + infra files" (`.rs`, `.toml`, `.yaml`, Dockerfiles, CI configs, etc.).

**Bias rule:** when a path is *ambiguous*, it **counts**. A false-include nudges the tier up (more review = safe); a false-exclude nudges it down (less review = unsafe). This is why the generated-dir set is kept tight — `build` / `out` / `gen` are deliberately *not* excluded (too likely to be real source/feature names). All matching is ASCII-case-insensitive on `/`-separated components.

#### 2b. `is_logical_line(path, content) -> bool` (pure, path-aware)

A blanket `#`-is-comment rule would be **wrong for this codebase** — `#[derive(...)]` / `#[cfg(...)]` / `#![...]` attributes start with `#` and are pervasive Rust *code*; a shebang and a Dockerfile `# syntax=` directive likewise. So `is_logical_line` is **path-aware**: it picks comment markers from the file's extension. Given a changed line (marker stripped), returns `false` for blank lines, else for lines whose trimmed content starts with a marker for that path's language:

| Path / extension | Line-comment / block-open markers |
|------------------|-----------------------------------|
| `.rs` | `//` only (**not** `#` — `#[…]` attributes are code) |
| `.toml`, `.yaml`, `.yml`, `.sh`, `.bash`, `.py`, `.rb`, `Dockerfile`, `.cfg`, `.ini`, `.conf` | `#` |
| `.c`, `.h`, `.cpp`, `.hpp`, `.go`, `.js`, `.jsx`, `.ts`, `.tsx`, `.java`, `.kt`, `.swift`, `.rs`-adjacent C-likes | `//`, `/*` |
| `.sql`, `.lua`, `.hs` | `--` |
| `.html`, `.xml`, `.vue`, `.svelte` | `<!--` |
| unknown / no extension | blank lines only (no comment stripping) |

**Honest limits** (documented in code; it's a heuristic, not a per-language lexer, and it only affects *sizing*, never *what* is reviewed):
- C-style block-comment *interior* lines (`* foo`) are **counted** — excluding a bare leading `*` would drop legitimate `*ptr = x` derefs.
- A trailing comment after code (`x = 1; // n`) correctly counts (there is code on the line); a comment marker *inside a string literal* is a rare false-drop.
- A one-line `/* … */ code` is treated as a comment (rare; minor undercount).

The metric is therefore "non-blank, non-leading-comment changed lines" — an approximate LLOC. All residual biases lean conservative *toward counting* for the dominant cases, consistent with the §2a bias rule.

### 3. Workflow topology — `implement-review-thorough` (example-only)

Mirrors the spec/plan/design draft→refine→synth pattern; reuses the existing reviewer + synth prompts plus one new refine prompt:

```
reviewer_codex_draft   (codex,  review-implement.md,         inputs = [])
reviewer_claude_draft  (claude, review-implement.md,         inputs = [])
reviewer_codex         (codex,  review-implement-refine.md,  inputs = ["reviewer_codex_draft"])
reviewer_claude        (claude, review-implement-refine.md,  inputs = ["reviewer_claude_draft"])
synth                  (claude, review-implement-synth.md,   inputs = ["reviewer_codex", "reviewer_claude"])
```

- Same 2-model diversity cap as standard (codex + claude). The two refine legs are the **terminal reviewer legs** feeding synth; they keep the ids `reviewer_codex` / `reviewer_claude`. Each draft node shares its refine's stem with a `_draft` suffix (`reviewer_codex_draft`), so `reduce()` can map a draft and its refine to **one logical leg** (§9) — this is why the draft ids are *not* `codexdraft`.
- The synth is `review-implement-synth.md` **unchanged** — same `VERDICT: APPROVE|REJECT` footer contract that `parse_verdict` already enforces.

### 4. Prompts

New `prompts/review-implement-refine.md` — a reviewer prompt (gets the same READ-ONLY + prism + git-archaeology + non-negotiable **line-by-line** clauses as `review-implement.md`). It takes the reviewer's own first-pass draft as input and instructs: re-read the diff line-by-line; re-verify each draft finding against the code (promote/demote/drop false positives); surface anything the first pass missed (the draft is a starting map, not a ceiling). It must **not** emit a `VERDICT` line (the synth owns the verdict). The prompt-contract test extends to assert the refine prompt has the line-by-line clause and no `VERDICT`.

`review-implement-synth.md` and `implement-review-light-synth.md` are unchanged.

### 5. Orchestration (`main.rs::run_review_step`)

Three edits in the block at `main.rs:1024–1070`:

- **Sizing call + tier resolution (fixes the auto-failure bug):** the git invocation drops `--numstat` and reads the full patch (`git -c core.quotePath=false diff --no-ext-diff --no-textconv …`, wrapped in `tokio::time::timeout(rcfg.slice_timeout, …)`; rename detection left on). The **`(usize::MAX, usize::MAX)` sentinel is removed** — feeding it through the new 3-way `select_tier` would select **Thorough** (`MAX ≥ thorough_min_lines`), the opposite of the intended fail-safe. Instead, sizing yields `Option<(files, lines)>` (`None` on git error/timeout) and tier resolution is explicit:

  ```rust
  let tier = match (depth, sizing) {
      (Depth::Forced(t), _)        => t,                 // forced always wins
      (Depth::Auto, Some((f, l)))  => select_tier(f, l, /* thresholds */),
      (Depth::Auto, None)          => Tier::Standard,    // unknown size → standard fail-safe
  };
  ```

  (`Depth::resolve` is updated to take `Option<(files, lines)>` and encode this, or the match lives inline in `run_review_step` — either way `Auto + None → Standard`, `Forced` still forces.)
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

- **`Option<Depth>` threading (required).** Today the CLI stores a bare `depth: review::Depth`, so "absent `--depth`" and "`--depth auto`" collapse to the same `Auto` value — there is no way to express "fall through to config" vs. "explicitly auto." Both the fresh-start args and the resume args must carry `depth: Option<review::Depth>`: `None` = not specified by this caller; `Some(d)` = this caller pins `d`.
- **Fresh-start precedence:** `call_depth.unwrap_or(rcfg.default_depth)` at one resolution point. `parse_depth_flag` gains `"auto" → Some(Depth::Auto)`; CLI absence → `None`.
- **`Depth::Forced(_)` is the deterministic override:** `--depth standard` on a 500-LLOC diff runs standard (downgrade); `--depth thorough` on a 5-line diff runs thorough (upgrade); `default_depth = "thorough"` makes every `implement` run thorough by default.
- **Resume precedence (replay-correct):** `None` (no `--depth` on resume) → use the checkpoint's `forced_depth`; `Some(Forced(t))` → override and persist `t`; **`Some(Auto)` → override and persist `None`** (so `--depth auto` can *clear* a previously-forced checkpoint — impossible today, since `Auto` reads as "no override"). The persisted `forced_depth` (`None`=Auto) is what replays.
- **Caveat — `forced_depth=None` freezes the *policy*, not the *thresholds*.** A resumed `Auto` run re-sizes each attempt against the **current** config `light_max_*`/`thorough_min_*`; editing thresholds between a crash and a resume changes which tier an auto run picks. This is acceptable (auto means "size each attempt") but must be documented so it isn't mistaken for a replay bug. Only `Forced(t)` is fully threshold-independent.
- **Shared parser (no duplication).** `parse_depth_flag` is private in `main.rs`; `config.rs` (for `default_depth`) and the checkpoint helpers must not re-implement it. Move depth parse/format into the pure `review.rs` module — `Depth::parse_flag(&str) -> Result<Option<Depth>, _>`, `Depth::from_forced_str(Option<&str>) -> Depth`, `Depth::to_forced_str(Depth) -> Option<String>` — and have CLI, config, and checkpoint all call them.

### 7. Config + validation (`config.rs`)

`ReviewToml` / `ReviewConfig` gain three fields:

```rust
thorough_min_lines: usize,   // default 150  (now LLOC-denominated, see note)
thorough_min_files: usize,   // default 6
default_depth: Depth,        // from string "auto"(default)|"light"|"standard"|"thorough"
```

`ReviewToml.default_depth` is a `String` (serde default `"auto"`); `to_config` parses it via the shared `review::Depth::parse_flag` (§6) into a concrete `review::Depth` (`"auto" → Auto`), rejecting unknown values with `ConfigError::Registry`. Validation in `to_config` (all via `ConfigError::Registry`):
- `thorough_min_lines > 0` and `thorough_min_files > 0`;
- `thorough_min_lines > light_max_lines` and `thorough_min_files > light_max_files` (strictly ordered bands).

**Threshold is now LLOC-denominated.** The 150/6 defaults were originally chosen against physical diff lines; 150 *logical* lines ≈ ~180 physical (typical code is ~20% blank + comment). Keeping 150/6 means thorough trips on a slightly larger code change than before — intentional, and tunable via config.

### 8. Resume / checkpoint (`bin/a2a-bridge/src/implement_resume.rs`)

No struct change — `ImplementCheckpoint.forced_depth: Option<String>` already exists (`implement_resume.rs:41`, with a round-trip test at `:309`). The shared helpers in `review.rs` (§6) gain the thorough/auto arms:
- `Depth::parse_flag`: `"auto" → Some(Auto)`, `"thorough" → Some(Forced(Thorough))`.
- `Depth::from_forced_str`: `Some("thorough") → Forced(Thorough)` (None → Auto).
- `Depth::to_forced_str`: `Forced(Thorough) → Some("thorough")` (Auto → None).

Resume resolution follows the §6 precedence: `None`→checkpoint, `Some(Forced(t))`→persist `t`, `Some(Auto)`→persist `None`. `forced_depth=None` re-sizes each attempt against current thresholds (the §6 caveat).

### 9. `reduce()` failed-reviewer accounting (`review.rs`)

`reduce()` counts failed non-`synth` nodes as failed reviewer legs. The executor (`executor.rs:284`) schedules a dependent once its inputs are **done regardless of `ok`**, and feeds the failed input's marker text forward — so in thorough, a failed `reviewer_codex_draft` does **not** stop `reviewer_codex` from running; the refine may then "succeed" on garbage. A naive "ignore `*draft` failures" filter would therefore report **zero** failed reviewers even though the codex leg fully collapsed — the bug the review caught.

Correct rule: **count distinct logical legs that had any failure.** `reduce()` normalizes each failed non-`synth` node id by stripping a trailing `_draft`, collects the distinct normalized ids, and counts them. A codex leg that fails at the draft, the refine, or both → exactly **one**. This is correct for every tier: light (1 reviewer node, no `_draft`) and standard (`reviewer_codex`/`reviewer_claude`, no `_draft`) are unchanged; thorough dedupes draft+refine per leg. The unit test covers draft-fail-refine-ok, draft-ok-refine-fail, and both-fail → all count 1 per leg.

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
- `select_tier` three-way at every boundary (light/standard/thorough edges; thorough-by-files-only; thorough-by-lines-only). Tier resolution: `Auto+None→Standard`, `Forced(t)+None→t` (the unknown-size fail-safe — guards the regression the review caught).
- `tier_workflow_suffix` for all three tiers.
- `counts_toward_depth`: markdown, each test convention, each lockfile, each generated pattern, the always-build-output dirs, and the ambiguous-counts-in cases (`build/`, `gen/`, a plain `.rs`, a `.toml`).
- `is_logical_line` (path-aware): `#[derive(...)]` / `#![...]` in `.rs` **counts** (not a comment); `#` in `.toml`/`.yaml`/`.sh`/Dockerfile is a comment; `//` in `.rs`/`.go` is a comment; `--` in `.sql` is a comment but counts in `.rs`; a `*ptr` deref counts; blank lines excluded; unknown extension strips nothing.
- `parse_diff_for_depth` (hunk-stateful + side-aware): a multi-file patch mixing code + md + test + lockfile + generated → only code/infra files and logical lines counted; a content line whose text is `---` / `--x` / `+++` (rendered `----`/`---x`/`++++`) is counted as a changed line, **not** misread as a header; `\ No newline at end of file` and `@@` headers ignored; the `@@` budget bounds the hunk; a pure rename → 0; rename-**out** of code into `docs/` counts the removals; rename-**into** code from `docs/` counts the additions; copy-with-edits; a deletion (path from `--- a/`, `+++ /dev/null`); a `"quoted path".md` still classified as markdown (excluded); mode-change-only → 0; a binary file → 0; a comment-only file → 0 files.
- `reduce()` per-leg dedup: draft-fail+refine-ok, draft-ok+refine-fail, both-fail → **1** failed leg each; standard's two distinct reviewer ids → 2; a `synth` failure → 0 reviewers.
- `Depth::{parse_flag, from_forced_str, to_forced_str}`: round-trip for `auto`/`light`/`standard`/`thorough`; `parse_flag` rejects unknown; **resume precedence** (`None`→checkpoint, `Some(Auto)`→clears forced→persist `None`, `Some(Forced(t))`→persist `t`).
- Config validation: ordered-band rejection (`thorough_min_* > light_max_*`), `default_depth` parse + reject-unknown, all `> 0`.
- Prompt-contract test: `review-implement-refine.md` has the line-by-line clause and emits no `VERDICT` (extends the existing reviewer/synth contract test).
- Podman parity test: `implement-review-thorough` + `[review]` thresholds + `default_depth` + the 1800s timeout mirrored structurally in `.podman.toml`.

**Live DoD gate (real codex + claude through the bridge):**
- A diff with ≥150 LLOC of code/infra auto-selects **thorough** → draft→refine→synth runs (4 reviewer nodes + synth, confirmed in logs) → slice present under `.git/` → APPROVE → `forced_depth` reflects the resolved depth in the checkpoint.
- `--depth thorough` on a small diff forces thorough (upgrade); `--depth standard` on a large diff forces standard (downgrade).
- A docs-only or test-file-only diff sizes to `(0,0)` → light, while the diff is still fully reviewed.

## Files touched

- `bin/a2a-bridge/src/review.rs` — `Tier::Thorough`, 3-way `select_tier`, `tier_workflow_suffix`, hunk-stateful side-aware `parse_diff_for_depth` + `counts_toward_depth` + path-aware `is_logical_line` (replacing `parse_numstat`), `reduce()` per-leg dedup, `Option<(files,lines)>`-based tier resolution, and the **relocated** shared `Depth::{parse_flag, from_forced_str, to_forced_str}`.
- `bin/a2a-bridge/src/main.rs` — sizing call (full patch + `core.quotePath=false`, `Option` result, `Auto+None→Standard`), slice condition `!= Light`, `variant_or_fallback`, `depth: Option<Depth>` on the fresh **and** resume arg structs + the override-seam resolution, callers updated to the relocated `review::Depth::*` helpers, usage strings (`--depth auto|light|standard|thorough`), the example timeout bump.
- `bin/a2a-bridge/src/implement_resume.rs` — no struct change (`forced_depth: Option<String>` exists); resume path consumes `Option<Depth>` per §8.
- `bin/a2a-bridge/src/config.rs` — `thorough_min_lines`/`thorough_min_files`/`default_depth` (via `review::Depth::parse_flag`) + ordered-band validation.
- `prompts/review-implement-refine.md` — **new** refine prompt.
- `examples/a2a-bridge.containerized.toml` + `examples/a2a-bridge.containerized.podman.toml` — `implement-review-thorough` workflow, `[review]` thresholds + `default_depth`, timeout 900→1800.

## Out of scope / deferred

- **Standalone `run-workflow` adaptive depth** (the "executor depth-gate" primitive) — thorough here is `implement`-review-only; the standalone review path remains un-tiered. Deferred (next-slice item #3).
- **Per-tier timeout** (`thorough_timeout_secs`) — kept a single `[review].timeout_secs`; the example default rises to 1800s and operators can raise further. Trivial to add later if 1800s proves tight.
- **Sub-file (language-aware) test/comment attribution** — sizing is per-file path + per-line heuristic; an inline `#[cfg(test)]` block in a code file still counts toward that file. Out of scope (would need a per-language lexer).
- **Configurable exclusion globs** — the `counts_toward_depth` heuristic is fixed (well-documented) rather than config-driven. YAGNI until a real target repo's convention is misclassified.
