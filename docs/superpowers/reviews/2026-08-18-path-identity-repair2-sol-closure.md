I’ll perform the bounded read-only review against `be7c6708..HEAD`, with a declared single review pass and no retries. I’ll first freeze commit/diff identity, then read every changed hunk and the complete handoff, trace the comparator through callers/persistence/projection, and separately audit each regression fixture and non-Unix cfg boundary. Prism/LSP tools are not exposed in this session, so I’ll use read-only Git/search navigation without probing for absent tooling.The checkout is clean at `39f8c3e1`; the requested delta matches the supplied 820 additions/194 deletions across exactly five files. I’m now reading the repair itself in full, then I’ll widen only around its reachable callers and persistence boundaries.The core comparator now visibly evaluates A3/A5/A6 before the case probe and rechecks both resolved ancestors before returning a computed verdict. I’m at the higher-risk boundary now: whether those observations remain tied to the repository and target that Git actually queried, and whether the test hooks truly bracket that subprocess read.I found one concrete fail-open in the B2 repair: the second resolution validates only `(dev, ino)`, not the resolved missing tail. Creating a symlink in a formerly missing component can make two paths alias while both ancestor identities stay unchanged, yet the comparator returns its stale `Different`. I’m completing the remaining caller/test/cfg audit before fixing the classification and verdict; no state-changing commands are being run.## WRONG

1. **WRONG — BLOCKER: B2’s stability check ignores resolved-tail drift.**  
   [fs_custody.rs:1717](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-core/src/fs_custody.rs:1717) re-resolves both paths but compares only ancestor `(dev, ino)`.

   Constructible state:

   - `/R/link` and `/R/foo` are absent.
   - Compare `/R/link/foo` with `/R/foo`. Initially their tails have two versus one components, producing A3 `Different`.
   - Before revalidation, create `link -> /R`.
   - Re-resolution now gives both paths ancestor `/R` and tail `foo`, but the ancestor identity remains unchanged.
   - The check passes and [returns the stale `Different`](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-core/src/fs_custody.rs:1764), although the paths now alias. Drift requires `CannotProve`.

   In production, a porcelain registration using the first spelling is discarded as unrelated at [host_git.rs:173](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/host_git.rs:173), allowing `Absent`, `BothAbsent`, and then `Authorized` at [sweep.rs:184](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/sweep.rs:184).

   Trigger: a stale nested registration plus concurrent symlink creation during comparison. Likelihood: **rare**, but reachable through arbitrary Git worktree paths and operator/concurrent-process activity. Impact: fail-open exact-absence/removal evidence and possible custody loss.

   Fix: compare the complete second `DeepestExistingPathV1` snapshots—identity, canonical path, and `missing_tail`—against the originals. Cost/blast radius: low, confined to the primitive. Add a deterministic resolver test with unchanged identities but changed tails, plus a stable-tail control. This repair only narrows stale `Different` to `CannotProve`; it does not widen `Different`.

## SMELL

1. **SMELL — DEFER: the B4 common-dir barrier can pass without proving the post-command revalidation caused the refusal.**  
   The hook runs after `spawn`, so Git may read or finish before the swap; the two renames also expose a temporary missing-`.git` interval. Moreover, the hook branch at [host_git.rs:58](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/host_git.rs:58) uses `spawn` without piped stdout/stderr, while the test merely checks `.is_err()` at [host_git.rs:592](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/host_git.rs:592). A Git failure can therefore satisfy it.

   Trigger likelihood: **plausible** under scheduler variation. Exposure is test evidence only; the production post-check is present and statically closes the named persistent-swap state. Fix: use a before-spawn seam after initial revalidation, preserve piped output, and assert the specific revalidation error. Low cost. **DEFER** because no additional production failure is established.

2. **SMELL — DEFER: the B5 end-to-end fixture does not prove its `RegistrationUnproven` came from A6.**  
   The corrected sibling at [backend.rs:10750](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/backend.rs:10750) is genuinely distinct and non-ASCII. However, after corrupting `HEAD`, the test never asserts that `git worktree list` succeeds and contains that stale registration. Both `Ok(CannotProve)` and any list `Err` become `RegistrationUnproven` at [host_git.rs:289](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/host_git.rs:289).

   Trigger likelihood: **rare** across Git versions/configurations. Impact is a false-green regression, not current production misbehavior. Assert successful porcelain output containing the exact stale path, preferably also the parser’s `CannotProve` result. Low cost; **DEFER** because static tracing confirms the tri-state is correctly persisted.

3. **SMELL — DEFER: B7 lacks an unchanged-sample positive control.**  
   The deletion and replacement tests correctly construct their invalidated snapshots, but a mutant making `sampled_entry_still_matches` always return `None` would pass both. The B3 injected test bypasses the real probe. Likelihood: **plausible** future regression; impact is fail-closed A7 over-refusal. Add a Unix test asserting an unchanged ASCII sample yields `Some(_)`. Trivial cost; **DEFER**.

## Closure assessment

| Item | Result |
|---|---|
| B1 | **Closed.** The skeleton proof is gone; A5 precedes unconditional A6 at [fs_custody.rs:1649](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-core/src/fs_custody.rs:1649). |
| B2 | **Open.** Identical spelling short-circuits correctly, but revalidation checks only ancestor identity and misses tail/canonical drift. |
| B3 | **Closed.** Sampling occurs only inside the shared ancestor at [fs_custody.rs:1585](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-core/src/fs_custody.rs:1585). |
| B4 | **Closed within the disclosed non-ABA scope.** Source/common-dir and target are rechecked after Git at [host_git.rs:214](/Users/wesleyjinks/code/.a2a-implement/impl-34701-vco2fqu5/crates/bridge-worktree/src/host_git.rs:214); regression evidence has the smell above. |
| B5 | **Closed in production.** `CannotProve` remains distinct through parsing, classification, backend settlement, claim construction, and canonical record encoding. |
| B6 | **Closed.** A3/A5/A6 resolve before the only probe-dependent A7 branch. |
| B7 | **Closed within the disclosed ABA scope.** Both alternate-hit and `ENOENT` paths revalidate the sampled object. |

A1–A7 match the pinned table for a stable resolved snapshot. A8 does not fully match because the B2 race is real resolution drift yet is accepted as stable and can return `Different`.

The B2 resolver test deterministically brackets identity drift but does not vary the canonical path or missing tail. The B3 injected probe brackets the intended call correctly. The target-creation barrier definitely falls between the two target probes. The common-dir barrier and B5 subprocess fixture have the evidence limitations above. B7’s direct tests faithfully construct deletion/replacement before the alternate lookup.

No new non-Unix ungated reference or `-D warnings` dead-code failure is apparent: Unix identity construction and Unix-only tests are gated, while portable helpers remain reachable through the public comparator. Non-Unix execution remains unverified.

The supplied host results—4,157 passed, 0 failed, 13 ignored; format and clippy green—were not rerun under the read-only contract. The checkout remained clean. The size waiver was accepted and not treated as a finding.

VERDICT: REJECT
SUMMARY: B1, B3–B7 close, but B2 still permits a fail-open stale Different when missing-tail topology changes without changing ancestor inode identity.