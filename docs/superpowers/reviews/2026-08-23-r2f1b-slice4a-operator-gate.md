# R2f1b slice 4A — operator gate and custody evidence

Implementation candidate: `8dfb899e` ("Share R2f1b attempt clock and internal timers")
Reviewed fix:            `07105482` ("Forward the attempt clock through ImplementAttemptTelemetry")
Base:                    `3dadec91` (`origin/main`)
Implementor:             `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`

## Loop extension — disclosed

4A's implement loop reached its **declared bound of 3** with `verify: PASS` and one unresolved
review BLOCKER. The loop was extended by **one targeted fix round** rather than restarted.

Justification (convergence discipline): the finding population was **closed and shrinking**, not
open-class.

| Round | Verdict | Findings |
|---|---|---|
| 1 | REJECT | clippy `dead_code` failure + a bridge-core test failure + no gates run before submission |
| 2 | REJECT | `run-workflow` terminal reporting still on a second `Instant::now()` epoch |
| 3 | REJECT | `ImplementAttemptTelemetry` does not forward the attempt clock (one-line fix agreed by both reviewers) |
| fix | **APPROVE** | converged on attempt 1, zero findings |

`implement --resume` refuses a `LoopStopped` run by design (terminal phase, and the attempt budget
is spent), so the fix ran as a fresh bounded run based on the secured candidate.

The candidate was fetched out of its quarantine clone to `refs/s4/4a-candidate` **before** any
further work, and the fix to `refs/s4/4a-fix`. Neither sat single-copy.

## The defect, verified by the operator at mechanism level

Not accepted on the reviewers' word:

- `impl RichEventSinkFactory for ImplementAttemptTelemetry` overrode **only** `make()`, so
  `monotonic_clock()` fell through to the trait default `None`.
- `crates/bridge-workflow/src/executor.rs` then runs
  `.and_then(|f| f.monotonic_clock()).unwrap_or_else(|| Arc::new(SystemMonotonicClock::start()))`
  — minting a **second epoch** for the cleanup tracker on the production `implement`-review path.
- The wrapped `AttemptTelemetrySinkFactory` already owned and exposed the correct clock via `clock()`.

**Severity, stated honestly:** no output is numerically wrong today, because
`review::reduce` discards `CleanupObserved.duration_ms`. Under severity discipline this is a
SMELL, not a WRONG. It blocked 4A anyway: "one clock identity per attempt across recorder,
telemetry, scheduler, cleanup and reporting" is the sub-slice's stated deliverable, and sub-slices
4B–4J arm timers on top of it.

## New regression test — operator-executed mutation check

The fix's test must discriminate the defect, not merely observe a value. Reverting **only** the
production forward while retaining the test:

```
test cli_tests::implement_attempt_telemetry_forwards_its_factory_clock ... FAILED
panicked at bin/a2a-bridge/src/main.rs:10284:42:
called `Option::unwrap()` on a `None` value
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1098 filtered out
```

Red population: exactly one test, and it is the named one. An `is_some()` assertion would have
passed against a freshly minted second clock — i.e. against the defect itself — so the test asserts
`Arc::ptr_eq` identity instead.

## Frozen mutation control — unchanged by the fix round

- Path: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-control.patch`
- SHA-256 on the candidate: `21b600c385d60b41b511c0acee30697e4f893b548d52c088300b9a04fb8bfd13`
- SHA-256 on the fixed tree: `21b600c385d60b41b511c0acee30697e4f893b548d52c088300b9a04fb8bfd13` — **byte-identical**
- Matches the SHA-256 recorded in the implementation handoff.

## Operator gate — fix tree `07105482`, idle machine

| Gate | Exit |
|---|---|
| `cargo fmt --all -- --check` | 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 |
| `cargo test -p bridge-core --locked --no-fail-fast` | 0 |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | 0 |
| `cargo test --workspace --locked --no-fail-fast` | 101 — 9 distinct failures |

## Attribution control — same environment, same machine, run sequentially

| Tree | Workspace exit | Distinct failures |
|---|---|---|
| `07105482` (fix) | 101 | 9 |
| `3dadec91` (base) | 101 | 9 |

Set difference in **both** directions is empty: no test fails on the candidate that passes on the
base, and none is fixed. The 9 are confined to `tests/smoke_cli.rs` (6) and
`tests/fallback_plan_cli.rs` (3) — host container/smoke tests, pre-existing on `origin/main`.

The two runs were sequential, not concurrent: this suite has previously inflated from 11 to 29
failures under parallel load on an identical tree.

### Measurement note

A first extraction pass reported "49 failing tests". That was a parser defect, not a result: the
`failures:` blocks embed captured JSON stdout, so lines such as `"attempt_id":` and `},` matched the
pattern. The corrected count of 9 reconciles exactly with the two `FAILED` summary lines (3 + 6).

## Size

- Counted added nonblank Rust lines, `3dadec91..07105482`: **316** (candidate 304 + fix 12)
- Cap: **450**. Projection was 300.
- Reviewer B's independent count of the candidate was 304 — an exact match with the operator count.

## Scope confirmations

- No timer is armed; `DeadlineActivationV2::AutomaticR2f1b` remains unconstructible from production.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched.
- Disclosed exclusions carried in the implementation handoff: `run_agent_preflight_uncached` and
  `run_node` turn-local diagnostics, and the pre-existing `workflow_history::DirectAttemptBarrier`
  surface (a separate execution path this slice does not reach; a follow-up candidate).
