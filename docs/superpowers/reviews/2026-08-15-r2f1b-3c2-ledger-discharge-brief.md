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
