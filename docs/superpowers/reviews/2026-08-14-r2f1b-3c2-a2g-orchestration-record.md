# R2f1b 3c2 A2-G orchestration record

Date: 2026-08-14

Orchestrator: Fable, per the binding handoff
[`2026-08-14-r2f1b-3c2-fable-orchestration-handoff.md`](2026-08-14-r2f1b-3c2-fable-orchestration-handoff.md).

Task contracts: the salvage plan
[`2026-08-12-r2f1b-3c2-salvage-redesign.md`](../plans/2026-08-12-r2f1b-3c2-salvage-redesign.md)
and the custody adjudication
[`2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md`](2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md)
are binding for every task's owned paths, behavior, red schedules, gates,
line caps, stop/split conditions, and review caps.

## Identity preflight (2026-08-14, read-only)

All checks passed; no drift found.

- `origin/main` after fresh fetch = `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`,
  exactly the recorded landed slice-3 base. Local `main` identical.
- Planning branch `agent/r2f1b-pre-slice2-custody-plan` HEAD `9c6565e0`;
  decision checkpoint `334201aa957fedd4c5c50e90f3c99ddfc0db231f` verified as
  ancestor.
- Feature worktree `.claude/worktrees/s3c2` HEAD =
  `2c6505eab220d0c732801882c725eada4ea71d21` on
  `feat/r2f1b-3c2-api-authority`, clean; preserved rejected artifact
  `530992b7` and Task A lineage root `771c0fb8` verified as ancestors. The
  preserved feature remains non-foldable and untouched.
- Retained A1 clone `/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5`
  is clean at exactly `5cbeea1ed882afe448d3825984af9a3ed74bcb58`, parent
  `6616753b`, lineage `517703cb -> bc262ad4 -> 6616753b -> 5cbeea1e`.
- Untracked files in the planning checkout are the four recorded pre-existing
  user-owned example configs plus user-owned
  `SSOT_AGENTS_BRIDGE_COORDINATION.md`; all preserved and excluded, none is a
  lane artifact.
- Custody finding and remedy: the A1 lineage objects existed only in retained
  clones (`5cbeea1e` solely in `impl-77617-f18mbkc5`). A second durable copy
  now exists as local unpushed branch `salvage/r2f1b-3c2-a1` at exact
  `5cbeea1e` in `/Users/wesleyjinks/code/a2a-bridge`. This is preservation
  only, not integration.

## A2 dispatch declaration

- Exact frozen input: `5cbeea1ed882afe448d3825984af9a3ed74bcb58`
  (closure-approved A1; APPROVE 0W/0S at 97/100).
- Contract: A2 "trusted route binding and sibling operation lease" per the
  custody adjudication (JournalRootBindingV2 / JournalRootCustodyV2 / owned
  operation guard; externally supplied binding; flock-then-re-prove; remove
  revalidate-as-authority and the path-exposing lock result).
- Line caps: stop/split before exceeding 220 changed production lines or 500
  total changed lines relative to `5cbeea1e`.
- Declared review cap (before dispatch): one implementation attempt with one
  independent implementation review; a closed enumerable rejection permits one
  targeted repair on the same artifact plus one closure review; at the cap,
  classify before acting — shrinking non-repeating findings may receive only a
  disclosed operator extension; repeated or open-class findings park A2 for
  design; never restart from a fresh implementation.
- After acceptance: run the salvage plan's exact common full-repository gate
  on the committed candidate at the operator boundary (host), record command,
  exit status, totals, exclusions, and exact HEAD; a red gate blocks A3.

## A2 dispatch identities

- Bridge binary: fold worktree release build of exact main `42249b3d`,
  34,090,224 bytes, SHA-256
  `18adb745020fc3a95ed210e81969670d89d5f0c20b4a3e5e02cc3e3083166168` (the same
  binary recorded by the design and custody rounds).
- Brief: `/private/tmp/a2a-r2f1b-3c2-a1.WOl3oH/task-a2.md`, SHA-256
  `ec60b84e853647ac50170be44a58090ec80925d0c3e1b45dda6f1a2c233ce0b6`; a
  byte-identical durable copy is committed beside this record as
  [`2026-08-14-r2f1b-3c2-task-a2-brief.md`](2026-08-14-r2f1b-3c2-task-a2-brief.md).
- Config: `/private/tmp/a2a-r2f1b-3c2-a1.WOl3oH/examples/task-a2.toml`,
  byte-identical copy of the A1 dispatch config `task-a1.toml`, SHA-256
  `cdeaf0cb2f4dfc812f434028ee3dcb4707915e8c24042cadd4f26f7c157e06fc`
  (impl = containerized codex `gpt-5.6-sol`/xhigh, `[implement]
  max_attempts = 1`, review workflow `implement-review-sol`, hermetic verify
  with the recorded container skips).
- Source repository for the clone: the retained A1 clone
  `/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5` with
  `--base-ref 5cbeea1ed882afe448d3825984af9a3ed74bcb58` (the exact A1 dispatch
  pattern; a branch name is never task input).
- Invocation: `implement --input task-a2.md --repo <retained A1 clone>
  --base-ref 5cbeea1e... --config task-a2.toml --strict-brief --lang rust`.

## Pre-dispatch probes (all green, 2026-08-14)

- `validate --config task-a2.toml`: ok (6 agents, 19 workflows, 3 prompts).
- `doctor --config task-a2.toml`: 51 ok / 1 warn / 0 fail; the single warn is
  the known kiro container-adapter provenance warn. impl runtime, locked
  network, image (`a2a-toolchain:latest`,
  `sha256:bb09479fd020...f4ff3086`), creds mount, adapter 1.1.7 /
  codex 0.145.0 all ok.
- `models --agent impl --json`: live in-container probe succeeded; current
  `gpt-5.6-sol[xhigh]` advertised. This exercises container spawn plus
  ChatGPT-auth session/new after the 2026-08-14 14:01 a2a-creds write, so the
  single-token-family rotation flaw is not currently blocking fresh container
  sessions.
- Egress stack up: `a2a-egress-proxy`, `a2a-verify-proxy`,
  `a2a-egress-internal`/`a2a-verify-egress` networks present. Disk headroom
  304 GiB.

## Execution log

- 2026-08-14: record opened; A2 dispatched (run id appended at the next
  stable point).

## Non-scope reaffirmed

No OpenRouter/OpenCode implementation, live/billable provider turn beyond the
bounded bridge dispatch turns, compatibility execution, production V3 arming,
production request-journal root, 3d work, automatic deadlines, release,
deployment, or running-operator mutation. The two-field
`CleanupReportV1 { result, checkout }` carry-forward remains binding; only
`Complete + Complete` may become `Complete`.
