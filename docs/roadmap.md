# Current roadmap

**Updated:** 2026-07-11

## Priority order

1. **P0 — bridge reliability and compatibility.** Make upstream agent, adapter, SDK, model, and
   container changes detectable and diagnosable before they consume feature work. The active plan is
   [`bridge-reliability.md`](bridge-reliability.md).
2. **M4 observability — paused after Slice 3a.** Slice 3a merged in
   [PR #19](https://github.com/shoedog/a2acp/pull/19). Slice 3b remains designed but unimplemented.
   Resume from [`m4-observability-roadmap.md`](m4-observability-roadmap.md), not from an older Slice 3
   revision.

## Why reliability is first

The bridge has recently paid the compatibility tax repeatedly:

- a new Codex model exposed both model-ID and stale-adapter drift on the host;
- the same model then failed in the shipped container for a different reason—redundant interactive
  authentication in a pre-authenticated, browserless image;
- Fable currently succeeds through the direct Claude CLI but fails through the tested host ACP/bridge
  path, while the host and reader image use different `claude-agent-acp` versions;
- adapter packages, embedded agent CLIs, Rust protocol crates, and model catalogs change on different
  cadences.

These are product-path failures, not incidental maintenance. The next work should establish explicit
compatibility evidence, phase-specific errors, pinned and floating canaries, and release gates before
adding more retention behavior.

For the majority case—read-only work on trusted own repositories—the host path is first-class and the
container is opt-in defense-in-depth. Reliability work must preserve that usable path while preventing
silent security downgrades: untrusted reads and all write-capable work remain container-required, and
today's trusted host fallback is an explicit operator-selected entry. Any future automated fallback
must also be audited.

## Resume rule for M4

Resume Slice 3b only after the reliability program has at least:

- isolated and dispositioned the Fable failure;
- established a repeatable minimal host/container smoke harness;
- recorded exact adapter/CLI/model compatibility in a checked-in matrix;
- preserved the failing phase and underlying error instead of only `AgentCrashed`;
- defined pinned-production and floating-current release lanes.

There is no committed Slice 3c design. Slice 3b completes the original M4 bounded-storage goal; 3c is
only a reserved decision point for separately justified administration or archival work.
