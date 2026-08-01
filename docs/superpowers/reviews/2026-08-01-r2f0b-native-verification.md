# R2f0b native verification

Date: 2026-08-01

## Exact artifact

- Repaired custody snapshot: `97f51144ad05ffb9ab1d2a1eca2c601ef1582548`
- Verified tree: `8bc7e3c43417ad650e2426756b4aafc5aca2582a`
- Exact pre-repair control: `ce38a4ef53bde58aa67f60b3558be532e35c0a32`
- Operator-authored code integration commit: `4ffcd5607a308ed9b1c73fe59bf2ff71b3f72889`
- Integration parent: `1a8cfc0020c0979b7a11724a7a39536dce41a680`

The integration commit has the verified tree as its exact tree and the previously live `origin/main` as
its sole parent. Commit construction therefore changes custody metadata, not reviewed source bytes.

## Deterministic gates

The following gates ran on the exact verified tree:

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- The four changed example configurations validated successfully:
  `a2a-bridge.containerized.podman.toml`, `a2a-bridge.containerized.toml`,
  `a2a-bridge.multi-agent.toml`, and `a2a-bridge.workflows.toml`.
- `cargo run -p a2a-bridge -- validate --repo-hygiene`: pass, with 39 tracked workflow artifacts and
  7 validated example configurations.
- `cargo test --workspace --all-features --no-fail-fast -- --quiet`:
  **3,079 passed / 9 failed / 12 ignored**.

The nine failures were:

1. `r2f0b_missing_trusted_cwd_is_rejected_by_the_production_resolver`
2. `r2f0b_private_foundation_uses_the_production_trusted_cwd_resolver`
3. `final_fence_rederives_fixed_foundation_before_publish`
4. `generated_sources_reopen_under_retained_foundation`
5. `source_generation_refuses_replaced_fixed_foundation`
6. `claimed_support_one_shot_rejects_replaced_fixed_foundation`
7. `completed_work_reuses_retained_fixed_foundation`
8. `r3d_manual_run_reuses_retained_fixed_foundation`
9. `scheduled_standing_run_reuses_retained_fixed_foundation`

An exact-base, same-environment control selected the 301 compatibility-schedule tests and produced
**292 passed / 9 failed**, with the same nine failing names. Both candidate and base failed during private
fixture-foundation setup before the intended assertions ran. The wording differed, but neither run reached
the target behavior, so those observations are inadmissible as R2f0b regression evidence. The failures
remain reported and deferred; they were not re-baselined or silently fixed.

## Fail-first behavioral controls

Each of the six Sol `WRONG` repairs has a behavioral control that failed on exact base `ce38a4e` and passed
on tree `8bc7e3c4`:

- direct unary provider reachability;
- workflow preflight attempt accounting;
- provider-future poll/dispatch ordering under cancellation;
- delegated peer response-loss accounting;
- exact 1,024-entry collector cap plus sticky overflow;
- implement-review attempt-owned activity and terminal evidence.

## Not verified

No authenticated provider, billable compatibility case, production-server request, release artifact,
deployment, or running-operator replacement was exercised. The nine fixture-blocked schedule behaviors
were not reached. The one authorized Sol review inspected the pre-repair tree and was not rerun after the
closed repairs; its exact verdict and the repair disposition are recorded separately.
