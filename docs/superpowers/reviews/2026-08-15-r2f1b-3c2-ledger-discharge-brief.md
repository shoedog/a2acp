# R2f1b 3c2 ledger-discharge slice — dispatch declaration (2026-08-15)

Owner directive: housekeeping → ledger discharge → 3d. This brief declares the
ledger-discharge slice before dispatch, per convergence discipline.

## Scope

Discharge every test-hardening item carried out of 3c2's acceptance ledger, in
one test-only slice on top of landed `main` (`6ad88565`). Ten items:

| Id | Source closure | Item |
|----|----------------|------|
| D-1 | D closure 2 | simultaneous-wrapper two-thread barrier (one inner poll, loser zero row effect) |
| D-2 | D closure 2 | publication-waiter cfg(test) latch replaces 1 s absence window |
| E-1 | E closure 2 | admission-reset state table: `(Complete,false)` re-admits; five terminals refuse |
| E-2 | E closure 2 | bound public-path stale-cell recreation (timeout → recreate → late release → successor live → old `Unknown` aggregated) |
| F-1 | F closure | reqwest poll barrier around real `send()`: `Failed,false` pre-poll / `Partial,true` post-poll |
| F-2 | F closure | refusing + mismatched-publisher cleanup tests → prompt failure + cleanup `Unknown` |
| F2-Z | F2 closure | signal test hermetic: bounded poll with Z-as-terminated + zombie red control |
| G-1 | G closure | configure-clean eviction: `Configure + Ok(Complete)` full-effect assertion |
| A-1 | aggregate re-look | equal-length same-inode commitment corruption → reopen refusal + root-byte preservation |
| watch | landing round | coverage-lane load-flake (`authority_mutation_lock_release_failure_is_loud_not_silent`) — WATCH ONLY, no work unless it recurs |

Non-scope: any production behavior change, 3d work, V3 arming, provider turns,
the fs_custody rustfmt hygiene slice, docs items S2/S3/S4/S8, ops items.

## Dispatch

- Route: `a2a-bridge implement` (owner rule 2026-08-09), config
  `examples/a2a-bridge.r2f1b-impl.toml` (impl = gpt-5.6-terra @ xhigh —
  terra implements because sol was the 3c2 review lens), `--lang rust`,
  `--depth light` (real closure review follows), `--base-ref main`.
- Task spec: `task-ledger-discharge.md` (dispatch workspace
  `/private/tmp/a2a-r2f1b-ledger-discharge.2Oo9Ii`, mirrored beside this
  brief).
- Caps declared to the implementor: soft 600 / hard 800 changed lines,
  test-only; unfitting items reported, not crammed.
- Bridge binary: fresh release build from exact `main` `6ad88565`
  (scratchpad worktree), replacing the reaped prior build.
- Preflight: egress stack up (`a2a-egress-proxy`, `a2a-verify-proxy`); no
  competing bridge run (single-token-family constraint); 194 Gi disk free.

## Review plan and cap (declared before dispatch)

- Internal implement-review: advisory (`--depth light`).
- Counted review: ONE Sol closure round (`run-workflow code-review`,
  established lens) on the full diff. Cap: one round, plus at most one
  targeted repair on closed enumerable findings. Open-class findings park the
  slice and escalate. Red-first evidence per test is an acceptance criterion,
  not a reviewer courtesy.
- Gates before fold: fmt + workspace clippy `-D warnings` + workspace suite on
  the host at the exact final head; known hermetic container classes ledgered
  by name if hit. Landing: branch + PR onto main, CI green, rebase merge
  (repo precedent).

## Custody

- Dispatch workspace holds the spec; clone lives under `~/code/.a2a-implement`
  (reaper-covered). Salvage branch on rejection, per lane practice.

## Execution log (2026-08-16)

- Dispatch 1 (impl-35732-yre4mrr8): EMPTY — cloned stale local `main`
  (42249b3d; the ref lives checked out in `.claude/worktrees/fold` and the
  3c2 merge never moved it). Terra found no target files, made no changes.
  GOTCHA ledgered: verify `local main == origin/main` before any
  `--base-ref main` dispatch. Fix: ff inside the fold worktree → `6ad88565`.
- Dispatch 2 (impl-36634-o9ndkzwa, base `6ad88565`): 3-attempt bound reached
  with candidate `69f144f1` — ALL TEN items delivered, per-item mutation
  evidence in the commit message; in-container verify red on exactly one
  assertion (original F-1), internal light review REJECT with a correct
  mechanism diagnosis.
- Operator adjudication: the F-1 before-case constructs a SILENT-DROP
  schedule; D's zero-poll privilege (rrf:1551 acceptance from
  `provider_send_armed`; :1562 privilege comment) makes the conservative
  possibly-accepted publication CORRECT production behavior; the literal
  `Failed,false` reading of F's DEFER applies to the wrapper-path window the
  passing siblings already pin. Operator completion `ab911ae5` splits the
  marker-bit assertion (false — fence held, zero POSTs) from the publication
  claim (true — armed-row custody), flagged for the closure to sustain or
  overturn.
- Test-only audit (operator): every production-region hunk across the five
  files is `#[cfg(test)]`-gated or in-test-module; workspace clippy
  `-D warnings` green proves no leakage. Terra's in-turn crates.io
  CONNECT-403 = egress lock by design (verify cache compiled everything);
  NOT a proxy-degradation event.
- Gates on exact `ab911ae5`: fmt clean; workspace clippy `-D warnings`
  clean; full suite **4,111/0/13 across 90** (3c2 baseline 4,104/0/13).
- Counted Sol closure DISPATCHED (sol/max via `run-workflow code-review`,
  solmax config; brief + diff at
  `/private/tmp/a2a-r2f1b-ledger-discharge.2Oo9Ii/closure-brief.md`).
