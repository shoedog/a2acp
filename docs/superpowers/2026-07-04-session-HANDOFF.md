# Session Handoff — 2026-07-04

**Purpose:** resume cleanly after a session compact. This one session ran: strategic
analysis → AGPL relicense + CLA → three code/docs hardening waves → the repo's first
public release (v0.2.0) → an eval harness (M3, in progress). Read this first, then the
memory index (`~/.claude/.../memory/MEMORY.md`).

## What shipped to `main` today (all pushed)

| Commit | What |
|---|---|
| `4c969b2` | `docs/2026-07-03-strategic-analysis.md` + `analysis-second-opinion.md` — six-lens whole-repo assessment (the roadmap source; identity ruling: "personal tool, published well") |
| `45bf05b` | **Relicense Apache-2.0 → AGPL-3.0-only** + `CLA.md` + `CONTRIBUTING.md` + CLA-Assistant workflow |
| `0d4d12c` | **Wave 1** — SQLite WAL pragmas; the `Configuring` claim-state lock fix (verified serve-wide serialization bug); async worktree git; CI toolchain pin + MSRV |
| `db4a8b3` | **Wave 2** — README rewrite; 172 one-shot artifacts moved to `docs/history/` (allowlist 208→37); ADR-0032 sandbox tier model + `tiers.toml` |
| `18e1c5a` | **Wave 3** — uniform `--help` (incl. the `init --help` file-scaffold hazard); **BREAKING: silent config auto-write removed**; `a2a-bridge doctor` (9-check read-only preflight); A2A golden wire tripwire |
| `d83241cf` | **M2** — v0.2.0 bump, CHANGELOG, `release.yml`, cargo-binstall metadata, install docs |
| tag `v0.2.0` | **FIRST PUBLIC RELEASE** — 3 tarballs (macOS arm64, Linux x64/arm64) + SHA256SUMS live at github.com/shoedog/a2acp/releases/tag/v0.2.0; pkg-url verified HTTP 200 |

Each wave went through the same pipeline: I write the spec → **codex gpt-5.5 xhigh spec-review via the bridge** → parallel sonnet implementors → opus (+me) task reviews → workspace gates + live gate → **dual whole-branch review (opus 4.8 + codex xhigh)** → fold → re-verdict → merge. The bridge dogfooded its own review path throughout.

## The one operational rule this session proved (4×)

**When the two review lenses conflict, read the primary evidence yourself — codex won every factual dispute.** Wave 1 (checkout lock codex found, opus cleared), Wave 2 (AGENTS staleness), Wave 3 (open-egress image advisory), M2 (dispatch-on-tag-ref BLOCKER). The dual-lens gate repeatedly caught bugs *I* would have merged, including in my own specs.

## M3 — COMPLETE (merged `256d356`; baseline committed 2026-07-05)

**STATUS 2026-07-05 — DONE (one owner action left: the C2 spot-check).** Infra merged to `main` (`256d356`). First measured baseline committed at `evals/results/baselines/2026-07-04-review-seeded-v1/` (full 45-run 3-cell matrix, kiro judge). **Headline result — seeded-defect recall = 11/11 = 1.000 in ALL three cells; the differentiator is PRECISION on clean twins, not recall:** duo (shipping shape) 23 false findings / item-pass 0.733 / TN 0; codex-solo 2 false findings / item-pass 0.867 / TN 2; claude-solo 17 false findings / 0.733 / TN 0. So the bridge catches every seeded bug but the duo/claude configs over-flag clean code, and the synth pass does NOT filter false positives (duo has the MOST). PROVISIONAL/n=15; awaiting owner C2 spot-check (`spotcheck.yaml`, fill `agree:` fields). One judge_error (codex-solo-rc-02, a clean item where the reviewer suggested tests) was resolved by a single standalone re-judge, folded + disclosed via the report's NOTE banner. **Blocker hit + fixed:** a concurrent Fable eval had set `~/.claude/settings.json` `"model"` to fable (bridge-blocklisted → every claude cell failed); reset to `opus` (owner-approved). See `memory/m3-fable-model-collision.md`. **NEXT = roadmap tail: #9 bin extraction, #10 Coordinator T9.**

**Goal:** measure the bridge's review quality — the strategic bet the analysis said is unmeasured. Design: adopt the eval *science* from the owner's `~/code/prompts-skills-steering` (`eval_framework_v1-1.md`; 4 live-validated experiments), rebuild the *engineering* bridge-native.

**Committed on the branch (all review-clean):**
- `099bf8e` spec v2.1 (codex xhigh reviewed; folded)
- `36e5c89` + `68bfa02` — corpus `evals/tasksets/review-seeded-v1/`: 15 items (11 seeded, one per this repo's own bug classes; 4 clean-as-correct-twins). Codex fairness review → 4 fixes folded (rc-01 was a poisoned clean item; rs-08/rs-02/rs-10/rs-04 rubric calibration). All fixtures build; validator green.
- `0602886` + `29f9ebb` — harness `evals/harness/`: bridge-native runner (drives real `run-workflow` per cell×item), BLIND cross-family judge, ported stdlib metrics. Opus review → 2 MAJOR + 3 MINOR folded (skip failed reviewer calls; disclose the lens confound). 41 offline tests green.

**Design specifics that matter on resume:**
- **Matrix (3 cells × 15 items):** `duo` (codex+claude reviewers → claude synth, grades synth) / `codex-solo` (raw codex, no synth) / `claude-solo` (raw claude, no synth).
- **Judge = kiro** (`kiro-cli acp`, Amazon family — cross-lineage from codex+claude authorship set). PIVOTED from gemini mid-build because gemini's OAuth is dead on this machine (probe: `throwIneligibleOrProjectIdError`). Fallback chain (README only): ollama-cloud → ollama-local qwen2.5-coder:7b (emergency, judge-tier flag). Kiro auth confirmed live.
- **Models = bare defaults** (config pins no model/effort) — measures the SHIPPING defaults: codex-acp gpt-5.5 / claude-agent-acp (Opus 4.8, not observable through bridge) / kiro. All on OAuth **subscriptions**, so dollar cost ≈ $0; the real cost is subscription quota (~120 agent turns) + ~60–120 min wall-clock at concurrency 2.
- **Known-honest caveats baked into the report:** PROVISIONAL/n=15 banner; McNemar demoted to paired-flips display; claude-solo's recall is LENS-confounded (architecture lens vs correctness-heavy corpus) and the report says so explicitly — do NOT read it as "claude is worse."

**WHAT'S LEFT (the actual deliverable — DoD items 3+4, DEFERRED for quota):**
1. `cargo build --release -p a2a-bridge -j 1` (binary gets cleaned by concurrent cargo).
2. **Live smoke:** `cd evals && uv run python -m harness.runner --smoke` (duo × 3 items, real kiro judge) — proves the pipeline end-to-end + gives a per-turn timing.
3. **Baseline:** full matrix → `results/baselines/<date>-review-seeded-v1/report.md` (the repo's FIRST measured catch-rate) + `spotcheck.yaml` for the owner's C2 human calibration pass.
4. Whole-branch dual review (RUNNING as of this handoff — codex `bq4zqnm0z` + opus `ac9e5bd14c1ba3915`), merge, push.

**Merge stance (owner-approved judgment call):** `evals/` is fully isolated — outside the cargo workspace (fixtures have empty `[workspace]` tables), outside CI, outside repo-hygiene scope — so it CANNOT break the shipping product. Safe to merge the reviewed infra now with the baseline explicitly deferred, IF the whole-branch review is clean. The measured-catch-rate deliverable is the only thing that then remains, gated purely on quota/time.

**Quota context at pause:** owner running a Fable eval in another session near a 5h limit (reset ~2h out from this handoff). The duo cell is the heaviest subscription consumer — smoke first, duo-cell baseline before the full matrix, owner's call on scope.

## How to resume M3 (fastest path)
1. Read the two open review outputs in scratchpad (`m3-branch-review-codex.md`, and the opus agent result) — fold any findings.
2. Merge `feat/m3-eval-harness` if clean (baseline deferred).
3. When quota allows: rebuild binary → `--smoke` → duo-cell baseline → commit the report → owner C2 spotcheck.
4. Then the roadmap tail: **#9 bin extraction** (~6.4k lines of controller loops out of the bin crate) and **#10 Coordinator T9** (finish the A2A-inbound → Coordinator migration) — the two structural slices from the strategic analysis.

## Roadmap position (from the strategic analysis)
Done: identity/license, Waves 1-3, M2 (release). In progress: M3 (evals). Remaining top-10: #9 bin extraction, #10 Coordinator migration. Medium/long-term ideas (M1 composable isolation, L1 A2A federation, L9 workflow packs, etc.) all still in `docs/2026-07-03-strategic-analysis.md`.
