# R2f1b 3d T2 — extension repair 2: dispatch declaration (2026-08-16)

Owner authorized a second extension on the parked T2. This brief declares the
round before dispatch, per convergence discipline.

## Why the park was lifted

The park rested on "the extension repair reached its 3-attempt bound with its
own three regressions red." That evidence was re-examined at source and does
not support a design-level failure:

- All three reds are ONE mechanical omission — `unique_temp_dir()` computes a
  path and never creates the directory, so the three new tests construct the
  control root against a nonexistent path. Host-verified: adding
  `std::fs::create_dir_all(&tmp)` to the three takes `bridge-worktree --lib`
  from 270/3 to **273/0**, with fmt and workspace clippy `-D warnings` clean.
- The agent's `a2a-lf` HTTP 403 is the implement-lane egress allowlist working
  as designed (ADR-0013: model APIs only; crates.io deliberately absent). The
  agent had no local compile loop — which is why a trivial harness omission
  survived three attempts. Per the evidence-admissibility discipline, a probe
  that failed for its own reasons yields no evidence about the hypothesis.
- E1, E2's core, and E3 are delivered. `git diff --numstat 435257ce..f66016e0`
  = 736 lines, inside the extension's 500/750 caps (no breach this round).

Per the convergence discipline's no-restart clause, the round continues on
`f66016e0` rather than restarting from `435257ce`: a restart would discard
delivered work with no evidence the artifact is unsalvageable.

## Scope — two findings, both closed

| Id | Class | Finding |
|----|-------|---------|
| R1 | mechanical | three new tests build a control root against a path `unique_temp_dir` never created |
| R2 | **WRONG** | a failing control-root pin permanently orphans the preparation reservation |

R2, proven on the host by driving `arm_nonreturning_control_root_pin`,
removing the control root while blocked, then releasing:

```
owner published before the blocking pin = true    <- E2's core IS delivered
first  configure = Err(StoreFailure)              <- correct
entry retained after failure = true               <- THE DEFECT
second configure = Err(AgentOverloaded)           <- permanent, process lifetime
```

Mechanism: the runner's `root_ready` error arm completes the caller, then calls
`runner_exit_guard.complete()`, disarming `terminalize_preparation_runner_exit`
— the only path that removes the `preparation_flights` entry. Every flight
parked on the same failed pin leaks its own reservation, not just the claimant.
The same arm also completes unconditionally, without consulting the phase, so a
transfer that claimed the terminal during the blocked pin can be completed over
(T-B covers this).

Non-scope, deferred with ledger entries: per-flight blocking waits on the root
pin (**SMELL** — a bounded resource concern naming no incorrect output); s1
abort residue; the slice-4 binding observer obligation.

## Dispatch

- Route: `a2a-bridge implement` (owner rule 2026-08-09), config
  `examples/a2a-bridge.r2f1b-impl.toml` (impl = gpt-5.6-terra @ xhigh — terra
  implements because sol is the review lens), `--lang rust`, `--depth light`
  (the counted re-look is the real gate), `--base-ref
  salvage/r2f1b-3d-t2-extension-candidate` (= `f66016e0`), `--strict-brief`.
- Task spec: `plans/2026-08-16-r2f1b-3d-t2-extension-repair2-task.md`.
- First dispatch was REFUSED by the typed task-spec gate (missing required
  `Acceptance Criteria`) and flagged by brief-lint (`premise-without-license`
  on the 270→273 host result). Both fixed before re-dispatch; the spec now
  carries an explicit falsification license telling the agent to report a
  mismatch rather than force the change.
- The spec states the no-crates.io constraint outright so the agent does not
  spend attempts on an environment it cannot fix.

## Declared cap (before dispatch)

**ONE targeted repair on frozen `f66016e0` + ONE bounded Sol re-look on the
repair delta.** Implementor caps: soft 150 / hard 250 changed lines
(`git diff --numstat f66016e0..HEAD`), production confined to `backend.rs`.

If this round does not converge, T2 goes to **option 2** — re-scope E1/E2/E3 as
their own designed sub-slice. There is no third extension.

## Gates before fold

fmt + workspace clippy `-D warnings` + the full workspace suite on the host at
the exact final head, run unloaded. Known flake classes ledgered by name if
hit — including the whole-bin parallel class observed this round
(`cli_tests::guarded_spawn_ignores_retargeted_static_cwd_for_native_mcp`,
outside T2's diff, passes in isolation on the same tree).

Operator red/green controls run on the host at the exact head, since container
reds have been egress-blocked for several rounds.

## Custody

Bench worktree `.claude/worktrees/t2ext` at `f66016e0` for host verification.
Harness fix + diagnostic probe preserved as a patch in the session scratchpad.
Clone lives under `~/code/.a2a-implement` (reaper-covered). Both prior T2 heads
remain pushed: `435257ce` (`feat/r2f1b-3d-t2`), `f66016e0`
(`salvage/r2f1b-3d-t2-extension-candidate`).

## Execution log

- Dispatch 1: refused pre-clone by the task-spec gate (see above). No clone, no
  spawn, no cost.
- Dispatch 2: running (`--strict-brief` passed; lsp warm-deps ok).
