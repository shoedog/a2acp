# HANDOFF — 2026-07-10 — M4 Slice 3 (retention) + gpt-5.6-sol in-container

Session origin: recovered the a2a-bridge session an overnight macOS auto-update reboot (01:44) killed,
then drove M4 Slice 3 through spec sign-off + Slice 3a implementation, and diagnosed why gpt-5.6-sol
fails inside the containerized `implement` agent.

Two independent workstreams below. All artifacts are committed (branches noted); nothing is merged to `main`.

---

# PART A — M4 Slice 3 (Retention under `[storage]`)

## TL;DR
- **Design is signed off through rev6** via a 6-revision, adversarially-reviewed loop. Verdict arc:
  `rev1 REDESIGN → rev2 FIX/REVISE → rev3 FIX → rev4 FIX (introduced+fixed a cancel regression) →
   rev5 FIX → rev6 FIX (remaining items are HTTP route-code precision + one pathological edge, no data-loss)`.
- **Slice 3a is IMPLEMENTED** (ownership fix + finalization barrier, no deletion), **review-APPROVED**,
  **not merged** — parked for your review with two small follow-ups.
- **3b (deletion + routes) is deferred**, with one owner scoping decision already made (below).

## Where everything lives
Branch **`feat/m4-slice3a-ownership-finalization`** (commit `564a1db`) holds all design/review/plan docs:
- Specs: `docs/superpowers/specs/2026-07-10-m4-slice3-retention-design{,-rev2,-rev3,-rev4,-rev5,-rev6}.md`
  (**rev6 is the current design of record**).
- Reviews (10): `docs/superpowers/reviews/2026-07-10-m4-slice3-*.md` — the per-rev data-safety (codex
  gpt-5.5 xhigh / gpt-5.6-sol) + architecture (sonnet, fable) reviews and synthesis docs.
- Plan: `docs/superpowers/plans/2026-07-10-m4-slice3a-implementation.md` (the 3a TDD task list T1–T6).

## Slice 3a — implemented, needs your review
- **Commit `11dfd32`** on branch **`implement/impl-10105-reruvu21`**, in the quarantine clone
  **`/Users/wesleyjinks/code/.a2a-implement/impl-10105-reruvu21`** (produced by `a2a-bridge implement`,
  gpt-5.5 xhigh — sol was blocked, see Part B).
- Diff: **12 files, +1966/−253** — ownership caller-sites (`coordinator/batch/detached/executor`), the
  storage-authored `usage_finalized_ms`/`usage_finalization_kind` barrier + `last_artifact_ms` recency
  bump (`sqlite.rs`, `task_store.rs`), `UsageFinalized(Option<UsageSnapshot>)` + `TurnFinal`-scoped dedup +
  Prometheus seeding (`observ/lib.rs`). The central A3 ownership overwrite in `spawn_detached_workflow`
  is also committed on the feat branch (`564a1db`).
- **Review verdict: APPROVE.** **Verify: FAIL at `fmt` only** (cosmetic — run `cargo fmt`).
- **Two follow-ups before merge:**
  1. `cargo fmt --all` (the whole verify failure is rustfmt style).
  2. Delete the stale `update_turn_usage` overrides in both concrete stores (`task_store.rs:357` + sqlite)
     — the review's one MAJOR: they make `no_usage_finalization_rejects_existing_usage_columns` pass via
     the wrong rejection path (test-quality, **not a runtime defect**).
- **To land it** (re-authored to you; bot identity is pre-merge only):
  ```
  git -C ~/code/a2a-bridge fetch ~/code/.a2a-implement/impl-10105-reruvu21 implement/impl-10105-reruvu21
  git -C ~/code/a2a-bridge cherry-pick -n FETCH_HEAD && git -C ~/code/a2a-bridge commit -C FETCH_HEAD --reset-author
  ```

## Deferred to Slice 3b (deletion + `[storage]` + drill-down routes)
From the rev6 sign-off (codex gpt-5.6-sol), with the **owner decision (2026-07-10)**:
1. **Route contract relaxed to task-level 410 (accepted non-goal).** Once a task has any purged artifact
   (`artifacts_purged_at` set), an absent artifact on it returns **410**; precise per-artifact
   404-vs-410 is NOT promised. Closes rev6 sign-off #2, simplifies #4 (no per-artifact purge history).
2. **Read-to-commit recency window (#1):** stamp recency commit-adjacent, or fail closed on a
   long-running (>TTL) writer transaction. Add a post-clock-read stall test.
3. **Body-first, race-safe route resolution (#3):** current-row lookup is authoritative; re-read the
   purge marker after an absent body. Add purge-between-reads tests.

## Recommended next step for Slice 3
Merge 3a (after the two follow-ups). Then implement 3b per rev6 + the scoping decision, TDD. The recency
model, `i64::MAX` fail-closed sentinel, `journal_fold_guard` atomicity, and `artifacts_purged_at` are all
settled in rev6 — 3b is TTL purge + `RetentionService` + the relaxed routes.

---

# PART B — gpt-5.6-sol fails inside the containerized `implement` agent

## Symptom
`a2a-bridge implement` with the impl agent set to `model = "gpt-5.6-sol"` dies at the edit turn
(`workflow did not complete`, no commit). `gpt-5.5` through the identical path works. This blocked
running Slice 3a on sol (fell back to gpt-5.5).

## Root cause (captured): a bridge ↔ codex-acp **model-selection API mismatch** — NOT the model/env
codex-acp **1.1.2** changed how models are advertised/selected:
- `session/new` returns a dedicated **`models: { availableModels:[…], currentModelId }`** field, where IDs
  carry the effort as a suffix (**`gpt-5.6-sol[medium]`**, `[high]`, …). The container session **defaults
  to `gpt-5.6-sol[medium]`**.
- `configOptions` now contains only **`mode`** (read-only / agent / agent-full-access) — **no `model`
  config option**.
- `bridge-acp` selects the model via `configure_model_option` → **`session/set_config_option(config_id="model", …)`**
  (`crates/bridge-acp/src/acp_backend.rs:598-655`, the `set_config_option` at `:618`), and **explicitly maps
  a failure there to `AgentCrashed`** (`:621`). That is the crash the bridge reports
  ("session/prompt failed: transport error or kill-switch escalation").

So the bridge drives model selection through the **old config-option API** that codex-acp 1.1.2 moved to
the new `models` field.

## Ruled out (with evidence) — sol itself is fine in the container
- **codex-acp version:** identical `1.1.2` host vs container.
- **codex runtime:** identical bundled `@openai/codex` **`codex-cli 0.144.1`** (linux-musl) host vs container.
- **egress:** sol fails with egress **open** too → not the tinyproxy allowlist.
- **auth:** `~/.codex/auth.json` and the container mount `~/.config/a2a-creds/codex/auth.json` are
  byte-identical (chatgpt tokens).
- **MCP / effort / model access:** sol runs fine via `codex exec` (xhigh, danger-full-access) **and** via a
  hand-driven codex-acp ACP session (initialize → session/new → session/prompt → returns "PONG"), with and
  without the lsp MCP. It only fails when the bridge does an **in-session model switch**.

## Reproduction harness
`docs/superpowers/2026-07-10-acp_drive-sol-repro.py` (on branch `fix/enable-gpt56-sol-container`; also in
this session's scratchpad). Drives codex-acp in the `a2a-toolchain` image through the ACP handshake.
- `python3 …/acp_drive-sol-repro.py gpt-5.6-sol` → sol works (model set at launch).
- `SWITCH_MODEL=gpt-5.6-sol python3 … gpt-5.5` → reproduces the bridge's in-session `set_config_option`
  path (note: the repro's `set_config_option` params use placeholder field names — codex-acp 1.1.2's real
  schema is `configId`/`type`/`value`; fix these to fully replay the bridge call).

## The one open thread
Why **gpt-5.5 succeeds** but **sol crashes** through the *same* `configure_model_option`/`set_config_option`
path. Best hypothesis: the effort-suffixed IDs (`gpt-5.6-sol[medium]`) interact with the bridge's
`model_values` / `resolve_model` / effort-walkdown differently than the plain `gpt-5.5` entry. Trace
`model_values(opts0)` and `resolve_model` (`acp_backend.rs`) against codex-acp 1.1.2's actual `session/new`
payload to nail it.

## The fix (bridge-acp code — your domain)
Adapt `bridge-acp` model selection to codex-acp 1.1.2's `models` field (effort-suffixed `modelId`s) instead
of the removed `model` config option. Likely also unifies model+effort (the `[effort]` suffix replaces the
separate effort walk-down).

## Container image work — DONE and committed (real fix, but NOT the sol fix)
Branch **`fix/enable-gpt56-sol-container`** (commit `c95084e`, worktree `~/code/a2a-bridge-sol-fix`):
- `deploy/containers/reader.Containerfile`: `codex-acp 1.1.0 → 1.1.2`, and **kiro-cli GNU → musl** build.
  (kiro-cli's GNU `latest` now needs glibc 2.39; the `node:24-slim` base is glibc 2.36, so the GNU install
  failed — this would break **any** reader rebuild today, independent of sol. The musl build is static.)
- Rebuilt `a2a-agent-reader:latest` + `a2a-toolchain:latest` (verified: codex-acp 1.1.2, kiro-cli 2.12.1).
- This is worth keeping regardless of sol; it un-breaks the image build and brings codex current.

## Temporary scaffolding to clean up
- `examples/a2a-bridge.m4-slice3a-impl.toml` + `…-openegress.toml` — the impl-run configs (docker
  containerized, impl agent = codex; the openegress one was the A/B test). Delete or fold into examples.
- Quarantine clones under `~/code/.a2a-implement/impl-*` (except keep `impl-10105-reruvu21` until 3a is
  merged).

---

## Branch / commit index
| Branch | Commit | Contents |
|---|---|---|
| `feat/m4-slice3a-ownership-finalization` | `564a1db` | Slice 3 design (rev1–6) + 10 reviews + 3a plan + central ownership overwrite + this handoff |
| `implement/impl-10105-reruvu21` (clone) | `11dfd32` | Slice 3a implementation (review-APPROVED, needs `cargo fmt` + override cleanup) |
| `fix/enable-gpt56-sol-container` | `c95084e` | reader.Containerfile: codex-acp 1.1.2 + kiro musl; ACP repro harness |
