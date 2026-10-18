# Current roadmap

**Updated:** 2026-10-18

## Priority order

1. **H1-3 — review-quality evaluation across new reviewer models.** Research first: establish the
   evaluation question, corpus, and judge method before choosing an implementation slice.
2. **H1-2 — cost and quota governance.** Write the specification first, including provider-cost truth,
   admission policy, and operator visibility.
3. **H0-3 and verification reliability.** Enforce `allowed_cwd_root` for local `run-workflow`, and resolve
   test flakes [#171](https://github.com/shoedog/a2acp/issues/171) and
   [#173](https://github.com/shoedog/a2acp/issues/173).
4. **Slim serve-swap lane.** Follow the next flight with validate + doctor, drain in-flight turns, restart,
   PONG check, and a rollback record. It combines the useful deferred R2g work with H1-4.

The reliability program is now in **maintenance**: it has no active slice. Its authoritative close-out and
reopen conditions are in the [`reliability execution roadmap`](reliability-execution-roadmap.md).

**M4 observability — no Slice 3b scheduled.** The resume rule is substantially met; the operator store is
about 14 MB, so bounded-storage work is not currently scheduled.

## Historical rationale: why reliability was first

The bridge has recently paid the compatibility tax repeatedly:

- a new Codex model exposed both model-ID and stale-adapter drift on the host;
- the same model then failed in the shipped container for a different reason—redundant interactive
  authentication in a pre-authenticated, browserless image;
- the Fable incident initially looked like adapter/model drift, but R1 proved the historical failure was
  a no-DNS managed execution sandbox and found a separate reader-settings prerequisite;
- adapter packages, embedded agent CLIs, Rust protocol crates, and model catalogs change on different
  cadences.

These were product-path failures, not incidental maintenance. The delivered work established the relevant
compatibility evidence, phase-specific errors, pinned and floating lanes, and release-gate foundations.

For the majority case—read-only work on trusted own repositories—the host path is first-class and the
container is opt-in defense-in-depth. Reliability work must preserve that usable path while preventing
silent security downgrades: untrusted reads and all write-capable work remain container-required, and
today's trusted host fallback is an explicit operator-selected entry. Any future automated fallback
must also be audited.

## Resume rule for M4

Slice 3b may resume only after the reliability program has at least:

- isolated and dispositioned the Fable failure;
- established a repeatable minimal host/container smoke harness;
- recorded exact adapter/CLI/model compatibility in a checked-in matrix;
- preserved the failing phase and underlying error instead of only `AgentCrashed`;
- defined pinned-production and floating-current release lanes.

There is no committed Slice 3c design. Slice 3b completes the original M4 bounded-storage goal; 3c is
only a reserved decision point for separately justified administration or archival work.

As of 2026-10-18, the five criteria are substantially met:

- **Fable:** dispositioned by R1.
- **Smoke harness:** delivered by R2c/R2d.
- **Checked-in matrix:** delivered through R3a–R3d3.
- **Phase-preserving errors:** delivered by R2b; issue #47 remains as the residual adapter-error projection gap.
- **Pinned/floating lanes:** defined and manually runnable; scheduled activation is deferred.

M4 Slice 3b remains designed but unscheduled because the operator store is about 14 MB. Slice 3c remains a
reserved decision point for separately justified administration or archival work.
