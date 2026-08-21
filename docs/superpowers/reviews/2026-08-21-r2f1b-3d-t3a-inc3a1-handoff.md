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
