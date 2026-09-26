# Provider refresh 2026-09-25 (Opus 5.5) — review record

**Branch:** `chore/claude-acp-opus55-20260925`, base `707b7f38`
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp` 1.13.1, driven by the served
operator's release binary (bridge 0.3.1, SHA-256 prefix `07aee33e487fdce6`) with `prompts/review-sol-risk.md`
**Declared cap:** 2 rounds. Rounds 3 and 4 were disclosed extensions under the owner's standing authorization to
continue while review converges.
**Execution evidence** (live lanes, image identities, promotion): the 2026-09-25 section of
[`docs/compatibility.md`](../../compatibility.md). This file records the review only.

| Round | Reviewed range | Execution / attempt | Raw result SHA-256 | Verdict |
|---|---|---|---|---|
| 1 | `707b7f38..17a7e743` | `exec-18de9a56…` / `attempt-b3d8a560…` | `c2f1982a805815bf5dec54ef84317ec68e1700ca6626eecf87c8b86e6d549bb6` | REJECT |
| 2 | `17a7e743..4f6748b0` | `exec-4f027229…` / `attempt-68f6ae51…` | `305d597467d7efda50738226054a9f8f90b34f35377d5b242cdf71f9979d34d4` | REJECT |
| 3 | `4f6748b0..205f0eba` | `exec-ebaf9d9f…` / `attempt-4851d1d5…` | `0124637e9b9b2fe56a116df0e2bf10dcb20b3aabfff850d6cd565417db69c4d3` | REJECT |
| 4 | `205f0eba..3fa7df6d` | `exec-633a9b46…` / `attempt-a597b197…` | `f6764e8a031d387e8c776a0e03c32d0a0052d4269840a345ae149ccb4451d6e3` | REJECT |

A first dispatch of round 1 was refused before any provider call because the brief lacked `task-type`
front-matter. It is not counted.

## Findings and dispositions

1. **Round 1, W1 — the pin guard accepted an additive floating install.** Folded in `4f6748b0`: each selector, label
   key, and Kiro build argument must appear exactly once. The new negative test failed against the old helper and
   passes after the fold.
   - Round 2 found the same class again: a bare `@openai/codex` operand or `npm update` still passes a substring
     check.
   - Classified **open-class** and **DEFERRED** to the
     [provider-refresh ledger](../../reliability-execution-roadmap.md) with an image-inspection fix. Round 3 judged
     that disposition adequate for this dependency bump: the current Containerfile has no later npm mutation, and
     the built image's installed manifests and labels were inspected.
2. **Round 1, W2 — the runbook said a `CLAUDE_CODE_OAUTH_TOKEN` host fails the credential-file preflight.**
   Resolved in rounds 2–4 across three wording issues:
   - `4f6748b0` stated the token bypass;
   - `205f0eba` listed every exception in `claude_credential_source`;
   - `3fa7df6d` spelled all five `CLAUDE_CODE_USE_*` names in full.

   Round 4 confirmed the result as RESOLVED.
3. **Round 3 — the deferral sat under the ADR-0041 2B2a ledger heading.** Resolved in `3fa7df6d` with a separate
   heading; round 4 confirmed it.
4. **Round 4 — the ledger cited execution evidence as review provenance.** Resolved by this record, which the
   ledger now cites.

**Convergence:** WRONG findings ran 2 → 2 → 2 → 1. After round 1, every finding was documentation precision or
provenance in text this branch added; none was a code or pin defect. The only code-level class, the pin guard, was
deferred as open-class.

## Verification

- The full workspace suite ran on `17a7e743` with `CARGO_INCREMENTAL=0` and exited 0. Summed `test result:` lines
  show 4,502 passed, 0 failed, 13 ignored. Later commits changed only the pin test file and documentation.
- `cargo fmt --check`, repository hygiene, and warnings-denied Clippy for the `a2a-bridge` tests passed.
- The pin test file passes 4/4.
