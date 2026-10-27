# H2 flake investigation

Base: `21ab36ae906af2d224532c7c18b4ccdf67626553`, Linux aarch64 container,
2026-10-27. Host behavior and live-provider compatibility were not tested.
All adapters/backends were local fakes; no provider turn was spent.

Hypotheses, confirming observations, and alternatives were written to
`/tmp/h2-hypotheses.md` before source edits or probes. Each measured population
contains 50 independent executions, including every failure, without retries.

## Method and setup controls

Build with `CARGO_INCREMENTAL=0 cargo nextest run --workspace --locked --no-run`;
get exact binary/test identities with `cargo nextest list --workspace --locked
--message-format json`. `/tmp/h2-probe.py` invokes each test binary with
`--exact <name> --nocapture`, captures its output/exit status, and joins all work.
Rust 1.94.0 and nextest 0.9.146 were used. For format/lint commands, the installed
toolchain bin directory was prefixed on PATH because cargo-fmt/cargo-clippy
proxies are absent from the default PATH.

Fifteen owned Python CPU-burn processes run during each measured population and
are terminated/joined afterwards. Concurrency per fixture: fd test 4, other CLI
and smoke/reaper tests 8, custody tests 50. Post-fix uses the same settings. The additional headroom fixture uses concurrency
8 with its existing eight row threads, in both its pre-fold and post-fold probe.
These repetitions are experiments, not nextest retries; nextest retries remain 0.

Two preliminary batches were interrupted before any test completed. Fifty
concurrent 390 MiB debug-binary snapshots exceeded this container's 18 GiB RAM;
a subsequent four-process batch still encountered the first batch's kernel swap
work. Only recorded owned process trees were terminated. These interrupted
attempts are excluded from flake counts. After recovery, `strip --strip-all`
was applied to the generated CLI and six selected test binaries for both measured
base and post-fix probes. No source behavior was changed by stripping symbols.

The first custody population hit 50/50 setup errors per test:
`Git(UnsupportedVersion("Git version exited Some(129)"))`. The fixture deliberately
skips symlinks, so it skipped `/usr/local/bin/git -> /opt/git/bin/git` and selected
old `/usr/bin/git`. These are setup failures, not reproduced flakes. Corrected
custody probes and the full suite explicitly use
`A2A_LANE_GIT=/opt/git/bin/git PATH=/opt/git/bin:$PATH` (Git 2.54.0).

Raw outputs and per-execution results remain in `/tmp/h2-extra-base/`,
`/tmp/h2-extra-post/`, `/tmp/h2-base/`,
`/tmp/h2-base-git/`, and `/tmp/h2-post/`; aggregate summaries use matching
`/tmp/h2-*-summary.log` paths. Scratch probes are not part of the change.

## Per-flake evidence

All probes below use the exact named test with the load described above.
The additional fixture uses its unchanged pre-fold body in the already compiled
core binary: its path had no armed test clock and retained the original real
deadline behavior before its own fix.
Counts are failures / executions, not retries. Custody names refer to
`custody_restore::git_tests` in `crates/bridge-core/src/custody_restore_git_tests.rs`.

| Test | Hypothesis / confirmation | Alternative / separating observation | Probe | Base | Change | Post-fix |
|---|---|---|---|---|---|---|
| `b2a_no_effect_follows…` | Real 6s deadline expires before selected guard; injection flag remains false. | Incorrect enumeration would select the same wrong label unloaded, independently of elapsed time. | 50 exact executions; concurrency 50 | **8/50**, max 354.876s; `objects-checked #18: the guard never ran` | Advance a scoped guard clock to the exact deadline at the selected hook; preserve effects/ledger assertions and real child watchdogs. | **0/50**, max 86.770s |
| `round6_each_late_validation_guard…` | Six child launches consume the 6s deadline before injection; missing target or earlier timeout. | Hook isolation would produce another row's label despite available headroom. | 50 exact executions; concurrency 50 | **Not reproduced, 0/50**, max 256.121s | Source-proven 6s pre-injection race: scoped guard clock for shape discovery and expiry rows; advance only at the chosen event. | **0/50**, max 238.703s |
| `b2c_every_final_guard…` | Concurrent rows spend the 12s deadline before their selected final guard. | Incorrect shape would repeat an index/label mismatch without clock expiry. | 50 exact executions; concurrency 50 | **50/50**, max 578.969s; missing selected guard, earlier clock expiry | Each row owns a scoped clock advanced at its named guard; retain all 95 rows, child/proof/ledger and no-later-guard assertions. | **0/50**, max 557.594s |
| `compatibility_child_closes_staged_capabilities_before_provider_spawn` | Inherited 1s smoke deadline can expire before adapter spawn; `non_goal` permits aggregate success without a report. | Detached-writer race would show adapter progress and a report appearing after return. | 50 exact executions; concurrency 4 | **Not reproduced, 0/50**, max 6.283s | Source-proven deadline race: fake adapter completes ACP/PONG; require smoke completion and joined cleanup before reading report, with watchdog headroom and private history. | **0/50**, max 9.281s |
| `run_attempt_prompt_barrier_persists_failed_cleanup_from_real_caller` | One-second `args()` budget runs during real provenance/config/SQLite work before the durable barrier. | SQLite fault/in-process interference would fail at the barrier with available clock. | 50 exact executions; concurrency 8 | **50/50**, max 7.364s; returned `not_started`, expected `durable_evidence_failed` | Freeze Tokio time and drive the same attempt future to the durable barrier/cleanup events; separate 60s real watchdog. Original lifecycle and durable-write assertions retained. | **0/50**, max 4.198s |
| `nonzero_rm_with_a_hung_inventory…` | 1s removal plus scheduling can consume 1.5s before inventory starts, or violate the elapsed-time assertion. | Reset production deadline would consistently finish near 2.5s with inventory started. | 50 exact executions; concurrency 8 | **Not reproduced, 0/50**, max 1.556s | Unchanged. | **0/50**, max 1.593s |
| `fresh_claude_oauth_does_not_block_adapter_spawn` | One-second timeout can expire before marker adapter starts. | Ambient auth configuration would report auth refusal with available deadline. | 50 exact executions; concurrency 8 | **49/50**, max 4.394s; adapter marker absent | Fake adapter exits after publishing marker instead of hanging in `cat`; assert spawn/exit and reject resolve timeout. Private history and 60s watchdog. | **0/50**, max 33.657s |
| `node_selection_uses_acp_capability_and_fable_gates_before_task_prompt` | Initial hypothesis: inherited environment/child failure. Captured positive-case failures refine this to shared default history contention. | A selection race would reach prompt with an incorrect model after successful ACP setup; instead captured cause is `persistence` / `bridge.store_failure`, dispatched=0. | 50 exact executions; concurrency 8 | **20/50**, max 73.237s | Explicit fixture-owned SQLite ledger replaces shared platform history; all model/mode/Fable and prompt assertions retained, with complete failure output. | **0/50**, max 11.787s |
| Additional: `b2c_a_slow_call_with_headroom_inside_a_childs_bracket_changes_nothing` | The real 8s budget runs during bootstrap before the intended 300ms call. Loaded failures also reach the call and then report `Git(Timeout)` / finished after deadline. | A bracket-window bug would miss the same site with available clock; captured rows reach the selected call and fail later instead. | 50 pre-fold and post-fold exact executions, concurrency 8, 15 burners, explicit Git | **27/50** before this fixture changed, max 72.286s; first full run also failed | Scoped event clock retains 8s logical headroom, advances 300ms at the chosen call, and leaves real child watchdogs bounded. | **0/50**, max 107.249s |

## Findings and gates

No production behavior changes: the source-file clock seam selects its test
clock only under `cfg(test)`; production still compares `Instant::now()` with
the supplied deadline. The clock regression covers equality and unwind cleanup.
No retries, skips, or longer sleeps are introduced. All eight original post-fix
populations passed: **400 executions, 0 failures**. The additional headroom
fixture also passed **50/50** after its fold: **450 executions, 0 failures** total. The reaper remains unchanged;
fd and round6 are explicitly recorded as not reproduced and changed only under
the source-proven-race exception.

- First container full nextest: **5,933 passed / 1 failed / 13 ignored**, 406.606s,
  run `5cc16dc6-a4c2-4baa-a78f-7ebb38398ca8`. Its sole failure was the additional
  headroom control above. All eight originally listed tests passed. This outcome
  is retained; the additional fixture is changed before another full gate.
- Final container full nextest: **5,934 passed / 0 failed / 13 ignored**,
  311.366s, run `0fb51c21-6433-4992-b67b-ee48a3679050` (111 binaries).
  All nine investigated fixtures and the strengthened clock regression
  passed. The inherited ignored set is unchanged; no live gate ran.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p a2a-bridge -p bridge-core --all-targets -- -D warnings`:
  passed (44.01s), with `CARGO_INCREMENTAL=0`.
- `cargo run -p a2a-bridge -- validate --repo-hygiene`: passed,
  **44 tracked artifacts / 9 validated example configs**.
- `git diff --check`: passed. All changes fit the approximately 400-line budget.
