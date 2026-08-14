# R2f1b 3c2 Task A1 owner-extension Sol/xhigh closure

Date: 2026-08-14

Reviewed commit: `5cbeea1ed882afe448d3825984af9a3ed74bcb58`

Exact parent: `6616753bf479d8775381eb9ef1d7237f5660514c`

Execution: `exec-b0c32fba22f61b89edbdb4a880e5f22a`

Attempt: `attempt-304c87ee714e2c212160a84785a39b3b`

Terminal artifact: 4,741 bytes, SHA-256
`d37fe339c94ed0abe20934f7c75ec6c5be6db40d7cde85bacb5d90de023966dd`.

The sole closure was a hard-read-only Codex `gpt-5.6-sol` / xhigh pass. It
verified exact head/parent and a clean worktree, read the full one-file diff and
source, and used bounded exact-symbol search because the configured navigation
services were not callable in the review session. It did not edit, build, test,
invoke another provider, or access the network.

## WRONG findings

No open WRONG findings.

- **Inherited restoration race: FIXED.** Unexpected successful capture now
  performs exactly one no-replace rename, leaves the captured entry as
  protective debt, and returns `Unknown`. Custody substitution at the old
  restoration boundary cannot move unrelated C into target because there is no
  second rename.
- **Inherited false no-effect result: FIXED.** Every post-attempt `Io` now
  returns `Unknown` immediately. The only identity observation before that
  return is the pre-capture probe, so hard-link-back cannot prove
  `RefusedNoEffect`.

## SMELL findings

None.

## Evidence assessment

- Exact scope is `crates/bridge-core/src/fs_custody.rs`: 96 additions and 43
  deletions, 30 changed production lines and 139 total changed lines.
- Expected capture, pre-capture incomplete-identity refusal,
  `PlatformUnsupported -> CompileUnsupported`, and the unchanged non-Unix stub
  remain intact.
- Repository-wide search found no production caller, persistence encoding,
  route, HTTP/API projection, or V3 activation for this A1 surface.
- The two regressions are constructively parent-breaking: `6616753b` performs
  the removed second rename and the removed post-`Io` identity probe.
- Supplied host gates are corroboration because the reviewer was read-only:
  focused 11/0 and 77/0; workspace 3,999/0/13; check, warnings-denied Clippy,
  release build, deny, hygiene, formatter, and diff checks all green.
- Real Windows compilation/execution remains excluded. The repair changes only
  Unix implementation and tests and leaves the non-Unix source unchanged.

Confidence: **97/100**. A real Windows gate and independently retained mutation
artifacts would raise it. An external V2 caller or projection missed by source
search would lower it. Any second production rename, post-`Io` identity probe,
head mismatch, or parent-green new regression would collapse it.

VERDICT: APPROVE

SUMMARY: Both inherited custody races are fixed fail-closed, the preserved contracts and scope remain intact, and only the disclosed Windows execution exclusion remains.
