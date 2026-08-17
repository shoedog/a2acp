# Coverage-lane flake family — investigation (2026-08-17)

Follow-up raised at the 3d-T2 landing. Three flakes have now been ledgered, all
in the same CI lane, each a different test:

| # | Test | First seen |
|---|------|-----------|
| 1 | `authority_mutation…lock_release_failure_is_loud_not_silent` | 3c2 landing |
| 2 | `compatibility::tests::staged_candidate_exec_is_bound_to_the_verified_file_object` (`smoke_process_launch_failed`) | 3d-T1 landing |
| 3 | `cli_tests::operator_container_authority_reports_failed_exact_id_removal` | 3d-T2 landing |

"Always that lane, always a different test" is the pattern worth explaining
once, rather than ledgering a fourth name later.

## Finding 1 — the coverage lane IS the workspace test run

`.github/workflows/ci.yml` has no plain `cargo test --workspace`. The only step
that executes the whole suite is:

```yaml
- name: Coverage — workspace (≥85% line coverage)
  env:
    CARGO_PROFILE_DEV_DEBUG: 0
  run: cargo llvm-cov --workspace --fail-under-lines 85
```

The dedicated `cargo test` steps are narrow (`-p bridge-store`, plus two exact
`bridge-store` cases, one of them `--test-threads=1`).

**Consequence:** in CI, every workspace test only ever runs *instrumented*, at
default parallelism, on a small hosted runner. So this is not "the coverage lane
is flaky" — it is "the only lane that runs these tests is the instrumented one."
Any timing- or resource-sensitive test has nowhere else to show up, and there is
no non-instrumented control to compare against.

That structural fact explains the *location* of all three flakes without
appealing to any property of the individual tests.

## Finding 2 — all three involve spawning child processes

Class 2 is literally a process-launch failure. Class 3 writes a shell script to
a tempdir, `chmod 0755`s it, and execs it as a fake container runtime
(`bin/a2a-bridge/src/main.rs:10261`). Class 1 is about releasing an advisory
lock while children exist. Instrumented builds make every spawned process
slower and add profraw writes, which widens every fork/exec window.

This is a coherent shared mechanism, and it is consistent with all three
observations — but see finding 3 before treating it as established.

## Finding 3 — the tempting explanation is RULED OUT

The passing job log carries, three times:

```
ERROR bridge_core::liveness: releasing an advisory lock failed;
  a concurrently spawned child may hold it until it execs
  lease="authority-state.lock" error="Bad file descriptor (os error 9)"
```

That diagnostic names fd-inheritance-across-fork, which is exactly class 1's
subject, and it is very tempting to call it the root cause.

**It is not.** Those lines appear in the run where every test PASSED. They are
logged-only noise present on green runs, so they do not discriminate between
pass and fail. Two further notes for whoever picks this up:

- The errno is `EBADF`, not `EWOULDBLOCK`. EBADF on release means the descriptor
  was already invalid, which points at a lock lifecycle/ordering question rather
  than at a child holding a duplicate — the message's own theory and its errno
  disagree. Worth resolving on its own merits; it is a real (if currently
  benign) inconsistency.
- Rust's std sets `O_CLOEXEC` by default, so plain inheritance would only span
  the fork→exec window, not outlive it.

## Finding 4 — the specific failure's assertion is UNRECOVERABLE, and that is my error

To obtain a same-SHA green control for class 3, I re-ran the failed job. **A
re-run replaces the job's log**, so the original failing output — which
assertion fired, with what message — is permanently gone. The captured evidence
is only `test cli_tests::operator_container_authority_reports_failed_exact_id_removal … FAILED`.

The control was worth having and the classification stands on four independent
legs (same-SHA sibling run green, same-SHA rerun green, host suite green, test
outside the diff). But the diagnostic evidence was destroyed to get it, and it
did not have to be.

**Process rule going forward: capture the failing job log to a file BEFORE
re-running it.** `gh run view --job <id> --log > failure.log` costs seconds and
preserves the only copy.

## Recommendation — instrument, do not guess

The mechanism in finding 2 is plausible and unproven, and finding 3 shows how
easily a plausible story here is wrong. Rather than "fix" a test on a guess:

1. **Add a plain, non-instrumented `cargo test --workspace` step to CI.** This
   is the highest-value change and is independently justified: today a genuine
   test regression and an instrumentation artifact are indistinguishable,
   because there is no uninstrumented control. If a class recurs on the plain
   step too, it is a real test defect; if it only ever fails under `llvm-cov`,
   the instrumented lane's resource profile is confirmed as the cause.
2. **On the next occurrence, capture the log first**, then read the actual
   assertion. One real assertion is worth more than this whole document.
3. Only then consider a targeted fix — e.g. bounding parallelism for the
   instrumented run, or making the specific test's process launch retry-tolerant.

Explicitly NOT recommended yet: lowering `--test-threads` for the coverage lane.
It would probably mask the family, would lengthen the slowest CI job, and would
destroy the signal before its cause is known.

## Status

Investigation only — no code change. Findings 1, 3 and 4 are established;
finding 2 is a live hypothesis. The family remains three ledgered classes, now
with a structural explanation for the lane and a named next probe.
