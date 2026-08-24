# Handoff — R2f1b slice 4 (scheduler activation): 4A gated and CI-green as PR #80, 4B–4J planned

**Written:** 2026-08-23 · **By:** session `7de137cd-c346-425d-a3b8-e405d2325941` · **Provider:** claude
**Workspace:** `a2a-bridge` · `agent/r2f1b-pre-slice2-custody-plan` · **Measured state:** `[MEASURED]` HEAD `7f1d84dd85fcf83e334f4bfb41d9bbbddd5e2eba` · Tree CLEAN · Probe `git status --porcelain; git rev-parse HEAD` · Output inline (0 dirty, 0 unpushed)
**Predecessor:** `docs/superpowers/2026-08-21-r2f1b-3d-t3b-HANDOFF.md` (T3b lane; now COMPLETE)
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` no live `implement` run; this session owns the lane. No other agent in it. — **RESOLVED (single owner, 2026-08-23)**

**(b) Custody exposure** — `[MEASURED]` tree clean, 0 unpushed. All eleven slice candidates are secured as refs **in this repo** (`refs/t3b/*`, `refs/sep/candidate`), independent of the implement clones. — **RESOLVED**

**(c) In flight / irreversible** — `[MEASURED]` nothing running. Slice 4A is **gated, CI-green, and open as PR #80** (`d0b856f7`), awaiting an owner merge decision. Both candidates are secured out of their clones: `refs/s4/4a-candidate` (`8dfb899e`) and `refs/s4/4a-fix` (`07105482`). — **OPEN: merge is an owner call, not yet given**

**(d) Authorization granted but not exercised** — owner directives, verbatim:
- opencode/ox may land on this lane: *"head on means I am approving its output can land on this lane? approved - fold legitimate and best of both - on disagreement eacalate to me"*
- implementor choice: *"use sol as implementor. no need to use terra as openai usage resets tommorow"*

## 1. Resume order

1. **Merge PR #80 on the owner's word.** It is gated, CI-green (5/5), and merge-ready; the owner has authorised each prior slice merge explicitly and has not yet done so for 4A. Do not merge without it.
2. **Dispatch 4B** (constructible-but-refused; cap 350) per the decomposition. Base it on `main` after #80 lands — this lane squash-merges, so basing 4B on the unmerged branch creates rework.
3. Carry the disclosed 4A follow-up: `workflow_history::DirectAttemptBarrier` still self-constructs two clock epochs on the separate "direct" execution surface. Not reached by slice 4; converge it in a later sub-slice or a follow-up.
4. Update the T3a carried ledger in `docs/reliability-execution-roadmap.md` (~line 1247) — all three items are closed but the roadmap still lists them open.

## Slice 4A — settled record

| Item | Value |
|---|---|
| PR | **#80**, branch `r2f1b/slice-4a`, head `d0b856f7`, base `3dadec91` |
| Implementation | `8dfb899e` (candidate) + `07105482` (reviewed fix) |
| Implement loop | bound 3 reached with REJECT; **extended by one targeted fix round** (converging, not open-class); fix APPROVED on attempt 1, zero findings |
| Local gate | fmt 0 · workspace clippy `-D warnings` 0 · `bridge-core` 0 · hygiene 0 (both points) · workspace 101 / 9 distinct failures |
| Attribution control | base `3dadec91`, same environment, sequential: **the same 9**; set difference empty both directions |
| Pre-existing failures | `tests/smoke_cli.rs` (6), `tests/fallback_plan_cli.rs` (3) — host container/smoke |
| Mutation checks | new test reddens on `unwrap()` of `None` when only the production forward is reverted; frozen control patch byte-identical, SHA-256 `21b600c3…fd13` |
| Size | 316 counted lines vs cap 450 (reviewer's independent 304 matched the operator count exactly) |
| CI | 5/5 pass |

| **Slice 4A** | **CI-green, PR #80, unmerged** | `[MEASURED]` implementor `gpt-5.6-sol` xhigh; spec `docs/superpowers/plans/2026-08-23-r2f1b-slice4a-task.md` |
| Slices 4B–4J | pending | Per the decomposition; 4J is the arming commit |
| R2f1b slices 5–6 | pending | Slice 5 = persistence + serving parity; slice 6 = aggregate closure |
| opencode + OpenRouter agents | **done** | `[MEASURED]` live on both served instances (:18080, :18097); verified end-to-end |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| `2026-08-02-r2f1b-focused-boundary.md` | Line numbers throughout | `[MEASURED]` STALE — `liveness_profile_v1` 107 → **128**; the #22 bare await 4619 → **5233**. Cite by symbol. |
| T3a carried ledger (roadmap ~1247) | Three open items | `[MEASURED]` all three now closed: separator guard merged (#78); F8 birthtime measured in **five** environments incl. real ext4; `force_next_release_failure_for` root-caused and fixed in T3b slice 4. **Ledger not yet updated — work item.** |
| `feat/r2f1b-3c2-request-flight` branch | Looks like unmerged work | `[MEASURED]` NOT outstanding — it is a **stranded pre-squash head** (53 ahead/85 behind). PR #51 merged it. |

## 4. Open work

| # | Work | State | Exact next action | Identifiers |
|---:|---|---|---|---|
| 1 | Merge PR #80 (owner call) | next | §1 step 1 | gated + CI-green at `d0b856f7` |
| 2 | Slices 4B–4J | pending | 4B = constructible-but-refused, cap 350 | decomposition `462e676b` |
| 3 | Update the T3a carried ledger | pending | All three items are closed; roadmap still lists them open | roadmap ~line 1247 |
| 4 | Head-to-head #3 (implementation) | **not run** | #1 spec and #2 review are done; #3 was superseded by the slice-4 decomposition | — |
| 5 | R2f1b slices 5–6 | pending | Slice 5 needs its own decomposition | — |

## 5. Invariants and traps — do not do these

- **Never verify a control with one gate.** A control passing `cargo test` can still die on `clippy -D warnings` via `dead_code` before reaching its red tests. Found by ox-alpha; missed by two implement reviewers, by sol, and by the operator.
- **Never report a reddened-test population from a filtered run.** Running only the named tests under a filter is not a population measurement. The separator control's real population was 4, reported as 3.
- **Never demand "exactly one reddened test."** Wrong twice this session. A mutation defeating a well-defended obligation *should* trip several guards; demanding one prefers a control that slips past real defences.
- **Never write a spec requirement without checking it against your own data.** Six operator-authored contract defects in T3b, all the same shape: self-referential head sha, spawn ban meaning read-only, a two-commit demand the harness cannot satisfy, a tripwire on an advisory line count, "exactly one reddened test", and requiring a non-divergence guard to fail pre-change.
- **Never trust a plan's claims about the tree.** The T3b plan's crash-safety claim was false (`is_custody_record_name` did NOT reject the residue). sol's slice-4 plan re-specified `FrozenR2f1bContractV1` (55 refs already on main) and invented a 64-plan admission cap with **zero** references in the frozen scope.
- **`head -1` on multi-match output is the most dangerous habit here.** It produced a wrong test result (first binary's summary), a wrong control sha (slice 4's, not 5B's), and near-misses elsewhere. Match the specific thing.
- **zsh does not word-split unquoted variables**, and `$VAR:` eats a history modifier. Both produced probes that could not fail and reported PASS.
- **Run host gates from under `/Users/wesleyjinks/code`** — 23 `r3d0_*` tests hard-fail on a trusted-cwd precondition otherwise.
- **The workspace gate inflates under parallel load** — 29 failures under load vs 11 idle, same tree. Compare populations on an idle machine.
- **Do not hand-roll the verify container.** Three probes died of their own setup (a `/src` mount made every `tests/*.rs` look like production; `rustdoc` never launched; a shared cache served a binary with the previous mount path baked in).
- **`gh pr checks` interleaves runs.** Verify against the PR head commit via the check-runs API; a `cancelled` job can read as settled.
- **The `implement` process can wedge AFTER completing** (9h at 0% CPU once). Watch the log for terminal markers, not the process.
- **The pipeline can misreport a review.** 5A printed `inconclusive / no actionable signal` over a body reading `VERDICT: APPROVE`.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| `origin/main` | `3dadec91c1a1e4d798ffb68209936b37f71c1da1` |
| Branch HEAD | `7f1d84dd85fcf83e334f4bfb41d9bbbddd5e2eba` |
| Slice 4 decomposition | `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (`462e676b`) |
| Slice 4A spec | `docs/superpowers/plans/2026-08-23-r2f1b-slice4a-task.md` |
| Scratchpad | `/private/tmp/claude-501/-Users-wesleyjinks-code-a2a-bridge/7de137cd-c346-425d-a3b8-e405d2325941/scratchpad` |
| 4A log | `<SCRATCH>/s4a-implement.log`; fix round `<SCRATCH>/s4a-fix.log`; gates `<SCRATCH>/g4a-*.log` |
| Implementor | `gpt-5.6-sol`, effort `xhigh`, `kind=container_rw`, in `examples/a2a-bridge.r2f1b-impl.toml` |
| Head-to-head config | `examples/a2a-bridge.workflows-sol-xhigh.toml` — `author`/`author-oc`, `review-sol`/`review-oc` |
| Served operator bridge | `:18080` · config `~/Library/Application Support/a2a-bridge/operator/a2a-bridge.toml` |
| Served stockTrading bridge | `:18097` · config `~/.config/a2a-bridge/stockTrading-v10impl-write.toml` |
| ox model ids | `opencode-go/ox-alpha-free` (ACP) · `stealth/ox-alpha` (OpenRouter api) |
| Free disk at write | 189 GiB |

## 7. Refutation verdict and owner questions

**§2c verdict:** RUN — claim: "D11's internal timers are strictly shorter than their observable bounds, and no timer arms in 4A." · pass: INDEPENDENT (dual reviewers across four review rounds, plus operator re-execution of the mutation control) · evidence tier: TEST-BACKED — the frozen control mutates the 30,000 ms internal timeout to 31,000 ms (equal to the observable bound) and reddens `internal_action_timers_leave_observable_settlement_margin`; the relationship, not the literal, is what the test asserts · record: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-operator-gate.md` and `…-slice4a-handoff.md`

**Questions the owner owes an answer to:**
1. **ox-alpha reliability.** Three distinct failures today — provider `network_error`, HTTP 429 rate-limit, and malformed output framing twice. It is a temporary promotional model and may vanish. Keep using it for review (where it earned its place), or move to `opencode-go/kimi-k3`? Non-blocking.
2. **Effort parity.** ox ran at opencode's **unset default** throughout; opencode advertises no effort levels, so `reasoning_effort` is unreachable via ACP. The head-to-heads were therefore not effort-matched. Worth re-running any comparison via the `kind="api"` OpenRouter agent, where effort is reachable? Non-blocking.
3. **Head-to-head #3** was superseded by the slice-4 decomposition exercise. Run a separate implementation comparison later, or consider the series closed at two? Non-blocking.
