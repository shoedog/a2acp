# Changelog

All notable changes to this project are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project is pre-1.0: no
API/config stability guarantees yet, but breaking changes are called out explicitly per
release (see [`docs/adr/`](docs/adr/) for the full architectural record).

## [Unreleased]

## [0.2.1] - 2026-07-10

### Fixed

- Containerized and already-logged-in Codex agents can declare `pre_authenticated = true`, preventing
  the bridge from re-invoking codex-acp's interactive ChatGPT browser login before `session/new`.
  Shipped Codex configs now use the setting, restoring gpt-5.6-sol model/effort selection and prompt
  turns with codex-acp 1.1.2 in browserless containers.

## [0.2.0] - 2026-07-03

### Changed

- **Relicensed Apache-2.0 → AGPL-3.0-only**, while the project had a single copyright
  holder, plus a Contributor License Agreement (CLA) and a CONTRIBUTING.md stating the
  "maintained, not (yet) supported" stance (`45bf05b`).
- **Wave 1 — runtime & CI hardening** (`0d4d12c`): SQLite opened with WAL +
  `synchronous=NORMAL` + `busy_timeout` (warn-not-fail); the `Configuring` claim-state
  fix so lazy agent spawn no longer holds the registry lock across resolve; worktree
  git invocations moved off the async runtime onto `tokio::process`; CI toolchain
  pinned to 1.94.0 with the workspace MSRV inherited by every manifest.
- **Wave 2 — identity & docs** (`db4a8b3`): README rewritten to current capability
  (command table, crate table, troubleshooting, sample output); 172 one-shot
  dev-process artifacts moved (pure renames) into `docs/history/`, shrinking the
  workflow-artifact hygiene allowlist 208 → 37; ADR-0032 sandbox tier model plus
  example tier presets; AGENTS.md/onboarding.md synced to the current CLI.
- **Wave 3 — CLI polish, doctor, A2A wire safety** (`18e1c5a`): uniform `--help` across
  every subcommand via dispatcher-level interception; `a2a-bridge doctor`, a
  read-only bounded preflight (9 checks, host-vs-sandbox aware, `--json`); A2A golden
  wire fixtures pinning the `a2a-lf` SDK boundary (redaction, `TaskSpecInvalid`
  passthrough, ordered SSE frame contract).

### Removed

- **BREAKING:** silent config auto-write removed. Bare `a2a-bridge` / `serve` / `mcp`
  invocations that can't resolve a config now error instead of scaffolding one —
  `init` is the only command that writes a config file.

## [0.1.0]

The pre-relicense era: the initial ACP↔A2A bridge (multi-agent registry, warm ACP
sessions, the workflow DAG engine with fan-out/pipeline/fan-in, containerized
sandboxing, streaming task reattach, and the MCP/Coordinator surface), built as a
sequence of increments and slices under the original Apache-2.0 license before this
project adopted semantic versioning. Not retroactively tagged — see
[`docs/adr/`](docs/adr/) for the architecture decisions and
[`docs/history/`](docs/history/) for the detailed increment/slice archaeology.
