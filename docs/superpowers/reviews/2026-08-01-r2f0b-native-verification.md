# R2f0b native verification

Date: 2026-08-01

## Exact artifact

- Repaired custody snapshot: `97f51144ad05ffb9ab1d2a1eca2c601ef1582548`
- Verified tree: `8bc7e3c43417ad650e2426756b4aafc5aca2582a`
- Six-finding immediate-base control: `ce38a4ef53bde58aa67f60b3558be532e35c0a32`
- Operator-authored code integration commit: `4ffcd5607a308ed9b1c73fe59bf2ff71b3f72889`
- Integration parent: `1a8cfc0020c0979b7a11724a7a39536dce41a680`
- Initial evidence/docs main: `666abb8a24f61b685219ac725f5e533b31f818a4`
- Operator-authored post-merge fixture repair: `2744cb13db336b8fa99db9c48e638b99c161fd82`
- Exact pre-R2f0b main CI control: run `30605640651` on `1a8cfc0020c0979b7a11724a7a39536dce41a680`
- Failed post-merge CI: run `30704164902` on `666abb8a24f61b685219ac725f5e533b31f818a4`
- Green replacement CI: run `30706571173` on `2744cb13db336b8fa99db9c48e638b99c161fd82`

The integration commit has the verified tree as its exact tree and the previously live `origin/main` as
its sole parent. Commit construction therefore changes custody metadata, not reviewed source bytes.

## Deterministic gates

The following initial integration gates ran on the exact verified tree:

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
3. `final_fence_rederives_scheduled_claimed_support_and_manual_sources`
4. `generated_sources_reopen_and_rederive_every_foundation_binding`
5. `source_generation_refuses_unknown_rows_and_caps_above_the_profile_maximum`
6. `claimed_support_one_shot_reselects_foundation_effects_and_revocation`
7. `completed_work_reuses_for_standing_and_manual_authority_without_new_effects`
8. `r3d_manual_uses_manual_effect_authority_and_only_active_grant_headroom`
9. `scheduled_standing_reselects_exact_grant_and_ledger_policy`

An exact-base, same-environment control selected the 301 compatibility-schedule tests and produced
**292 passed / 9 failed**, with the same nine failing names. Both candidate and base failed during private
fixture-foundation setup before the intended assertions ran. The wording differed, but neither run reached
the target behavior. That control establishes only that the six-finding repair delta from `ce38a4e` did not
introduce the failures; it does not control the cumulative R2f0b change.

The correct cumulative attribution control is exact pre-R2f0b main `1a8cfc0` in the same GitHub Actions
workflow. Run `30605640651` was green there, while post-merge run `30704164902` failed workspace coverage
with **840 passed / 9 failed** in the main binary. The nine failures were therefore a cumulative R2f0b
regression, superseding the initial deferred disposition.

Two shared-fixture mechanisms accounted for the complete known population:

- On Linux/GitHub, the fixture rewrote scheduled session CWDs beneath the checkout while production policy
  remained correctly pinned to `/Users/wesleyjinks/code`; every rewritten row was outside the approved root.
- On macOS, `tempfile::tempdir()` retained a `/var/...` spelling while descriptor-backed reads canonicalized
  to `/private/var/...`, so the copied foundation falsely appeared to escape its root.

Commit `2744cb1` canonicalizes the copied foundation root, preserves the production policy pin and resolver,
uses a retained real CWD below the approved root when it exists, and uses the resolver's intentional lexical
offline branch when that root is absent. It also preserves a symlinked approved root's lexical spelling; the
production resolver remains responsible for canonical object containment.

## Post-merge repair gates

- The original macOS positive regression failed before the repair and passed after it: **1 / 0**.
- The paired invalid-CWD negative remained fail-closed: **1 / 0**.
- Sol's symlink-root finding received a new fail-first regression: **0 / 1** before the one-line correction,
  then **1 / 0** after it.
- Complete compatibility-schedule slice: **302 passed / 0 failed**.
- Final solitary full workspace: **3,089 passed / 0 failed / 12 ignored**.
- Format, diff check, warnings-denied all-target/all-feature Clippy, and repository hygiene **39 / 7** passed.
- Replacement GitHub run `30706571173` passed Build/Lint/Coverage in **11m11s**, Windows unsupported-target
  in **1m18s**, and macOS store in **56s**. This includes the exact Linux workspace coverage gate that failed
  in run `30704164902` plus every per-crate threshold.

One local instrumented coverage attempt intermittently failed the unrelated macOS
`bridge-controller::implement_resume::operation_lock_excludes_same_run_but_not_another_clone` test. Its
feature-identical exact test and complete `bridge-controller` library passed alone, and the final ordinary
workspace passed it. No lock code is in this repair; the observation is reported but not attributed or folded.

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

One authenticated Sol/xhigh read-only provider turn reviewed the fixture repair. No billable compatibility
case, production-server request, release artifact, deployment, or running-operator replacement was exercised.
The initial Sol rejection remains the verdict on its frozen pre-repair production tree; the distinct post-merge
fixture review and its folded finding are recorded separately in the repair disposition.
