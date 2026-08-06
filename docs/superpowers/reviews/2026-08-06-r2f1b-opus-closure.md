# R2f1b inactive-foundation aggregate closure

Date: 2026-08-06

## Disposition

The sole billable hard-read-only Opus closure review returned:

`VERDICT: APPROVE`

It found **0 WRONG**, **14 SMELL / DEFER**, and no open-class finding. This approves the exact inactive
R2f1b foundation for integration; it does not activate automatic deadlines, authorize a provider smoke or
compatibility run, release or deploy a binary, or replace a served operator.

PR #50 is the GitHub landing record. Its first commit is the exact reviewed tree; its second commit changes
documentation only. GitHub is authoritative for the PR's eventual open/merged state and merge commit.

## Frozen identities

- Frozen R2f1b base: `56334e98291c96c69f5a6fc37a15a8fdaf9634e0`
- Current-main / PR base: `9a0bf6e9a62efd240c74fae3635bd21bf172e77a`
- Current-main tree: `1ecebab61b746061a8e2a572abae0775897a3205`
- Preserved candidate: `1d7821309efa19f2403191ff34272eb3285c80f9`
- Preserved candidate tree: `f2ebb430fb5c35da24d227db59c9c456340e4659`
- Repaired descendant: `882f6b681efc9416c670be8d0de8ede439970f86`
- Repaired descendant tree: `f0139cf2228acb9679537d97d813af14e1ef0da3`
- Exact reviewed aggregate tree: `4fb6bfe1cebcad937204a1a798fd2e7c8f7fa0e7`
- Stable base-to-preview binary-diff SHA-256 with `core.abbrev=40`:
  `a0ee2996963cc5efb37e0d941f6fed61df5e7469e0273ae6ff94322d70e02946`
- Operator-authored exact-tree integration commit:
  `23ed6439c7e1f5315db7d0dc57502f5eafcb7aa9`
- Integration commit parent: `9a0bf6e9a62efd240c74fae3635bd21bf172e77a`
- Integration commit tree: `4fb6bfe1cebcad937204a1a798fd2e7c8f7fa0e7`

Current main changed 11 paths and R2f1b changed 21 paths; their intersection was zero. The preview was
composed branchlessly and reviewed cumulatively from `56334e9`.

## Bounded repairs

1. Migrated schema-0 configured-history rows remain canonical ticketless/no-roster legacy entries while
   malformed structured V1/V3 rows and tickets remain corruption.
2. SQLite V3 lookup now matches Memory semantics: an unknown `(task, attempt)` is an error, a known reserved
   attempt without evidence returns an empty set, reads remain attempt-scoped, and replay returns each row's
   persisted sequence.
3. The characterization regression validates the raw checked-in bundle, reseals only a copied current fixture,
   mutates scheduled timeout `180` to `181` without resealing, and proves fingerprint rejection. Production
   fingerprint validation was not weakened.

The same aggregate also closes the prior V3 task/attempt alias, replay-sequence, cleanup-owner/coherence, and
configured-roster WRONG mechanisms.

## Deterministic acceptance

One admissible full verifier against the exact preview passed:

- `git diff --check`
- `cargo fmt --all -- --check`
- locked warnings-denied all-target/all-feature Clippy
- locked workspace build
- locked workspace tests with `--no-fail-fast`

Raw parsed totals were **83 harnesses / 3,150 passed / 0 failed / 12 ignored / 0 measured / 3 filtered**.
The declared container exclusions were package `bridge-container` and exactly:

- `process::tests::terminate_reaps_child_no_zombie`
- `process::tests::term_ignoring_loop_forces_group_sigkill`
- `process::tests::drop_group_kills_descendants`

Focused controls passed for configured V3 roster isolation, Memory attempt scope, SQLite replay and unknown
attempt behavior, cleanup evidence coherence and foreign ownership, and stale fingerprint rejection. Repository
hygiene reported **39 tracked artifacts / 7 configs**.

## Review profile and limitations

- Requested advertised model selector: `opus`
- Bridge-resolved effort: `high`
- Bridge-resolved mode: `plan`
- Reviewer-observed provider identity: `claude-opus-5[1m]`
- Reviewer-observed mode: plan/read-only

The provider did not expose its reasoning-effort tier back to the session, so `high` is exact bridge request
evidence rather than independent provider metadata. There was no fallback, retry, second provider invocation,
repair, or re-review.

The reviewer independently re-parsed the raw suite log to the same totals and authenticated all retained
evidence checksums. It adjudicated every historical WRONG as fixed and found no fresh WRONG.

## Deferred hard gates

The fourteen DEFER findings remain nonblocking for this inactive foundation but constrain later R2f1b work.
Before slice 2 begins, close the remaining plan-section-6 custody/flight/snapshot tests and adjudicate the
`fs_custody` versus `local_file` extraction. Before any `AutomaticR2f1b` construction, bind activation and the
contract fingerprint into `workload_fingerprint`. Additional bounded follow-ups cover cleanup-before-primary
ordering, snapshot custody-plan coverage, the legacy SQLite schema rewrite, sequence/journal accounting,
history-growth preflight symmetry, reserve literal binding, direct `integrate_run_tree` tests, and platform/test
fixture edges.

## Landing condition

PR #50 may merge only after required GitHub checks are green and live `main` still equals exact reviewed base
`9a0bf6e9a62efd240c74fae3635bd21bf172e77a`. Target drift invalidates the reviewed composition and parks the
landing under the one-review cap.
