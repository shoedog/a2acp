# Contributing to a2a-bridge

Thanks for your interest. Two things to know up front, honestly stated:

## Project stance: maintained, not (yet) supported

a2a-bridge is an actively developed personal tool published as a reference
implementation. Issues and pull requests are welcome and read, but there are **no
support commitments, no SLAs, and no API/config stability guarantees pre-1.0**.
Breaking changes land when the design needs them (each significant one is recorded in
`docs/adr/`).

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
