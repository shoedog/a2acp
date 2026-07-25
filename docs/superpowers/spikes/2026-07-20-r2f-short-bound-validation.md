# R2f short-bound validation spike

- **Status:** EVIDENCE COMPLETE — D11 owner-approved with explicit observation margins on 2026-07-20
- **Base:** `345941db91a7d898884bfe79e573433484ccafcc`
- **Owner design:** [`../specs/2026-07-20-r2f-owner-design.md`](../specs/2026-07-20-r2f-owner-design.md)
- **Production effects:** forbidden; no provider prompt, production-server mutation, or production task/store write

## Question

Do the proposed R2f boundaries at or below 60 seconds fire correctly and leave adequate measured headroom for the
real local operations they bound? This spike can validate timer semantics and representative host behavior. It
cannot prove that a retry delay is optimal for every future network/provider failure; that remains reviewed owner
policy informed by the measured behavior.

## Candidate boundaries under test

| Id | Candidate | What must be validated | Spike verdict |
|---|---:|---|---|
| B1 | 30 s | First external-degradation half-open delay is monotonic and never fires early. | **POLICY ONLY** — deterministic semantics pass; R2f scheduler does not exist yet. |
| B2 | 31 s observable bound (30 s internal deadline) | One prompt-free health/control check has a hard deadline and terminal observation. | **SUPPORTED** for the local ACP control path — observed terminal at 30.001926917 s; arbitrary provider/network latency remains out of scope. |
| B3 | 6 s observable bound (5 s cooperative grace) | ACP graceful cancellation allows cooperative settlement, then escalates a non-cooperative exact owner. | **SUPPORTED** — real process groups escalated and terminalized by 5.006593584 s without survivors. |
| B4 | 60 s | The whole post-deadline cleanup envelope has headroom for cancel, exact process/container settlement, session close/debt, and ownership transfer. | **SUPPORTED** as the numeric exact-owner envelope; future partial/debt transfer still requires fail-first implementation tests. |
| B5 | 10 s | Terminal persistence/reporting has headroom under normal and bounded contention/failure. | **SUPPORTED** for the current terminal-store path — normal/contended writes are sub-millisecond and locked writes fail explicitly near 5.3 seconds; future reporting needs fail-first coverage. |
| B6 | 60 s | The first durable cleanup-debt retry is scheduled once, not early, and remains restart-reconstructible. | **POLICY ONLY** — deterministic restart semantics pass; durable debt scheduling does not exist yet. |

The later 2-minute/10-minute health delays and 5-minute/30-minute debt retries are outside the owner's requested
short-bound set, but the implementation plan still requires the same fake-clock no-early/one-fire coverage.

## Hypothesis / probe / falsifier log

Every probe records its result below before the spike verdict changes.

### H1 — timer semantics

- **Hypothesis:** monotonic fake-clock scheduling can represent B1/B2/B6 exactly, with no event immediately before
  the boundary and exactly one event at/after it.
- **Probe:** deterministic paused-time tests covering 30 seconds, 5 seconds, 10 seconds, and 60 seconds, including
  wall-clock rollback and duplicate wakeup controls.
- **Falsifier:** early fire, missed fire, duplicate fire, wall-clock sensitivity, or a test that requires real-time
  sleeping.
- **Result:** PASS as a policy prototype. A pure injected monotonic clock exercised 5-second, 10-second, 30-second,
  and 60-second one-shots. Every case was false at `due - 1 ms`, true exactly once at `due`, and false on duplicate
  and later polls. Restart reconstruction preserved the full delay after wall-clock rollback, preserved the
  remaining delay after a 1 ms forward move, and became immediately eligible only after the persisted wall-clock
  interval had elapsed. Tokio's current workspace feature set does not include `test-util`, and no R2f scheduler
  exists yet, so this is design characterization rather than a claim about implemented production scheduling.

### H2 — 31-second prompt-free control-check envelope

- **Hypothesis:** a deliberately non-responsive local adapter does not fail before its configured 30-second
  spawn/initialize deadline and returns terminal within a 1-second scheduler/cleanup margin, without sending a
  model prompt.
- **Alternative cause:** a control operation may hide a provider/network call whose latency distribution is not
  represented by local fakes.
- **Probe:** one real-process ACP handshake hang at the production 30-second deadline, plus source inspection of the
  exercised phase to prove no prompt path and exact-PID absence after terminal return.
- **Falsifier:** the deadline fires before 30 seconds, lacks a terminal result by 31 seconds, invokes a prompt, or
  claims terminal while the exact child survives. Passing local evidence cannot validate arbitrary provider network
  latency; that limitation remains explicit.
- **Result:** PASS for the real local control seam. A disposable `/bin/sh` -> `/bin/cat` child consumed ACP input but
  emitted no initialize response. `AcpBackend::spawn` used the production 30-second handshake deadline, returned an
  error after **30.001926917 s**, and left the exact recorded PID absent. Source inspection confirmed this path sends
  ACP initialize/control traffic and never enters `prompt`; no provider credential, provider request, or production
  server was involved. This does not characterize arbitrary provider/network latency or a future provider-specific
  health RPC.

### H3 — six-second cancellation envelope

- **Hypothesis:** cooperative ACP cancellation settles well below five seconds, while a non-cooperative exact
  subprocess reaches escalation at the configured grace and produces a terminal result without a surviving owned
  process tree.
- **Alternative cause:** a driver may terminalize while its child/container survives, making stream latency look
  healthy while cleanup leaks.
- **Probe:** existing ACP cancellation regressions plus disposable real child/process-group controls for cooperative,
  TERM-ignoring, and descendant-survival behavior; verify process absence separately from stream completion.
- **Falsifier:** escalation fires early, terminal arrives materially after grace without typed partial cleanup, or an
  exact owned descendant survives a claimed complete cleanup.
- **Result:** PASS. The existing `cancel_hung_agent_is_terminated_within_grace` ACP regression passed **1/0** with a
  scaled 150 ms grace, establishing cancel-stream terminalization. The real production `Supervised` path then ran
  30 cooperative exact groups and 10 concurrent TERM-ignoring exact groups at the actual five-second grace:

  | Population | n | min | mean | median | p95 | p99 | max |
  |---|---:|---:|---:|---:|---:|---:|---:|
  | cooperative | 30 | 41.291 us | 46.983 us | 44.542 us | 61.917 us | 86.125 us | 86.125 us |
  | TERM-ignore escalation | 10 | 5.006327125 s | 5.006371387 s | 5.006343792 s | 5.006593584 s | 5.006593584 s | 5.006593584 s |
  | stacked 5 s cancel + 500 ms terminate | 10 | 5.508283416 s | 5.508320449 s | 5.508315250 s | 5.508360916 s | 5.508360916 s | 5.508360916 s |

  No escalation fired early, and signal-zero checks found every exact process group absent after terminal return.
  The owner therefore approved a six-second observable cancellation bound: five seconds for cooperative settlement
  plus one second for escalation, reap, and terminal observation. The follow-up population directly composed the
  current ACP five-second wait with the production 500 ms `TERMINATE_GRACE`; it remained below six seconds with
  about 492 ms measured margin. Container removal remains part of B4's separate 60-second cleanup envelope.

### H4 — sixty-second outer cleanup

- **Hypothesis:** representative host process-tree and available container-runtime cleanup complete inside 30 seconds
  (at least 2x headroom), while ambiguity/forced delay transfers to typed durable debt rather than blocking toward
  60 seconds.
- **Alternative cause:** container runtime latency or a shared-generation collateral hold, rather than process
  signaling, may dominate cleanup.
- **Probe:** repeated disposable process-group settlement and disposable container stop/remove/reap measurements;
  separately exercise an intentionally unresolvable owner and inspect the partial/debt outcome where a current
  fixture exists.
- **Falsifier:** any ordinary exact-owned cleanup exceeds 30 seconds, claims complete with survivors, or lacks an
  ownership-transfer result for the unresolvable case.
- **Result:** PASS for the numeric envelope on current exact-owner mechanisms. Ten live disposable Docker containers
  were removed through `ReapController::production` and then proved absent by exact-name inspect:

  | Population | n | min | mean | median | p95 | p99 | max |
  |---|---:|---:|---:|---:|---:|---:|---:|
  | Docker production reap | 10 | 107.156750 ms | 125.376741 ms | 127.940750 ms | 148.874708 ms | 148.874708 ms | 148.874708 ms |

  Even the representative sequential composition of the measured worst stacked cancellation, Docker reap, and
  locked-store failure is about **10.982 s**, below the spike's 30-second 2x-headroom criterion for a 60-second outer
  bound. Current main has no R2f typed `cleanup_partial`/durable-debt transfer for an unresolvable owner; that path
  remains a required fail-first implementation test and cannot inherit the exact-owner completion claim.

### H5 — ten-second terminal persistence/reporting

- **Hypothesis:** owner-local SQLite terminal writes are normally sub-second and remain below two seconds under
  representative concurrent writer load, leaving at least 5x headroom; lock/fault cases fail explicitly rather than
  hanging to ten seconds.
- **Alternative cause:** the single async-path blocking SQLite mutex or filesystem sync, rather than SQL execution,
  may dominate tail latency.
- **Probe:** repeated real `SqliteStore` terminal mutations on a private temporary database, concurrent writer load,
  and an external lock/failure control; report n/min/mean/median/p95/p99/max.
- **Falsifier:** normal/concurrent max exceeds two seconds, a failure hangs beyond ten seconds, or the measurement
  bypasses the store API that production uses.
- **Result:** PASS through the production `SqliteStore::set_terminal` API on private file-backed WAL databases:

  | Population | n | min | mean | median | p95 | p99 | max |
  |---|---:|---:|---:|---:|---:|---:|---:|
  | normal terminal write | 1,000 | 21.458 us | 29.069 us | 25.250 us | 36.042 us | 50.792 us | 661.917 us |
  | same-store concurrent write | 30 | 24.875 us | 112.766 us | 120.625 us | 290.583 us | 375.541 us | 375.541 us |
  | external writer-lock failure | 30 | 5.218102250 s | 5.279622972 s | 5.273870500 s | 5.322202542 s | 5.324939208 s | 5.324939208 s |

  Normal and same-store contention stayed far below two seconds. Every external `BEGIN IMMEDIATE` control failed
  explicitly after the production five-second SQLite busy timeout and before ten seconds. The first 30-runtime
  failure harness hit the host's 256-descriptor limit (`EMFILE`) before measuring the store; the shell had only nine
  descriptors, and the first failure was per-thread runtime construction, ruling out a persistent host leak. Reusing
  the existing runtime handle preserved 30 parallel locked databases and produced the passing population above.
  A future terminal reporting sink is not implemented by this spike and retains its own fail-first deadline test.

### H6 — first cleanup-debt retry

- **Hypothesis:** a durable debt created at monotonic T schedules no retry before T+60 s, schedules exactly one at
  T+60 s, and reconstructs the same next action after restart without depending on a persisted monotonic instant.
- **Probe:** deterministic state-machine/fake-clock characterization, including restart from wall-clock timestamp and
  rollback/forward-jump controls.
- **Falsifier:** early/duplicate retry, lost retry after restart, or wall-clock rollback makes debt immediately
  eligible.
- **Result:** PASS as a policy prototype, therefore **POLICY ONLY**. The same deterministic state machine used for H1
  proved no early/duplicate retry and restart reconstruction from the durable wall timestamp. A backward wall jump
  receives a fresh full monotonic delay rather than becoming eligible; a forward jump deducts only elapsed durable
  time and saturates at immediate eligibility. No implementation exists yet, so these cases must be fail-first R2f
  tests rather than current-main executable evidence.

## Commands and test totals

- Existing focused controls: `cancel_hung_agent_is_terminated_within_grace`,
  `spawn_handshake_failure_reaps_the_container`, and
  `file_backed_open_sets_wal_synchronous_busy_timeout`: **3 passed / 0 failed** total. The second control can return
  on immediate protocol behavior and is not used as the 30-second proof.
- Disposable timer/process harness: **2 passed / 0 failed** across the two selected invocations; one other harness
  test was filtered in each invocation.
- Disposable Docker harness: **1 passed / 0 failed**.
- Disposable SQLite harness: first harness attempt **0 passed / 1 failed** because the harness exhausted its own file
  descriptors; corrected rerun **1 passed / 0 failed**.
- Disposable real 30-second ACP harness: **1 passed / 0 failed**.
- Disposable stacked-cancellation follow-up after owner margin selection: **1 passed / 0 failed** across 10 exact
  process groups.
- All spike-specific processes, process groups, Docker containers, databases, and temporary harness source were
  removed after observation. The repository's full suite was not run because this spike changes no production code;
  the implementation verification contract below remains unchanged.

## Sampling and reporting contract

- Timer/state probes are deterministic pass/fail and do not use real sleeps for policy-length delays.
- Real process cleanup: at least 30 repetitions per ordinary scenario.
- Available local container runtime: at least 10 disposable repetitions; absence is reported, not substituted with a
  mock claim.
- SQLite normal writes: at least 1,000; concurrent/failure scenarios: at least 30 where mechanically possible.
- Latency populations report count, minimum, mean, median, p95, p99 when meaningful, and maximum.
- Setup/build time is excluded from operation latency and reported separately.
- Every temporary process/container/database uses a spike-specific exact identity under the host temporary root and
  is removed after proving it is not owned by a live test.

## Verdict contract

Each boundary ends as one of:

- **SUPPORTED:** deterministic semantics pass and representative real operations meet the stated headroom criterion;
- **REVISE:** evidence falsifies the candidate and records the measured replacement;
- **POLICY ONLY:** mechanism/timer is validated, but available evidence cannot establish operational optimality;
- **BLOCKED:** a required representative runtime/path is unavailable and no honest equivalent was exercised.

All B1-B6 have a verdict. On 2026-07-20 the owner approved D11 with two observation margins derived from the spike:
the ACP control timeout is 31 seconds observable around its 30-second internal deadline, and cancellation is six
seconds observable around its five-second cooperative grace. B1 and B6 remain owner policy backed by deterministic
characterization rather than current production implementation. No implementation slice may cite this file as
implementation-closure evidence; it defines the fail-first tests that implementation must satisfy.
