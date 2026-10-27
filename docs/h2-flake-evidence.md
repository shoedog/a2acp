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

## H3 — remaining custody deadlines and dirty snapshots (2026-10-27)

Base: `30c4bfae328dd64ea7812cf178a456454c7c0f24`, Linux aarch64 container,
Rust 1.94.0, Git 2.54.0, nextest 0.9.146. No host or provider gate ran.
The initial inventory-only handoff stopped at the original size gate. After
review rejected the incomplete delivery, the explicit continuation requested
both fixes and their verification; this section records the completed work.

### Population

Audited all three `custody_restore*tests.rs` modules, every `Instant::now`, sleep,
finite deadline, and callers of `pass_the_deadline`, `run_late`, `run_guarded`,
`CallClockV1::watch`, `bracket_row`, `known_row`, `owed_row`, and `run_raw_row_v3`.
The table names every deliberate expiry/headroom control, including immediate
spent-deadline controls and H2's shared guard-driver controls. Generic 120s
success watchdogs without deliberate pacing/expiry are not guard-clock races.
Filesystem timestamp sleeps (`let_the_clock_tick`, the payload content rewrite)
are not deadline controls. `custody_restore_tests.rs` has no timing controls.
Payload tests are nested under `custody_restore::git_tests::payload_tests` and
already import the H2 seam via `super::*`; no second clock is needed.

“Converting now” denotes H3's event-clock conversions. Real-time rows retain
bounded real runner watchdogs: their purpose requires a real child to overrun
its deadline. These mixed tests' non-timeout outcomes retain watchdog headroom.
The same H2 thread-local clock is armed in each row's owning thread, including
payload row threads. `with_headroom` gives guards their original logical budget
and children the existing 120s real watchdog. Expiry occurs at the selected event,
including equality; headroom advances preserve each original logical delay.
Every effect, ledger, outcome, footprint, and no-later-call assertion remains.
The added `event_clock_ignores_elapsed_wall_time_before_the_selected_guard`
control starts with an already-spent wall deadline and reaches the final metadata
guard using event time; no scheduling sleep is required.

Population: 72 original Git tests (8 already H2, 53 converted here, 11 real-child
timeout tests), 5 converted payload tests, and the additional H3 clock control.

Git test names below are relative to `custody_restore::git_tests`.

| Test | Classification / reason |
|---|---|
| `event_deadline_clock_expires_at_equality_and_restores_after_unwind` | Already converted by H2 (existing scoped guard clock). |
| `an_expired_deadline_refuses_before_any_child_and_a_slow_child_times_out` | Real-time: must observe the runner killing the real index-pack child; entry-expired row is convertible. |
| `round5_a_final_fsck_that_finishes_before_the_deadline_cannot_succeed_after_it` | Converting now: advance the existing H2 clock at the selected event. |
| `round5_a_final_fsck_settling_with_headroom_succeeds_before_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_the_near_expiry_rows_are_timed_from_the_deadline_the_bootstrap_receives` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_a_deadline_spent_before_the_final_gate_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_a_deadline_spent_inside_the_closing_tree_census_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_a_deadline_spent_inside_the_closing_mount_census_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_a_deadline_spent_inside_the_final_childs_success_gate_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_a_deadline_spent_after_the_final_digest_read_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_with_headroom_every_late_validation_step_succeeds_before_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `round6_the_late_validation_guards_run_in_the_documented_order` | Already converted by H2 (existing scoped guard clock). |
| `round6_each_late_validation_guard_halts_the_run_alone` | Already converted by H2 (existing scoped guard clock). |
| `round6_the_root_identity_is_still_the_last_fallible_gate_after_its_guard` | Already converted by H2 (existing scoped guard clock). |
| `round6_a_failed_final_child_is_never_held_to_the_success_gate_guards` | Already converted by H2 (existing scoped guard clock). |
| `b1a_every_refusal_is_in_the_failure_shape_that_began_no_materialization` | Converting now: advance the existing H2 clock at the selected event. |
| `b1a_the_paced_rehash_checks_rewinds_and_reads_a_bounded_prefix` | Converting now: advance the existing H2 clock at the selected event. |
| `b1a_each_guard_halts_the_run_alone_and_headroom_succeeds` | Converting now: advance the existing H2 clock at the selected event. |
| `b1a_a_spent_deadline_refuses_before_any_frame_is_touched` | Converting now: advance the existing H2 clock at the selected event. |
| `b1a_the_deadline_is_observed_before_every_decoded_entry` | Converting now: advance the existing H2 clock at the selected event. |
| `b1a_no_frame_byte_is_read_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b1b_no_source_read_follows_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b1b_a_spent_deadline_refuses_before_a_file_is_routed` | Converting now: advance the existing H2 clock at the selected event. |
| `b1b_the_agreement_phases_each_observe_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b1b_the_retained_agreement_observes_the_one_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b1b_the_retained_agreement_decodes_nothing_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_a_spent_deadline_refuses_before_any_write` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_each_guard_halts_the_run_alone_and_headroom_succeeds` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_a_deadline_spent_in_the_final_gate_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_no_chunk_is_read_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_no_effect_follows_a_guard_that_finds_the_deadline_spent` | Already converted by H2 (existing scoped guard clock). |
| `b2a_a_slow_short_write_keeps_exactly_the_accepted_prefix_and_makes_no_second_write` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_an_interrupted_read_or_write_is_never_retried_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_the_length_probe_is_never_called_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_no_read_write_or_probe_follows_a_call_that_returned_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2a_interrupted_and_short_calls_with_headroom_copy_exactly_and_conserve_the_ledger` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_the_drain_after_a_held_ref_fault_never_reads_past_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_a_spent_deadline_refuses_before_any_write` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_each_guard_halts_the_run_alone_and_headroom_succeeds` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_a_deadline_spent_in_the_final_gate_cannot_succeed` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_no_effect_follows_a_guard_that_finds_the_deadline_spent` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_a_slow_short_write_keeps_exactly_the_accepted_prefix_and_makes_no_second_write` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_an_interrupted_write_is_never_retried_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_no_chunk_is_read_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_no_read_or_write_follows_a_call_that_returned_after_the_deadline` | Converting now: advance the existing H2 clock at the selected event. |
| `b2b_interrupted_and_short_writes_with_headroom_restore_exactly_and_conserve_the_ledger` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_the_one_deadline_reaches_every_helper_and_none_resets_it` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_every_final_guard_halts_the_run_alone_and_no_success_follows_expiry` | Already converted by H2 (existing scoped guard clock). |
| `b2c_a_final_child_that_leaves_an_effect_is_refused_beside_its_own_outcome_on_every_outcome` | Real-time timeout rows: must kill the final fsck after its filesystem effects; other outcomes use watchdog headroom. |
| `b2c_a_failed_childs_reconciliation_digest_reads_one_byte_past_the_census_and_no_further` | Real-time timeout row: must kill fsck before measuring its effect through retained descriptors. |
| `b2c_a_deadline_spent_before_the_call_refuses_before_any_child_starts` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_deadline_spent_before_a_childs_digest_refuses_before_the_child_starts` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_slow_call_that_returns_after_the_deadline_is_followed_by_no_further_call` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_deadline_spent_in_a_runner_callback_is_reconciled_beside_what_the_child_left` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_slow_call_inside_a_childs_bracket_is_followed_by_no_further_call` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_slow_call_with_headroom_inside_a_childs_bracket_changes_nothing` | Already converted by H2 (existing scoped guard clock). |
| `b2c_a_timed_out_final_child_is_reconciled_once_beside_every_retained_chain_and_mount_it_left_owed` | Real-time: runner must kill fsck after swaps/mount effects, then reconcile each owed unit. |
| `b2c_a_successful_final_child_whose_settlement_returns_late_is_reconciled_once_beside_what_it_left` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_known_failure_whose_gate_already_ran_is_reconciled_for_the_tree_only_and_a_headroom_call_for_none` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_timed_out_final_child_that_added_an_entry_and_rewrote_head_reports_the_timeout_beside_both` | Real-time: must kill fsck after both entry creation and HEAD rewrite. |
| `b2c_a_drifted_binary_stays_the_primary_beside_the_callback_refusal_and_what_the_child_left` | Real-time timeout row: must kill the drifted route and retain drift beside callback timeout. |
| `b2c_two_swapped_retained_chain_entries_and_a_swapped_root_are_all_reported_with_the_root_last` | Real-time timeout row: must kill fsck after both chain and root swaps. |
| `b2c_controls_a_refused_pre_spawn_gate_is_not_replayed_and_a_completed_post_exit_gate_is_not_repeated` | Real-time timeout control: must kill the final fsck and count exactly one reconciliation. |
| `b2c_a_returned_slow_census_call_is_counted_once_and_every_unattempted_unit_still_runs_once` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_a_timeout_after_the_paced_digest_bytes_returned_does_not_reread_them` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_work_and_plain_swapped_together_are_both_reported_once_each_with_unique_prefixes_and_the_root_last` | Real-time timeout rows: must kill fsck after work/plain/root swaps; successful-child controls retain watchdogs. |
| `b2c_a_gate_stopped_between_an_admitted_files_two_calls_does_not_repeat_the_first` | Converting now: advance the existing H2 clock at the selected event. |
| `b2c_lossless_raw_name_aliases_are_independent_census_units_with_exact_attempts_and_resume` | Converting now: advance the existing H2 clock at the selected event. |
| `corrective_a_pre_spawn_or_post_exit_refusal_settles_the_reservation` | Converting now: advance the existing H2 clock at the selected event. |
| `corrective_a_timed_out_writing_child_settles_its_reservation` | Real-time: must kill real init/index-pack children and settle their actual reservations. |
| `corrective_a_timed_out_read_only_child_is_refused_beside_its_effect` | Real-time: must kill verify-pack/fsck and reconcile the effect beside the runner timeout. |
| `round4_the_baseline_digest_honours_the_one_deadline_before_a_read_only_child_starts` | Converting now: advance the existing H2 clock at the selected event. |

Payload test names below are relative to `custody_restore::git_tests::payload_tests`.

| Test | Classification / reason |
|---|---|
| `c2a_the_one_deadline_is_kept_before_every_step_and_entry` | Converting now: use the imported H2 guard clock in each row thread. |
| `c2b_the_deadline_expiring_mid_frame_or_mid_step_halts_with_the_partial_kept` | Converting now: use the imported H2 guard clock in each row thread. |
| `c2b_every_materialization_guard_is_in_its_place_and_halts_the_run_alone` | Converting now: use the imported H2 guard clock in each row thread. |
| `c2b_a_create_returning_after_the_deadline_is_never_followed_by_another` | Converting now: use the imported H2 guard clock in each row thread. |
| `c3_the_one_deadline_is_kept_before_every_entry_and_every_seal` | Converting now: use the imported H2 guard clock in each row thread. |

### Custody-push hypothesis, probe, and result

Before probing, `/tmp/h3-hypotheses.md` recorded the hypothesis: copying the Git
index gives it a newer timestamp, suppressing racy-clean content checks after a
same-length worktree rewrite. Alternative: a concurrent writer/shared fixture.
The unchanged test owns a unique `TempDir`, writes both versions synchronously,
and starts no writer thread. Its loaded base failure was **1/50**, with the exact
`original`/`modified` signature at `custody_push.rs:1197` (run 14, 2.96s).

**Production finding:** the private index copy acquired a fresh mtime. Git uses
that timestamp to decide whether matching cached file stats need a content check;
copying could turn a racy entry into an apparently clean one. The regression
`snapshot_preserves_racy_index_content_checks` uses a fixed, nonzero file/index
mtime and disables ctime comparison in its isolated clone policy to model coarse
stat precision. With the original production function it deterministically fails
(`original` instead of `modified`, 0.15s); preserving the source index's mtime on
the private copy passes. The copied bytes and timestamp come from the same open
source descriptor. No shared index, ref, or worktree is modified by snapshotting.
The regression covers unchanged content, a tracked-but-ignored rewritten file,
an unchanged tracked file, exclusion of an ignored untracked file, and cleanup.
The original terminal push/lease/immutability test remains intact.

### Loaded comparisons and gates

Unchanged base binaries were built with `CARGO_INCREMENTAL=0 cargo nextest run
--workspace --locked --no-run` (117s) and privately copied/stripped. Probes use
H2's 15 owned Python CPU burners, 50 concurrent independent executions per test,
and `A2A_LANE_GIT=/opt/git/bin/git PATH=/opt/git/bin:$PATH`. Each child runs
`--exact <name> --nocapture`; outputs, exit codes, and durations are retained.
No retries, skips, or sleep inflation were introduced. The initial interrupted
base population has no count; its completed replacement is recorded below.
A focused nextest run overlapping the resumed base probes was interrupted to
isolate measurement load (60 passed, 15 SIGINT, no assertion failures); it is not
a suite gate. Its compile overlapped the start of the resumed base population.

Names below are relative to `custody_restore::git_tests`, except the snapshot
row, which is `custody_push::tests` in bridge-controller. Counts include every
completed execution, without nextest retries.

| Test | Isolated loaded base failures | Loaded post-fix failures | Maximum base / post duration |
|---|---|---|---|
| `dirty_snapshot_contents_and_no_worktree_index_or_ref_mutation` | 1/50 | 0/50 | 4.388s / 5.334s |
| `b2b_each_guard_halts_the_run_alone_and_headroom_succeeds` | 0/50 | 0/50 | 82.551s / 106.500s |
| `b2b_no_effect_follows_a_guard_that_finds_the_deadline_spent` | 0/50 | 0/50 | 69.856s / 62.355s |
| `b2c_a_slow_call_inside_a_childs_bracket_is_followed_by_no_further_call` | 50/50 | 0/50 | 92.928s / 66.527s |
| `b2c_a_slow_call_that_returns_after_the_deadline_is_followed_by_no_further_call` | 50/50 | 0/50 | 28.511s / 16.580s |

The isolated metadata no-effect base sample was **not reproduced (0/50)**.
The earlier population with initial compilation overlap failed **38/50**
(maximum 208.020s), including `metadata-checked #11: the guard never ran`.
Both populations are retained; the isolated one is used in the comparison table.
The unchanged source's 1.2s guard budget can expire before its selected hook;
the added unarmed/armed control proves that entry refusal and the intended final
guard refusal independently, without sleeps. The two slowest H3 conversions in
the full gate were the metadata each-guard (7.285s) and no-effect (5.788s) tests;
both are included above. All five post-fix populations passed: **250 executions,
0 failures**.

The first completed focused run found a defect in the new scheduling control's
logging: a second hook replaced its recorder (342 passed, 1 failed, 139 not run).
The control now records and expires in the same hook. The subsequent full gate
includes the corrected control and every original custody test.

- Container full nextest: **5,937 passed / 0 failed / 13 inherited skips**,
  158.717s, 112 binaries, run `69a2d3bf-2d32-4ee2-aaf1-e51d08a16385`.
  All 58 converted tests, the clock control, and the snapshot regression passed.
  No new ignore/skip or retry policy was added.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p bridge-core -p bridge-controller --all-targets --locked --
  -D warnings`: passed, 11.28s.
- `cargo run -p a2a-bridge -- validate --repo-hygiene`: passed,
  44 tracked artifacts / 9 validated example configs.
- Cargo build, full nextest, clippy, and hygiene used `CARGO_INCREMENTAL=0`.
  Format/clippy used the installed toolchain bin directory on PATH.
- `git diff --check`: passed. The tested source hashes still match the files
  staged for the repair.

### Durable evidence and retrieval

The temporary paths named above are also bundle member names; the original
`/tmp` directories are no longer required. The durable archive is retained
off-repo by the operator (it holds host-specific paths), identified by:

SHA-256: `c1b4919fb0026e94dc078bdfea7931a7c992e153060b7d39be498639cc792aed`; 591,807 bytes.
The archive contains 585 files: complete base/post outputs and per-execution JSON,
hypotheses, probe scripts, test binary hashes, tested source hashes and files,
the code patch, gate logs, the expected-red snapshot regression output,
and a per-member size/SHA-256 manifest. Retrieval was validated from the checked-in
path by reopening the archive and verifying every member against that manifest;
every measured population JSON has exactly 50 executions. Committing the bundle
with this handoff retains the records after container and clone cleanup.
