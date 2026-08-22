# Handoff — R2f1b 3d T3b (candidate settlement): slice 1 dispatched and in flight, slices 2–5 unauthored

**Written:** 2026-08-21T00:00Z · **By:** session `7de137cd-c346-425d-a3b8-e405d2325941` · **Provider:** claude
**Workspace:** `a2a-bridge` · `agent/r2f1b-pre-slice2-custody-plan` · **Measured state:** `[MEASURED]` HEAD `6e54817b7ddf89dcdfed5d1aeaf8bfda0ed198b3` · Tree CLEAN · Probe `git rev-parse HEAD; git status --porcelain` · Output inline (0 dirty files, 0 unpushed)
**Predecessor:** `docs/superpowers/2026-08-19-r2f1b-3d-t3a-HANDOFF.md` (T3a lane; T3a is now COMPLETE)
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` `pgrep -f "a2a-bridge implement"` returns one live PID; this session owns it. No other agent is in this lane. — **RESOLVED (single owner, 2026-08-21)**

**(b) Custody exposure** — `[MEASURED]` `git status --porcelain` = 0 files; `git log origin/<branch>..HEAD` = 0 unpushed. Everything authored this session is pushed. The **only** single-copy artifact is the in-flight container's work product, which lives inside the quarantine clone and is not yet a commit — see (c). — **RESOLVED for committed work; the in-flight product is covered by (c)**

**(c) In flight / irreversible** — `[MEASURED]` Slices 1 and 2 are merged (`c65c8eca`, `bae894e3`). **T3b slice 3 `implement` is RUNNING**; log `<SCRATCH>/t3b3-implement.log`. Effect-free by design at the primitive level, and it adds no custody-layer caller. If it dies, fetch the candidate out of its clone before anything else. — **OPEN until the run terminates**

**(d) Authorization granted but not exercised** — the owner's standing instruction for this turn, verbatim: **"merge slice 1 when green then dispatch slice 2"**. A successor may merge slice 1 on a green gate + green CI **without re-asking**, and may then dispatch slice 2. Slice 2 must be **authored first** (§4 #2) — the authorization covers dispatching it, not skipping its spec.

## 1. Resume order

1. **Wait for slice 1 to terminate.** `pgrep -f "a2a-bridge implement"`; log at `<SCRATCH>/t3b1-implement.log` (§6). Do not poll tightly — use an until-loop.
2. **Run the host gate with a same-environment control.** Base control is `bridge-worktree` **331 passed** at `cafeae13`. Run the base in the *same* environment that produces the candidate's numbers — a green from a different environment is not a control.
3. **Verify the frozen mutation control.** Apply `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch` to **slice 1's own head** (not the base), confirm the recorded SHA-256, and confirm it reddens **exactly one** test — `the_window_refuses_a_record_that_changed_between_its_two_reads` — and nothing else. Read the actual output; exit status is not behavioural evidence.
4. **Bounded effect audit.** Confirm no edge from the added path to rename, unlink, publication, transition, settlement, provider removal, prune, or process spawn. Confirm no `acquire_*_blocking_in` in `settle.rs`, and guard-field declaration order such that drop releases custody-then-publication.
5. **Fill the six `PENDING OPERATOR` lines** in `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-handoff.md`, then make the **handoff-only evidence commit** (two-commit custody: implementation candidate, then docs-only evidence). Provisional `git diff --cached --check`, restage, final check with no edits after.
6. **PR → CI green → merge.** Then update §2 of this file and the roadmap.
7. **Author slice 2** against slice 1's *merged head* (§4 #2), then dispatch.

**STOP conditions:**
- Slice 1's gate is red on a test the base control also reddens → that is a pre-existing failure; report it, do not re-baseline and do not silently fix it.
- The mutation control reddens zero tests, or more than one → the control is not doing its job; the evidence is inadmissible and slice 1 does not merge on it.
- The counted total exceeds the **790** cap → stop and report; do not trim tests to fit.
- A spec anchor turns out false → the implementer has a falsification license and will stop. Fix the spec, do not argue it through.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| T3a increment 1 slice A2a-1 (one checked-scan engine) | done | `[MEASURED]` PR #62, merged |
| T3a inc1 A2a-2 (characterization) | done | `[MEASURED]` PR #63, merged |
| `implement` commit-failure surfacing | done | `[MEASURED]` PR #64 `a8b13fe6`, merged |
| T3a inc1 A2b (return the sweep report) | done | `[MEASURED]` PR #65 `9ce2074e`, merged |
| T3a slice B (root observations from retained descriptor) | done | `[MEASURED]` PR #66 `bade9866`, merged |
| T3a increment 2 (admit candidate populations, type placement guards) | done | `[MEASURED]` PR #67 `84a48a4c`, merged. Cap breach accepted by owner with disclosed extension. |
| T3a 3A-1 (repository-authority behind the probe) | done | `[MEASURED]` PR #68 `b73e0a5a`, merged |
| `implement` staging-detection defect (`stage_state` porcelain trim) | done | `[MEASURED]` PR #69 `be6df2e4`, merged. Root cause: `git_ok` trims, eating porcelain's index column, so ` M` read as `M`. |
| T3a 3A-2 (type claim-authority failures) | done | `[MEASURED]` PR #70 `f7e2e8e2`, merged |
| T3a 3B (retain and bracket custody-root identity) | done | `[MEASURED]` PR #71 `cafeae13`, merged. **T3a is COMPLETE.** |
| T3b plan (Opus + sol, two-author) | done | `[MEASURED]` `docs/superpowers/plans/2026-08-22-t3b-plan-OPUS.md`, `-SOL.md` |
| T3b slice 1 spec | done | `[MEASURED]` `docs/superpowers/plans/2026-08-22-t3b-slice1-task.md`, committed `6e54817b` |
| **T3b slice 1 implement** | done (REJECT at bound, adjudicated) | `[MEASURED]` candidate `1efb3154`; evidence commit `71c7a3cf`; **PR #72**. 3 rounds: R1 build-broken → R3 one docs-provenance defect. |
| T3b slice 1 operator gates | done | `[MEASURED]` all 5 exit 0 at `1efb3154` from a trusted cwd root; workspace 0 FAILED/91 binaries; `bridge-worktree` 338 passed |
| T3b slice 1 base control | done | `[MEASURED]` `bridge-worktree` @ `cafeae13` = 331 passed, same host/command. Delta **+7** |
| T3b slice 1 frozen control | done | `[MEASURED]` SHA-256 matches; applies to real head; reddens exactly `the_window_refuses_a_record_that_changed_between_its_two_reads` |
| Round-3 BLOCKER (stale control sha) | done | `[MEASURED]` CONFIRMED unreachable; corrected in `71c7a3cf`. Root cause = spec, see §5 |
| Round-3 MAJOR (dead check) | done | `[MEASURED]` **REFUTED** — deleting it reddens 1 test (337/1) |
| Round-3 MAJOR (red bridge-api test) | done | `[MEASURED]` **REFUTED** — no dep edge, byte-identical `backend.rs`, `Elapsed` timeout, candidate green on container re-run |
| T3b slice 2 (re-prove gate) | next | Spec **not written.** Rebinds to slice 1's merged head. |
| T3b slices 3–5 | pending | Planned only; see `2026-08-22-t3b-plan-OPUS.md` §4 |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| `docs/reliability-execution-roadmap.md:1247` | Header says "T3a carried ledger — **three** lane items", recorded "after T3a increment 1, slice B, increment 2, and 3A-1 merged (PRs #60, #62-#68)" | `[MEASURED]` T3a is now COMPLETE at `cafeae13` (PRs #70, #71 also merged). The ledger's *rows* are still accurate; its *recorded-at* line is stale by two PRs. Not yet corrected — **this is a work item** (§4 #6). |
| `docs/reliability-execution-roadmap.md` `force_next_release_failure_for` row | "Dormant across **seven** consecutive PRs (#62-#68)" | `[MEASURED]` now dormant across **ten** (#62–#71), all green on first run. Strengthens the close-as-dormant proposal. Not yet corrected — §4 #6. |
| Memory `r2f1b-custody-plan-rev2-review.md` | Says T3a "unblocks T3a rebuild + T2 control-root" | `[INHERITED]` (memory index line, not re-read this turn) T3a is complete; the lane is now T3b. Memory needs a T3b entry. §4 #7. |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | T3b slice 1 | **done** | Merged `c65c8eca` (PR #72) | — | 415/790 lines |
| 1b | T3b slice 2 | **done** | Merged `bae894e3` (PR #73), after a 1-round repair | — | 711/770 lines; 347 passed |
| 1c | T3b slice 3 | **in flight** | Gate → evidence → PR → merge | run terminating | base `bae894e3`; cap 740, projection 500 |
| 2 | T3b slice 4 (candidate settlement — **destructive**) | pending | Author against slices 1–3 merged. First slice that renames/unlinks for real. | slice 3 | cap 790 |
| 3 | T3b slice 5 (boot wiring, legacy markers, readiness) | pending | **Binding obligation carried from slice 2:** the settlement caller MUST supply a read-only probe; wiring a spawning probe is a slice-5 blocker. Readiness flip is its own commit. | slice 4 | cap 790 |
| 4 | T3b slice 4 (candidate settlement — **destructive**) | pending | Author after slice 3. First slice that renames/unlinks. | slices 1–3 | cap 790 |
| 5 | T3b slice 5 (boot wiring, legacy markers, readiness flip) | pending | Readiness flip is its **own commit** with its own frozen control. | slice 4 | cap 790; flips `EXACT_ABSENCE_POLICY_READY_V1` |
| 6 | Refresh roadmap T3a ledger (rows in §3) | pending | Edit `docs/reliability-execution-roadmap.md:1247` header + flake row | — | — |
| 7 | Write a T3b memory entry | pending | New file + `MEMORY.md` pointer | — | — |
| 8 | Unix-only separator guard in `is_custody_record_name` | parked | Its own small slice **after T3b** — it sits in code T3b is still changing | T3b complete | latent: CI runs `bridge-store` on Windows only |
| 9 | F8 ext4 birthtime observation | parked | Opportunistic: capture the `SLICE-B-F8` line via `--nocapture` on Linux/ext4 | needs a Linux lane | expected `none/none/none → Pinned` |
| 10 | `force_next_release_failure_for` flake | parked | **Propose closing as dormant** (10 clean PRs) | owner ruling | `compatibility_schedule_state.rs` |
| 11 | Operator config swap window | parked | Staged, unapplied | — | — |

## 5. Invariants and traps — do not do these

- **Never demand a behavioural red against `cafeae13` for slice 1** — no symbol it touches exists there, so any control naming `settle` is a compile error, and this lane has already root-caused compile-error "reds" as non-evidence. Slice 1's control is a **single-mutation control against its own head**. That was a deliberate design choice, not an oversight.
- **Never trust `grep -cE '^\+[^+]'` and then subtract blank lines** — that pattern already excludes them. Double-subtracting produced 636 instead of 673 and made me **refute a correct cap-breach finding**. The implementer caught it and escalated.
- **Never extend a short hash into a 40-char SHA.** I wrote one by inventing characters; only `implement --base-ref` rejecting it caught the fabrication. Copy shas verbatim; never type one.
- **Never slice a spec table by line number.** I cut sol's mapping table from its *third* row, dropping the header. The agent refused, correctly.
- **Never anchor a spec on line numbers.** `--strict-brief` warns R3 on every one; four dispatches in this lane have carried the warning. Symbol anchors cost nothing and don't drift. One R3 WARN still survives at slice 1's spec line 29 (WARN, not VIOLATION — it passed).
- **Never assert a fact about the base in an acceptance criterion.** "These six tests exist" is a fact assertion `--strict-brief` rejects under R4 — and it was *false*, since `settle.rs` doesn't exist at `cafeae13`. Phrase ACs as imperatives.
- **Never conclude "one blocker" from one compile error.** A non-compiling crate means the defect count is **unknown**. In 3B, fixing the one visible mis-scoped path revealed two more first-execution failures.
- **Never count tests by grepping `^\s+fn [a-z_]+\(\)`** — it matches helpers like `decoded_custody()`. Count `#[test]`. This caused a null dispatch.
- **Never treat a self-failed probe as evidence.** Real instances this lane: `rustfmt --out-dir` (no such flag — every block read as DIFF); a test filter matching 0 tests and exiting 0; unbraced `$C:` eating paths; `git fetch … | tail && echo OK` (the `&&` bound to `tail`); tests failing only because the worktree sat under `/private/tmp`.
- **Never FF-forget the fold worktree.** Sol authored an A2 spec claiming inspection it could not have done because its worktree was 3 merges behind. FF the worktree before every authoring dispatch — this is now a standing pre-dispatch probe.
- **Never assume three green environments means three environments were tested.** macOS/APFS and the container's overlayfs both passed a fixture only ubuntu/ext4 caught (inode reuse after unlink+recreate).
- **Summing `test result:` lines over-counts** (nested filtered subprocess). Trust exit status + FAILED count.
- **Never hand-roll the verify container.** Three probes in a row died of their own setup: mounting at `/src` made `r2b1_diagnostics` treat every `tests/*.rs` as production (it flags any path component literally named `src`); `rustdoc` never launched, giving exit 101 with zero test failures; and a shared `/cache` target dir served a cached test binary with `CARGO_MANIFEST_DIR` baked to the *previous* mount path. Reproduce verify by running the pipeline, or accept that ad-hoc runs need their own controls.
- **Run host gates from under `/Users/wesleyjinks/code`.** The 23 `r3d0_*` tests hard-fail in 0.03s from a scratchpad checkout: "current checkout must be an existing directory under the owner-approved trusted cwd root". Not a regression — a location precondition.
- **The implement loop AMENDS, so a spec must never ask the implementer to record its own final head sha inside the artifact.** It is rewritten by the next amend. This cost slice 1 two of its three review rounds. The binding goes in the operator's evidence commit.
- **A wedged `implement` process can outlive its own completion.** Slice 1 finished and then sat at 0% CPU for 9.4h. Check the log tail for `loop:`/`review:` terminal lines before assuming it is still working; the work product is a commit in the clone and should be fetched out immediately.
- **The source plan is not a verified anchor set.** Slice 3's authoring falsified three plan claims, one load-bearing: the plan says `is_custody_record_name` rejects the `.a2a-v2-rtc-` retirement residue, and an executed probe returns `true` — the reserved name is `prefix ++ target`, the target already ends in `.custody.v1.json`, so the prefix is absorbed into the stem. Verify every plan claim by execution before writing it into a spec.
- **Do not write an unsatisfiable contract.** Slice 2 was rejected partly because its task required routing through machinery that takes a probe while banning process-spawn edges; the production probe shells out to `git rev-parse`. Two slices in a row lost a round to an operator-authored contradiction — when a lane keeps losing rounds to one class, suspect the contract.
- **A CI waiter must guard against the pre-registration race.** `gh pr checks | grep -c pending` returns 0 before jobs register, so a naive until-loop exits instantly. Require a minimum completed-job count as well.
- **Check free disk before dispatching.** The floor is 50 GiB; a retry was blocked at 39. `[MEASURED]` now 56 GiB after reaping 18.8 GiB of build targets. Dry-run inspect before reaping.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Branch | `agent/r2f1b-pre-slice2-custody-plan` |
| Branch HEAD | `6e54817b7ddf89dcdfed5d1aeaf8bfda0ed198b3` |
| `origin/main` (slice 1 base) | `cafeae13a67b621f194e4cb82b2fc6765f4e8b4a` |
| Scratchpad (`<SCRATCH>`) | `/private/tmp/claude-501/-Users-wesleyjinks-code-a2a-bridge/7de137cd-c346-425d-a3b8-e405d2325941/scratchpad` |
| Slice 1 log | `<SCRATCH>/t3b1-implement.log` |
| Slice 1 quarantine clone | `/Users/wesleyjinks/code/.a2a-implement/impl-92607-lz719c13` |
| Slice 1 lsp cache volume | `a2a-impl-lsp-cache-8ac4ca8ed1db0dde` |
| Slice 1 dispatch (verbatim) | `a2a-bridge implement --input docs/superpowers/plans/2026-08-22-t3b-slice1-task.md --repo /Users/wesleyjinks/code/a2a-bridge --base-ref cafeae13a67b621f194e4cb82b2fc6765f4e8b4a --config examples/a2a-bridge.r2f1b-impl.toml --depth thorough --lang rust --strict-brief` |
| Binary used | `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/target/debug/a2a-bridge` |
| Implementor | `gpt-5.6-terra`, `effort = "xhigh"`, via `codex-acp` |
| Reviewers | Sonnet (`claude-agent-acp`) + gpt-5.5 — deliberately different models |
| Slice 1 frozen control | `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch` |
| Slice 1 evidence handoff | `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-handoff.md` |
| Base control | `bridge-worktree` **331 passed** at `cafeae13` |
| Plans | `docs/superpowers/plans/2026-08-22-t3b-plan-OPUS.md`, `-SOL.md`, `-authoring-input.md` |
| Predecessor handoff | `docs/superpowers/2026-08-19-r2f1b-3d-t3a-HANDOFF.md` |
| Free disk at write | 56 GiB (floor 50) |

## 7. Refutation verdict and owner questions

**§2c verdict:** NOT RUN — why: this is an **interim checkpoint** written mid-run for compaction, not a lane handover; slice 1's claim-bearing artifact (its handoff and control) does not exist yet because the run has not terminated. The pass is owed at slice 1's evidence commit, against the claim below. · claim: "The settlement window refuses in both contention orders and binds to exactly one custody record by descriptor, with no edge to any mutating operation." · pass: pending — must be INDEPENDENT (the gate operator, not the implementer) · evidence tier: pending TEST-BACKED (mutation control + both-order contention matrix) · record: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-handoff.md` (to be written)

**Questions the owner owes an answer to:**
1. `force_next_release_failure_for` — close as dormant on 10 clean PRs (#62–#71)? Standing proposal, no ruling yet. Non-blocking.
2. Slice 3 has no code dependency on slices 1–2 and may run in **parallel**. The plan sequences it third so review order matches risk order. Take the throughput or keep the ordering? Non-blocking; default is to keep the plan's ordering.
