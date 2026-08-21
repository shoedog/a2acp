# T3a increment 3A-1 handoff — repository-authority observation behind the probe

## Candidate checkpoint

- Base tree: `84a48a4cff85fc7b1aba50c3a569ae79f6074a52`.
- Pre-edit `git status --short` was empty and the added nonblank Rust-line count was 0.
- This is an implementation candidate only. Cargo gates and all runtime results are operator-owned and were not run in this container.
- The task-cited combined increment specification, `docs/superpowers/plans/2026-08-21-t3a-inc3-task.md`, is absent at this `HEAD`; the checked-in source and authoritative task were used instead.

## Implementation and scope

- `ExactAbsenceProbeV1` now exposes `observe_source_common_dir_identity`.
- `HostGitWorktree` owns the unchanged `git -C <source> rev-parse --path-format=absolute --git-common-dir` observation and preserves each existing source-authority error message verbatim.
- Candidate construction (`from_legacy` and `from_claim`) and `revalidate_source` accept the caller-supplied `&dyn ExactAbsenceProbeV1`; `source_common_dir_identity` delegates to that supplied instance, while host revalidation passes `self`.
- `Probe`, `BothAbsentProbe`, and `RecordingProbe` all delegate source authority to `HostGitWorktree`, preserving their real-Git candidate construction behavior while leaving their canned exact-absence results and `observe_exact_absence` counter values unchanged.
- No report vocabulary, construction-failure mapping, admitted-record `.ok()` discard, population admission, placement precedence, readiness, `effective()`, or action authority changed. Retyping remains wholly deferred to 3A-2.

## Evidence classification

This is behavior-preserving structural and characterization work. There is no genuine behavioral red: the Git command, decision paths, error bytes, and existing probe-call counts are intentionally unchanged. No existing test assertion was edited, including no assertion that mechanically names a Git call site.

Known non-blocking follow-up: `host_git.rs` retains its preexisting async `common_dir()` fallback separately from this strict, byte-preserving source-authority probe. Consolidating them would alter the required error behavior, so it is deliberately out of scope for 3A-1.

## Structural review anchors

| Anchor | Repository result |
| --- | --- |
| Injected candidate probe | `from_legacy`, `from_claim`, and `revalidate_source` pass their caller-supplied probe to `source_common_dir_identity`; no candidate construction code instantiates `HostGitWorktree`. |
| Probe implementation count | Four implementations supply the method: production `HostGitWorktree`; test `Probe`, `BothAbsentProbe`, and `RecordingProbe`. |
| Error text | The host method retains the four source-authority strings and delegates directory capture to the original helper. |
| Deferred 3A-2 work | `report.rs` is unchanged; no typed-vocabulary arm or construction-failure behavior changed. |

## Final counted-line worksheet

Counted as nonblank added physical lines in changed `.rs` files against the base, after formatting.

| File | raw added | blank | nonblank counted |
| --- | ---: | ---: | ---: |
| `crates/bridge-worktree/src/sweep.rs` | 51 | 0 | 51 |
| `crates/bridge-worktree/src/host_git.rs` | 45 | 0 | 45 |
| `crates/bridge-worktree/src/sweep/checked_scan.rs` | 9 | 0 | 9 |
| **Total** | **105** | **0** | **105** |

The post-format total is 105 nonblank added Rust lines, within the 220-line hard cap. No build, test, or hygiene gate result is claimed.

## Operator gates

- [ ] PENDING OPERATOR — `cargo fmt --all -- --check`
- [ ] PENDING OPERATOR — `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] PENDING OPERATOR — `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast`
- [ ] PENDING OPERATOR — `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast`
- [ ] PENDING OPERATOR — `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point)
- [ ] PENDING OPERATOR — `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point)


---

## Operator evidence — filled 2026-08-21

**Implementation commit:** `228e15de` · **Base:** `84a48a4cff85fc7b1aba50c3a569ae79f6074a52`

All gates green. `bridge-worktree` **320 passed — identical to base**, which is the
expected result for a behaviour-preserving port: no new tests, no changed decisions.

### Behaviour preservation, verified independently

- `source_common_dir_identity` now delegates to
  `probe.observe_source_common_dir_identity(source)`; it invokes no Git command.
- `sweep.rs` retains five `Command::new("git")` sites. Four are test fixtures. The
  fifth is `run_git_sync`, the **action path** (`worktree remove`, `prune`), which
  is unchanged from base — `git show base | grep -c run_git_sync` returns 4 at both
  ends. It is outside candidate construction, so the acceptance criterion holds.
- All four probe implementations supply the new method: `HostGitWorktree`, `Probe`,
  `BothAbsentProbe`, `RecordingProbe`.
- All four `source authority probe …` messages appear exactly once before and once
  after.

### Counted lines — 105 against a 220 cap

Reviewers disagreed 100 vs 105. **105 is correct**: independently measured per file
as 51 (`sweep.rs`) + 45 (`host_git.rs`) + 9 (`checked_scan.rs`), matching this
handoff's own worksheet exactly. Blank added lines are zero, and the count does not
subtract them twice — the error that produced a false 636 for increment 2.

### Why this half exists

3A's first dispatch stopped at its cap: 509 nonblank lines against 500, before
formatting and before finishing. 3A was split at an **ordered** seam, not the
obvious one — four of the messages 3A-2 must retype live inside the function this
half relocates, so retyping first would type them where the port then moves them.

**3A-2 rebinds to this accepted head** and carries all of 3A's behavioural red.

### Limits

- These results attest the tree at `228e15de` only.
- `EXACT_ABSENCE_POLICY_READY_V1` remains `false`; readiness is still the sole
  remaining production gate.
- No typed-vocabulary arm was added, removed, or newly constructed; the
  admitted-record `.ok()` discard is unchanged. Both are 3A-2's.
