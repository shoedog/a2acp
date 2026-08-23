# Handoff — R2f1b slice 4 (scheduler activation): 4A in flight, 4B–4J planned

**Written:** 2026-08-23 · **By:** session `7de137cd-c346-425d-a3b8-e405d2325941` · **Provider:** claude
**Workspace:** `a2a-bridge` · `agent/r2f1b-pre-slice2-custody-plan` · **Measured state:** `[MEASURED]` HEAD `7f1d84dd85fcf83e334f4bfb41d9bbbddd5e2eba` · Tree CLEAN · Probe `git status --porcelain; git rev-parse HEAD` · Output inline (0 dirty, 0 unpushed)
**Predecessor:** `docs/superpowers/2026-08-21-r2f1b-3d-t3b-HANDOFF.md` (T3b lane; now COMPLETE)
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` one live `a2a-bridge implement` (slice 4A); this session owns it. No other agent in the lane. — **RESOLVED (single owner, 2026-08-23)**

**(b) Custody exposure** — `[MEASURED]` tree clean, 0 unpushed. All eleven slice candidates are secured as refs **in this repo** (`refs/t3b/*`, `refs/sep/candidate`), independent of the implement clones. — **RESOLVED**

**(c) In flight / irreversible** — `[MEASURED]` **slice 4A `implement` RUNNING, 1h28m elapsed**, implementor `gpt-5.6-sol` xhigh. Log `<SCRATCH>/s4a-implement.log`. Effect-free by design: 4A arms no timer and leaves the executor untouched. If it dies, **fetch the candidate out of its clone before anything else** — see §5. — **OPEN until it terminates**

**(d) Authorization granted but not exercised** — owner directives, verbatim:
- opencode/ox may land on this lane: *"head on means I am approving its output can land on this lane? approved - fold legitimate and best of both - on disagreement eacalate to me"*
- implementor choice: *"use sol as implementor. no need to use terra as openai usage resets tommorow"*

## 1. Resume order

1. **Wait for 4A**, then **secure the candidate out of its clone immediately** (`git fetch <clone> 'refs/heads/implement/<id>:refs/s4/4a-candidate'`). A candidate sat single-copy for 1h47m earlier in this lane; do not repeat it.
2. **Gate 4A**: fmt, clippy, `-p bridge-core`, hygiene, and the workspace gate **on an idle machine** compared against a same-environment base at `3dadec91`.
3. **Verify the frozen control under BOTH gates** — `cargo test` *and* `clippy -D warnings`. A control that dies on `dead_code` before reaching its red tests proves nothing (this is a real finding from this session, §5).
4. Operator evidence commit → PR → merge on verified green.
5. Dispatch **4B** (constructible, fully refused; cap 350) per the decomposition.

**STOP conditions:** a red test the base control also reddens is pre-existing — report it, do not re-baseline. If the counted total would exceed a sub-slice cap, split before review; caps are a stop boundary and capacity is **not transferable** between sub-slices.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| T3b (6 increments) | **done** | `[MEASURED]` PRs #72–#77; `EXACT_ABSENCE_POLICY_READY_V1 = true` on main |
| Separator guard (carried since T3a) | **done** | `[MEASURED]` PR #78 `018bb930`; defect verified closed on main under both spellings |
| Separator control smells | **done** | `[MEASURED]` PR #79 `3dadec91` |
| R2f1b slices 1–3 | **done** | `[MEASURED]` slice 1 `aedd2c2`; slice 2 = T3a/T3b; slice 3 = 3c2, merged PR #51, ledger discharged #52 |
| **Slice 4 decomposition** | **done** | `[MEASURED]` `462e676b` — 10 sub-slices, 2,610 projected / 3,780 caps |
| **Slice 4A** | **in flight** | `[MEASURED]` implementor `gpt-5.6-sol` xhigh; spec `docs/superpowers/plans/2026-08-23-r2f1b-slice4a-task.md` |
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
| 1 | Slice 4A gate → evidence → PR → merge | next | §1 steps 1–4 | cap 450, base `3dadec91` |
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
| 4A log | `<SCRATCH>/s4a-implement.log` |
| Implementor | `gpt-5.6-sol`, effort `xhigh`, `kind=container_rw`, in `examples/a2a-bridge.r2f1b-impl.toml` |
| Head-to-head config | `examples/a2a-bridge.workflows-sol-xhigh.toml` — `author`/`author-oc`, `review-sol`/`review-oc` |
| Served operator bridge | `:18080` · config `~/Library/Application Support/a2a-bridge/operator/a2a-bridge.toml` |
| Served stockTrading bridge | `:18097` · config `~/.config/a2a-bridge/stockTrading-v10impl-write.toml` |
| ox model ids | `opencode-go/ox-alpha-free` (ACP) · `stealth/ox-alpha` (OpenRouter api) |
| Free disk at write | 189 GiB |

## 7. Refutation verdict and owner questions

**§2c verdict:** NOT RUN — why: interim checkpoint written mid-run for compaction, not a lane handover; 4A's claim-bearing artifact does not exist yet. The pass is owed at 4A's evidence commit. · claim: "D11's internal timers are strictly shorter than their observable bounds, and no timer arms in 4A." · pass: pending — must be INDEPENDENT · evidence tier: pending TEST-BACKED · record: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-handoff.md` (to be written)

**Questions the owner owes an answer to:**
1. **ox-alpha reliability.** Three distinct failures today — provider `network_error`, HTTP 429 rate-limit, and malformed output framing twice. It is a temporary promotional model and may vanish. Keep using it for review (where it earned its place), or move to `opencode-go/kimi-k3`? Non-blocking.
2. **Effort parity.** ox ran at opencode's **unset default** throughout; opencode advertises no effort levels, so `reasoning_effort` is unreachable via ACP. The head-to-heads were therefore not effort-matched. Worth re-running any comparison via the `kind="api"` OpenRouter agent, where effort is reachable? Non-blocking.
3. **Head-to-head #3** was superseded by the slice-4 decomposition exercise. Run a separate implementation comparison later, or consider the series closed at two? Non-blocking.
