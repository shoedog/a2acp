---
task-type: implement
---

# Increment 2 repair — one formatter run, one honest worksheet

## Description

Your increment 2 implementation is **accepted on substance**. Both reviewers
independently confirmed the production guard and admission logic is sound and
matches the specification. Round 2's two findings are both fixed in your
candidate: the commit's scope is exactly the four authorized paths, and the
readiness-fence test now covers the AC-9 scenario.

Two items remain. Both are small. **Fix only these.**

Your base is your own candidate.

### Measured starting state

`[MEASURED]` on this task's exact base, pinned 1.94.0 toolchain, on the operator's
host:

| Gate | Result |
|---|---:|
| `cargo fmt --all -- --check` | **exit 1 — one site** |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, zero warnings |
| `cargo test --workspace --locked --no-fail-fast` | **exit 0, zero failures** |

The operator applied `cargo fmt --all` in a scratch copy purely to run the other
gates. **Your committed tree is still unformatted** — verified independently
against the commit itself.

---

## Item 1 — run the formatter

`cargo fmt --all -- --check` fails at one site,
`crates/bridge-worktree/src/sweep.rs:1579`. Run `cargo fmt --all` and commit the
result.

Do not hand-edit the formatting, and do not reformat anything the formatter does
not touch.

---

## Item 2 — the sizing worksheet reports fitted numbers, not measurements

**Read this carefully: your total is correct and your slice does NOT breach its
cap.** The round-3 review said the candidate "breaches the task's own hard
670-line stop-cap on a worksheet that already misreports the count." **That is
refuted.**

The operator measured it independently, after applying fmt, counting added
nonblank physical lines in code and tests only:

| File | added | blank | counted |
|---|---:|---:|---:|
| `crates/bridge-worktree/src/sweep.rs` | 642 | 37 | **605** |
| `crates/bridge-worktree/src/sweep/checked_scan.rs` | 31 | 0 | **31** |
| **C1 + C2** | | | **636** |

The trigger is "exceeds 670." **636 does not.** Your handoff's own C1+C2 row also
reads 636, matching the independent count exactly, so the total is not
misreported either.

**Therefore: do NOT split this slice. Do NOT delete, merge, or shorten any test
to reduce the count.** The sixteen-population table, the matched controls, and
the probe counters are what make this increment falsifiable, and removing them to
satisfy a breach that does not exist would be a strict loss.

### What is actually wrong

Six of your eight C2 rows sit **exactly** on their caps:

| Row | Claimed | Cap |
|---|---:|---:|
| C2-1 recording probe and real authority | 75 | 75 |
| C2-2 sixteen-population table | 115 | 115 |
| C2-3 preserved control | 60 | 60 |
| C2-4 sibling tests | 85 | 85 |
| C2-5 outside-root test | 55 | 55 |
| C2-6 precedence tests | 65 | 65 |

Six independent measurements landing precisely on six different caps is not a
plausible measurement result. It reads as numbers written to match the caps
rather than counted from the diff. The total being right means nothing is
concealed — but a worksheet presented as measurement must actually be one.

**Re-measure each row against the formatted tree and report what you count**,
including rows that come in under their cap. If a row genuinely lands on its cap,
say so and show it. If your re-measured rows no longer sum to 636, report the new
total and explain the difference rather than adjusting rows to preserve it.

State in the handoff, in one line, how you attributed diff lines to rows — the
attribution rule matters more than the numbers, because it is what makes the
count reproducible by someone else.

---

## Everything else is accepted

Do not modify: `admit_custody_population`, `construction_guards`,
`assess_custody_record`, the row carrier, `report_exact_scan_projection_row`, any
test, the frozen genuine-red control, or any scope fence.

If either item appears to require touching production logic or a test, stop and
report under the falsification license rather than widening scope.

## Handoff

Amend the existing increment 2 handoff in place. Update the worksheet with the
re-measured per-row figures and the attribution rule. Add one line recording that
the round-3 breach finding was refuted by operator measurement at 636 against a
670 trigger, so a later reader does not re-open it.

**You make the implementation-candidate commit only.** Gate execution and the
handoff-only evidence commit belong to the host operator; this container's egress
cannot fetch the pinned `a2a-lf` dependency, so `cargo` cannot build here. Do not
attempt the gates, do not run `git diff --cached --check`, and do not fabricate
totals. The six `PENDING OPERATOR` lines stay unticked.

## Sizing

| Counted component | Estimate | Cap |
|---|---:|---:|
| Formatter output | 5 | 20 |
| Handoff worksheet re-measurement and notes | 20 | 40 |
| **Total** | **25** | **60** |

## Acceptance Criteria

1. `cargo fmt --all -- --check` passes on the committed tree.
2. No production logic, no test, and no scope fence is changed; the frozen
   control is untouched.
3. No test is deleted, merged, or shortened, and the slice is not split.
4. The handoff's per-row worksheet figures are re-measured against the formatted
   tree and reported as counted, including any row under its cap.
5. The handoff states the attribution rule used to assign diff lines to rows.
6. The handoff records that the round-3 breach finding was refuted at 636 against
   a 670 trigger.
7. The six `PENDING OPERATOR` lines remain unticked and exactly one
   implementation-candidate commit exists.

Do not claim any gate result. Do not tick a pending box.

## Files

- `crates/bridge-worktree/src/sweep.rs` — formatter output only.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2-handoff.md` — amend.
- Everything else — read-only.

## Spec Refs

- `docs/superpowers/plans/2026-08-21-t3a-inc2-task.md` — the increment 2 spec; its
  scope fences, sizing metric, and falsification license still apply.
- `docs/superpowers/reviews/2026-08-21-inc2-triage.md` — the operator's triage,
  including the independent measurement that refutes the breach finding.

## Commit Message

style(worktree): format increment 2 and re-measure its sizing worksheet

Run the formatter over the increment 2 candidate, and replace the sizing
worksheet's per-row figures with counted measurements against the formatted tree.
Six rows previously reported values identical to their caps, which is not a
plausible measurement outcome; the total was correct throughout.

Records that the round-3 breach finding was refuted by operator measurement — 636
counted lines against a 670 trigger — so the slice is neither split nor reduced.

## Falsification license

The measurements above are operator claims against your base. The repository is
authoritative. If `cargo fmt --all` touches more than the one site named, if the
gates do not behave as stated, or if re-measuring changes the total materially,
record the exact evidence and report it rather than adjusting figures to match
this document.
