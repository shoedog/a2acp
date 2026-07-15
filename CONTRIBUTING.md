# Contributing to a2a-bridge

Thanks for your interest. Two things to know up front, honestly stated:

## Project stance: maintained, not (yet) supported

a2a-bridge is an actively developed personal tool published as a reference
implementation. Issues and pull requests are welcome and read, but there are **no
support commitments, no SLAs, and no API/config stability guarantees pre-1.0**.
Breaking changes land when the design needs them (each significant one is recorded in
`docs/adr/`).

## Issue intake and lifecycle

GitHub Issues are the canonical intake for bugs, compatibility regressions,
enhancements, and agent wedge incidents. Choose the matching structured form and
provide the smallest safe reproduction or outcome statement it requests. Do not put
secrets, credentials, private prompts, sensitive logs, or security vulnerabilities in
a public issue; use the repository's private security-advisory link for vulnerabilities.

Labels are orthogonal. A form applies only its `kind:*` label and `status:triage`;
maintainers separately classify `area:*`, `priority:*`, lifecycle `status:*`, and
`environment:*` after reviewing the evidence. Existing GitHub default labels remain
available for compatibility with earlier issues and pull requests.

Once work is accepted or scheduled, maintainers link the issue to the applicable
reliability-roadmap increment, design, or implementation plan. Pull requests should
close the canonical issue with a [supported closing
keyword](https://docs.github.com/en/issues/tracking-your-work-with-issues/linking-a-pull-request-to-an-issue),
include regression and edge-case tests for changed behavior, and record the review and
verification evidence needed by that increment.

## Contributor License Agreement (required)

All contributions require signing the [Individual CLA](CLA.md). It grants the project
owner the right to sublicense and re-license contributions (this is what keeps
dual-licensing possible for a single-owner AGPL project). The CLA-Assistant bot will
prompt you on your first pull request — signing is a single PR comment:

> I have read the CLA Document and I hereby sign the CLA

If you can't or don't want to sign, that's completely fine — file an issue describing
the change instead, and it may get implemented independently.

## Practical notes

- License: **AGPL-3.0-only** (see `LICENSE`).
- Build: pinned toolchain in `rust-toolchain.toml`; `cargo build --all-targets` (on
  memory-constrained machines use `-j 1` for `--all-targets` test builds).
- Gates that must stay green: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo deny check`, per-crate coverage floors (see
  `.github/workflows/ci.yml`), and `a2a-bridge validate --repo-hygiene`.
- Architecture orientation: `README.md`, then `docs/adr/` (decisions), then
  `docs/2026-07-03-strategic-analysis.md` (current priorities).
- Agent-facing quickstart: `AGENTS.md`.
