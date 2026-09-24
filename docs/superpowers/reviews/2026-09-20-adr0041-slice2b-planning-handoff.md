# Handoff — ADR-0041 Slice 2B local capsule planning

**Written:** 2026-09-20T22:51:32Z; refreshed 2026-09-23 after R1 provenance repair cycle 1 · **By:** Codex `/root` · **Provider:** codex + Claude ACP review
**Workspace:** retained implementation clone plus planning worktree · **Measured state:** `[MEASURED]` R1 provenance candidate `88013eb4` APPROVED; PR #105 open
**Predecessor:** `docs/superpowers/reviews/2026-09-16-adr0041-slice2a-handoff.md`
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker from GitHub/remote-main probes, merged ADR/code, and predecessor handoffs. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` provenance review cycle 1 completed APPROVE; no agent is in flight — **RESOLVED 2026-09-23.**
**(b) Custody exposure** — `[MEASURED]` reviewed candidate `88013eb4` and docs successor `5f79427f` were pushed to `feat/adr0041-slice2b1-capsule-contracts-20260923`; PR #105 is open — **RESOLVED for local and remote branch custody.**
**(c) In flight / irreversible** — `[MEASURED]` review terminal and cleanup were complete. No capsule/restore effect, cleanup, remote promotion, merge, or operator mutation was started — **RESOLVED 2026-09-23.**
**(d) Authorization granted but not exercised** — The R1 repair, controller verification, independent approval, push, and PR creation are complete. Merge, cleanup, 2B2/2B3, and operator mutation remain unauthorized.

## 1. Resume order

1. In this exact worktree run `git status --short --branch`, bind HEAD/base to the identifiers in §6, and require no code paths outside the planning delta.
2. Read ADR-0041 §§4, 8, 9, 11, 14, and 17; the Slice 2A handoff §4; and the complete plan in §6.
3. Bind both review records, the final approved closure, and the typed implementation task in §6.
4. Preserve reviewed candidate `88013eb4`, its review record, and the docs-only publication successor.
5. Inspect PR #105 and stop. Do not merge or begin 2B2 without new owner direction.

**STOP conditions:** Do not start 2B2/2B3 effects, use a bare path as a quiescence/capture capability, add remote/provider/destructive authority, clean worktrees, merge, or mutate the running operator. Publication is limited to the approved PR branch. An open-class review population or exhausted nonconverging cap parks the plan.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| PR #104 merge verification | done | `[MEASURED]` `gh pr view 104` reports `MERGED`, head `65a572df`, merge `27a885f6`, at `2026-09-20T22:01:16Z`; `git ls-remote origin refs/heads/main` returns the same merge commit. |
| Merge topology/content control | done | `[MEASURED]` `git show -s --format='%H%n%P%n%s' origin/main` names `65a572df` as second parent; `git merge-base --is-ancestor 65a572df origin/main` exits 0; `git diff --name-status 65a572df origin/main` is empty. |
| Exact planning base | done | `[MEASURED]` isolated branch/worktree created at `27a885f6`; older primary and PR worktrees were not edited or cleaned. |
| Slice 2B parent plan | done | `[MEASURED]` `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md` defines serial 2B1 contracts, 2B2 isolated export/object closure, and 2B3 inert restore/hidden-state proof. |
| Durable-state reconciliation | done | `[MEASURED]` roadmap plus Slice 1B/2A handoffs now record PR #104 merged and preserve adoption/effects as separate gates. |
| Documentation gates | done | `[MEASURED]` `git diff --check` exits 0; locked/offline repository hygiene reports 41 tracked artifacts / 9 validated example configs. No code changed, so no implementation/full-suite claim is made. |
| Independent spec review | approved | `[MEASURED]` Opus 5/high round 2 returned `APPROVE`: all 4 inherited WRONG RESOLVED, 0 new WRONG, 6 non-blocking SMELL. The converging two-round cap is exhausted; residuals were folded without extension. |
| Implementation/effects | 2B1 approved; PR #105 open | `[MEASURED]` candidate `88013eb4` passed two discriminating mutations, full host suites 4,471/0 and 4,477/0 with 13 ignored, and static/policy/hygiene gates. Sol/xhigh review APPROVED with 0 WRONG / 5 deferred SMELLs. No capsule/restore filesystem or Git effect started. |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| `docs/reliability-execution-roadmap.md` | PR #104 was open/unmerged and old `main` was current. | `[MEASURED]` reconciled to merge `27a885f6`, second-parent head `65a572df`, and Slice 2B planning. |
| Slice 2A handoff | PR #104 merge was unexercised. | `[MEASURED]` reconciled to the exact merge and points to the Slice 2B planning candidate. |
| Slice 1B handoff | PR #104 merge decision was open. | `[MEASURED]` reconciled to the exact merge and next spec-review gate. |
| Memory store | Prior note says Slice 2A is uncommitted/unpublished. | No direct owner request to update memory; live Git/GitHub evidence and repository docs supersede it for this lane. |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Planning custody | done | Preserve the approved closure, implementation task, and this handoff seal; do not push without separate authority. | None. | Approval `56922bdd`; task `81c292ce`; final plan digest in §6. |
| 2 | Spec review | approved | Preserve both review records; no third round. | None. | Round-1 and round-2 records in §6. |
| 3 | Child 2B1 implementation | approved; PR #105 open | Preserve reviewed code `88013eb4` and monitor PR checks; do not merge without owner direction. | Merge authority. | `https://github.com/shoedog/a2acp/pull/105`; review `exec-e7ee36345656679aec8923c4be9d25b6`. |
| 4 | Children 2B2/2B3 | parked | Proceed serially only after each predecessor is approved. | 2B1 then 2B2 approval; effect authority. | §§5–6 of the plan. |
| 5 | Worktree cleanup/operator adoption | parked | Keep separate from Slice 2B planning and implementation. | Separate exact authority and custody proof. | ADR-0041 stages 3–5. |

## 5. Invariants and traps — do not do these

- Never treat PR merge as remote custody or running-operator adoption — it proves project integration only.
- Never let repeated inventory stand in for coherent snapshot/quiescence — a writer can mutate and restore between scans.
- Never prove only that manifest objects are a subset of a pack — closure requires exact isolated inventory equality plus connectivity.
- Never let Git consult alternates, promisor/lazy fetch, ambient config, hooks, filters, or credentials during proof — those recreate the laptop dependency or execute archived behavior.
- Never restore raw config/hooks into the active plane — preserve originals as evidence and synthesize an inert active configuration.
- Never implement 2B1 and filesystem/Git effects in one dispatch — the serial split is the review-convergence control.
- The primary checkout is on an older unrelated branch; do not edit, reset, switch, or clean it.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Merge commit / current remote main | `27a885f6d4af6a517c2a8899aa5bfe36605b8fb7` |
| PR #104 reviewed head | `65a572df40f07316124a70b7ec353a2d38dd334d` |
| PR URL / merged at | `https://github.com/shoedog/a2acp/pull/104` / `2026-09-20T22:01:16Z` |
| Planning branch | `docs/adr0041-slice2b-plan-20260920` |
| Planning worktree | `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/slice2b-plan-20260920` |
| Parent plan | `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md` |
| Predecessor handoff | `docs/superpowers/reviews/2026-09-16-adr0041-slice2a-handoff.md` |
| Planning content commit / plan SHA-256 | `0ddfbb489d39e7c8367292df2cdf85b3cd3b18b4` / `24a99946f4bab284c11089ea4b1f0b32eeb4fc7e13734e096a1d986e3973784a` |
| Round-1 repaired-content commit | `9d982940ed9bd030a42e7284eb3a6c92f0e940f8` |
| Repaired candidate plan SHA-256 | `deac1ef6edae6c6ebcb3a37b1422114b811b2e3b365b83b586517c37aedb4162` |
| Round-1 review record | `docs/superpowers/reviews/2026-09-20-adr0041-slice2b-spec-review-round1.md` |
| Round-1 execution / attempt | `exec-4206c5c65ea2dd11959c4c1b34546e2b` / `attempt-b299fb54e0b360d808021749f8d3291c` |
| Round-2 review record | `docs/superpowers/reviews/2026-09-20-adr0041-slice2b-spec-review-round2.md` |
| Round-2 execution / attempt | `exec-4f668e0a4d8b406c07f79001174a2452` / `attempt-fac52a4c94cfc419a2e0edf582e01e16` |
| Final approved closure plan SHA-256 | `fd865c357fad25980fd23bf16f7af2d9cff10dc429de6a55577d09ae6be5fa71` |
| Approval-closure commit | `56922bdd66a6f92522848f2167647e9576e74d8a` |
| 2B1 implementation task / commit | `docs/superpowers/plans/2026-09-20-adr0041-slice2b1-implementation-task.md` / `81c292ceee9fcb1221747d606f862929c5e880e7` |
| Second repaired code candidate / base | `8d9d4c45d93ef6441aef52fea2fc35db4b2316b9` / `80d4586828aca7d5959b3931f3d30d2cd593e662` |
| R1 provenance repair cycle 1 incoming HEAD / repair contract SHA-256 | `99df794cac012ce35dc9d33b4ac8182fe29fe407` / `4dd06fae44e072fdfdd8e650e9cc8ce2a4e8c6ee9a7d855c74737fc59b2f9b13` |
| Second closure review | `docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-second-closure-review.md` |
| Second closure execution / attempt | `exec-6fb6dc2200ad44563d487e3e2ee96408` / `attempt-0c3b24562fa60b0e6c9fd6dd1e850641` |
| Second closure raw result SHA-256 | `574c9481eb6a9fe4a862b55d001b5fe57142f472577b38c5a3a044a7fa7c46bb` |
| Approved provenance candidate / tree | `88013eb4408d5afecb0b9101ef43c9c398226ef5` / `6159fa2f95e95d5976f7738e5a5e49d7ecf4f48a` |
| Provenance review / execution / attempt | `docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-provenance-review.md` / `exec-e7ee36345656679aec8923c4be9d25b6` / `attempt-86036c5afb97c003d9f62bb2fc0c9163` |
| Provenance review raw result SHA-256 | `ae10a401186fc9933a959a17ff6da1c8d5727f0e2ee79440ab63306663fb2eb0` |
| Published branch / PR | `feat/adr0041-slice2b1-capsule-contracts-20260923` / `https://github.com/shoedog/a2acp/pull/105` |

## 7. Refutation verdict and owner questions

**§2c verdict:** REFUTED — corrected in place · claim: "The second repair closes all three inherited Slice 2B1 WRONG mechanisms." · pass: INDEPENDENT · evidence tier: STATIC-ONLY plus inherited mutation/full-gate evidence · record: `docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-second-closure-review.md`

**Cycle-1 repair status:** APPROVED and published as PR #105. R1/R2/R3 are RESOLVED with no blocker. Merge, cleanup, 2B2/2B3, and operator mutation remain separate gates.
