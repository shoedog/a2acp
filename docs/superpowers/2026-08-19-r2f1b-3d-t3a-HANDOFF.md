# Handoff — R2f1b 3d T3a: A1 landed, A2 next; bridge observability repaired mid-lane

**Written:** 2026-08-19 · **By:** session_012SUgPvVNbwpuGQKxWtxLgr (Claude Opus 5, 1M) · **Provider:** claude
**Workspace:** a2a-bridge · agent/r2f1b-pre-slice2-custody-plan · **Measured state:** `[MEASURED]` main `9ce2074e` · T3a increment 1 COMPLETE · Tree CLEAN · Probe `git status --porcelain && git rev-parse HEAD` · Output inline this turn
**Predecessor:** the 2026-08-17 3d handoff (`docs/superpowers/2026-08-17-r2f1b-3d-HANDOFF.md`), superseded for everything below
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — no other session/agent alive in this lane. `[MEASURED]` `pgrep -fl "a2a-bridge (implement|run-workflow)"` → empty. — **RESOLVED** (this turn)

**(b) Custody exposure** — none. `[MEASURED]` tree clean, 0 unpushed commits on the planning branch, 0 open PRs (all six merged). Every artifact branch pushed to origin. — **RESOLVED**

**(c) In flight / irreversible** — nothing running. `[MEASURED]` no implement/run-workflow processes. The operator serve is UP as PID 23161 on `:18080`, running release `ee3b5966ad3b35ef` — that is the intended steady state, not an in-flight operation. — **RESOLVED**

**(d) Authorization granted but not exercised** — owner said, verbatim: *"add docker to allowed_cmds and stage both"* and *"prepare the candidate build and store backup for the swap"*. The config candidate is STAGED and validated but **NOT APPLIED**; the swap window has not been opened. Also standing, verbatim: *"use inference and assumptions as hypothesis but to always prove or disprove them"* — framed as precision, not slowness.

## 1. Resume order

1. **T3a increment 1 is DONE.** The next substantive work is increment 2 or slice B per the 3d brief; neither is specced. The open items below are independent and none of them blocks that.
2. **Open: the index-emptying mystery.** `[MEASURED]` PR #64 made a failed implement commit legible and non-destructive but did NOT diagnose what empties the index between `stage_state` and `host_commit`. `decide()` maps `DirtyUnstaged` to a friendly message and that path did not fire. Two candidates are named in `reviews/2026-08-20-implement-commit-failed-nothing-staged.md`; neither is proven.
3. **Open: the Unix-only separator guard.** `is_custody_record_name` guards the empty-basename case by testing only the forward slash, so a backslash-spelled path diverges from the Unix spelling. Characterized by A2a-2 and deliberately unrepaired; it needs its own slice.
4. **Open: the `force_next_release_failure_for` flake.** It has not fired across four consecutive PRs (#62-#65). Still unfixed; possibly worth closing as dormant rather than spending a slice.
5. The operator config swap window (worktrees + container writer) remains staged and unapplied — see §4.

**STOP conditions:** a spec that fails two counted review rounds → park and escalate, do not fold a third time by hand (§5). An implement run that returns "made no changes" → read the agent's message, which is now printed; do not theorise. Any instruction naming a path outside the repo in a container-bound spec → unsatisfiable, fix the spec.

## 2. State ledger

| Item | State | Evidence |
|---|---|---|
| main | `9ce2074e` (A2b, PR #65) | `[MEASURED]` |
| path-identity primitive | **LANDED** | PR #57 → `9aedf175`; 8 defects closed, closure APPROVE 0W/0S |
| CI uninstrumented control | **LANDED** | PR #56 → `1f14342a` |
| h2 RUSTSEC-2026-0258 | **LANDED** | PR #58 → `c14813b7` |
| agent transcript persistence | **LANDED** | PR #59 → `ee3b5966` |
| T3a inc1 slice **A1** | **LANDED** | PR #60 → `13dfcc27`; 698 lines vs 700 cap; review APPROVE 0 findings; host gate 4,163/0/13 |
| agent reply surfacing | **LANDED** | PR #61 → `c637e493`; host gate 4,169/0/13 |
| operator serve | **SWAPPED, RUNNING** | PID 23161, release `ee3b5966ad3b35ef`, binary-only swap, all 5 post-swap checks passed |
| operator config candidate | **STAGED, NOT APPLIED** | `operator/a2a-bridge.candidate.toml`, SHA `67f3bf01…`; validate pass, doctor 31 ok/1 warn/0 fail |
| T3a increment 1 | **COMPLETE** | A1 #60 -> A2a-1 #62 -> A2a-2 #63 -> A2b #65. `sweep_orphans_with_exact_absence` returns the report; all four A1 `dead_code` allowances consumed; readiness still `false` |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| My own investigation `reviews/2026-08-18-a1-dispatch-no-changes-investigation.md` | Two hypotheses for the null dispatch: spec too large / pre-edit cap fired | `[MEASURED]` **Both wrong.** The spec told the agent to read `~/.claude/handoff-template.md`; in-container `HOME=/root` and only the code tree is mounted, so it does not exist. The agent refused correctly. Recovered verbatim from the persisted rollout. |
| My claim that "the served bridge cannot author code" | Drawn from `implement --help` lacking `--serve` | `[MEASURED]` **False.** `submit --url --agent --cwd --mode --input` authors code and prints the reply. What has no served path is the implement *pipeline* (quarantine clone, verify, review, commit, hand-off). |
| My claim that the serve has "0 workflows" implying no capability | Config count, not capability | `[MEASURED]` The serve fully supports workflows: `run-workflow --serve`, `submit [skill]`, and `agent_card()` advertises one skill per configured workflow id. The operator instance simply configures none. |
| `docs/superpowers/2026-08-17-r2f1b-3d-HANDOFF.md` §8/§9 | Describes T3a as blocked on the primitive | Superseded: primitive landed, A1 landed, A2 is next. |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | **T3a inc1 slice A2** | spec v1 in review | Harvest spec-review round 1; dispatch its findings + the operator findings to sol via `author`; re-extract, re-verify, commit v2 | round 1 in flight | spec `docs/superpowers/plans/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2-task.md`; operator findings `docs/superpowers/reviews/2026-08-19-a2-spec-operator-findings.md` |
| 2 | Operator config swap | staged | Open a pause window; re-take the drained store backup; apply `a2a-bridge.candidate.toml`; verify; update SERVICE.md + manifest | owner window | manifest §"Staged follow-on" |
| 3 | `force_next_release_failure_for` flake | open | Fix or quarantine the 3-test group | none | `bin/a2a-bridge/src/compatibility_schedule_state.rs` ~1538/1575/1608 |
| 4 | Cap rule change (owner-proposed) | agreed, not written | Write into steering: literal byte-for-byte spec lines exempt; count logical LOC for the rest | none | owner turn 2026-08-19 |
| 5 | G1: `[worktrees]` is a boot flag | open | Make it per-request like every other overridable knob | none | `main.rs` `resolve_worktree_runtime_cfg` |
| 6 | G2: worktree-in-container unsupported | open | Mount a host-made worktree as the container `:rw` target | none | `workflow_planner.rs:58` |
| 7 | T3b | pending | Consumes A2 | #1 | slice-3 brief |

## 5. Invariants and traps — do not do these

- **Never hand-fold review findings into a spec.** Measured across this lane: rounds authored by sol converged (9→5→2→1→0 gating); the single round the operator hand-folded regressed 5→7, and four of those seven were self-contradictions introduced by the fold. Dispatch findings to sol via the `author` workflow and extract between `<<<BEGIN ARTIFACT>>>` markers.
- **Never put a path outside the repo in a container-bound spec.** `HOME=/root`; only the code tree and `auth.json` are mounted. Host-side operator obligations (e.g. "use the installed handoff template") do NOT cross the container boundary.
- **Never conclude a capability is absent from an interface's absence.** Three instances this session (§3). Absence of a flag is evidence about that flag only.
- **Never trust exit status over the artifact.** Four instances: `${PIPESTATUS[0]}` is bash-only and this shell is zsh; a trailing `echo` masked a failing dispatch; `implement` returned `Ok(())` on a refusal; a test insert silently landed nowhere ("0 passed; 1090 filtered out").
- **Never re-run a failed CI job before capturing its log** — the re-run replaces it.
- **Never mutation-skip a load-bearing test.** This lane has shipped four tests that proved less than they claimed. Revert the fix, confirm the test reds.
- **Never count test totals by summing `test result:` lines** — a bridge-core test re-executes the binary as a filtered subprocess and inflates the sum by one. Count `Running` binaries + `Doc-tests` suites.
- **Never stop the operator serve without checking `pane_current_command` first.** Its tmux pane runs the binary directly, so `C-c` ends the session; you then need `tmux new-session`, which may be permission-gated.
- **Never write a fact into a spec that the task does not need.** `[MEASURED]` A falsification license makes EVERY stated fact load-bearing, so decorative precision is pure liability. Two dispatches were burned on operator line-count anchors (199/108/110 vs the true 218/107/111) that told the implementer nothing it could not measure itself. Root causes were an awk boundary left over from an exploratory command, and an off-by-one on the first `#[test]` line. State only what the work depends on.
- **Never assume the implement container can run the gates.** `[MEASURED]` Its egress denies the pinned `a2a-lf` dependency, so `cargo` cannot build there; the harness verify container CAN, because it has a warmed read-only `CARGO_HOME`. Gates are operator-owned: the implementer marks them `PENDING OPERATOR`, the operator runs them on the host and makes the handoff-only evidence commit. Spec v2 had this split; v3/v4 lost it and one dispatch was rejected for it.
- **Never accept a review finding without probing it when it is mechanically checkable.** `[MEASURED]` A2 round 2 raised a BLOCKER claiming rustfmt would rewrite the normative seam block. Under the pinned 1.94.0 toolchain, `rustfmt --check` exits **0** on the v2 block and **1** on the v1 block — the probe discriminates, and the reviewer was re-raising a fixed finding against the fixed artifact. Folding it would have reintroduced the round-1 defect. Reviewers regress too.
- **Never dispatch an authoring or review run without checking what commit its `--session-cwd` is actually on.** `[MEASURED]` 2026-08-19: `.claude/worktrees/fold` sat at `9aedf175` while the A2 spec was authored against a stated base of `c637e493`; `sweep/report.rs` does not exist at `9aedf175`, so the agent could not have inspected the surface it claimed to inspect (it restated the authoring input, which supplied the same facts). Caught before the review round only because the same probe was run for the reviewer. Local `main` had silently lagged three merges behind `origin/main`. Probe: `git -C <cwd> rev-parse HEAD` and confirm one file the work depends on exists there.
- **A backgrounded `nohup ... &` reports exit 0 the instant it detaches.** `[MEASURED]` the spec-review task notified "completed (exit code 0)" while PID 95068 was still mid-run with both lenses started. Confirm with `pgrep -fl` and the presence of the `--out` artifact, never the notification.
- Container `verify: PASS` is not host-green. Run the host gate.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| main | `9ce2074ef2a4e7b7bb81b9561b79ba672f9db9db` (A2b, PR #65) |
| A2 spec v1 | `4234517d` · `docs/superpowers/plans/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2-task.md` |
| A2 operator findings | `904c89fb` · `docs/superpowers/reviews/2026-08-19-a2-spec-operator-findings.md` |
| fold worktree | `.claude/worktrees/fold`, branch `main`, at `c637e493` |
| planning branch | `agent/r2f1b-pre-slice2-custody-plan` = `904c89fb` |
| A1 artifact | `feat/r2f1b-3d-t3a-inc1-sliceA1` = `b28c1aef` |
| operator serve | PID 23161, `:18080`, release `ee3b5966ad3b35ef` |
| staged config candidate | `operator/a2a-bridge.candidate.toml` SHA `67f3bf0183f9e6db3654fe56dbd6f29f6bb0d58f5c855c76d1190956c11c3d7a` |
| drained store backup | `releases/ee3b5966ad3b35ef/pre-swap-store/tasks.drained-pre-swap.sqlite` SHA `37ead98a…` |
| author workflow config | `examples/a2a-bridge.workflows-sol-xhigh.toml` (sol xhigh; `author`, `design`, `spec-review`, `plan-review`) |
| implement invocation | `./target/debug/a2a-bridge implement --input <spec> --repo /Users/wesleyjinks/code/a2a-bridge --base-ref <ref> --config examples/a2a-bridge.r2f1b-impl.toml --depth thorough --lang rust --strict-brief` |
| author invocation | `./target/debug/a2a-bridge run-workflow author --input <spec> --session-cwd .claude/worktrees/fold --config examples/a2a-bridge.workflows-sol-xhigh.toml --out <raw> --strict-brief` |
| candidate worktree | `/Users/wesleyjinks/code/.a2a-candidate-ee3b5966` (detached at `ee3b5966`) |

## 7. Refutation verdict and owner questions

**§2c verdict:** REFUTED — corrected in place · claim: "the null A1 dispatch failed because the spec was too large (1,543 lines vs 192/225/380 for specs that worked)" · pass: INDEPENDENT (the persisted rollout, read after PR #59 landed) · evidence tier: TEST-BACKED · record: `docs/superpowers/reviews/2026-08-18-a1-dispatch-no-changes-investigation.md`
<!-- The size correlation was real but not causal. The agent was blocked on a host path invisible in the container, and said so on both null runs. The investigation doc records the correction; the size hypothesis must not be revived without new evidence. -->

**Questions the owner owes an answer to:**

1. Non-blocking — when to open the config swap window (#4 item 2). Nothing depends on it; A2 proceeds either way.
2. Non-blocking — is the `force_next_release_failure_for` flake worth its own slice now? It has cost a re-run on 2 of the last 3 PRs.
