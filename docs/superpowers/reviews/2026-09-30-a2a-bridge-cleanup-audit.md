# a2a-bridge cleanup audit

Snapshot: 2026-10-01T03:03:28Z; ancestry bound to origin/main `251289fe3fada54a740be4879fa48e7adeb9f3cb`.

Read-only audit; no worktree, branch, build, image, volume, source, or evidence deletion performed. Disk counts are allocated bytes from du; APFS actual reclaimed space can differ.

## Useful build candidates

| Directory | Allocated GiB | Evidence |
|---|---:|---|
| `/Users/wesleyjinks/code/.a2a-provider-refresh-post-parser-20260905/target` | 0.678 | clean checkout; HEAD ancestor of bound main; no matching open file or container mount in snapshot |
| `/Users/wesleyjinks/code/a2a-bridge-operator-main/target` | 3.280 | clean checkout; HEAD ancestor of bound main; no matching open file or container mount in snapshot |
| `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/rustls-pr104-20260916/target` | 11.888 | clean checkout; HEAD ancestor of bound main; no matching open file or container mount in snapshot |

Root `target/debug/incremental` is about 4 GiB. Root `target/debug/deps` is about 8.3 GiB, but is useful for subsequent host verification; retain it for this lane. Root `target/release` contains the running candidate and is protected. Before deleting an old target, inventory and preserve any release binary used as compatibility evidence; recheck active use.

## Clean merged worktrees

The following ten checkouts have clean tracked/untracked status and HEADs that are ancestors of bound main. Ignored entries were separately inventoried; only generated target directories appeared. Removal would still require a final custody and liveness check.

- `/Users/wesleyjinks/code/.a2a-main`
- `/Users/wesleyjinks/code/.a2a-opencode-go-docs-verify-20260904`
- `/Users/wesleyjinks/code/.a2a-provider-refresh-post-parser-20260905`
- `/Users/wesleyjinks/code/.a2a-provider-refresh-runtime-dispatch-verify-20260905`
- `/Users/wesleyjinks/code/.a2a-provider-refresh-secret-scan-base-20260904`
- `/Users/wesleyjinks/code/a2a-bridge-operator-main`
- `/Users/wesleyjinks/code/a2a-bridge-r2f1b-4i-control-20260904`
- `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/r2f1b-4j-gate-20260906`
- `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/rustls-pr104-20260916`
- `/Users/wesleyjinks/code/a2a-r2f1b-4i-integrated-gate-20260905`

## Branches and protected material

227 local branches: 97 are merged and not checked out. Deleting their refs would reclaim negligible space; do not prioritize them.

Dirty or non-ancestor worktrees are retained. Non-ancestry can reflect squash/re-authored integration; it is not proof that work is unlanded. Current quarantine impl-35818-c2c16khq, its original Rejected checkpoint, independent round3 approval, host receipts, and the fresh base control remain protected.

The bridge storage report counted 106 direct a2a-bridge standalone clones totaling 15.65 GiB; three were dirty. Its main-containment answers were 11 yes_head, 10 yes_tree, 81 unknown, and 4 unavailable. These stale/ref-limited answers do not authorize clone deletion. The safe bridge build-target dry run proposed less than 1 MiB in fixtures, so it offers no meaningful savings.

## Active-use snapshot

lsof exited 0; Docker inventory exited 0; 0 inspect failures. No selected old worktree had an observed open file or container mount. Current root and running bridge are active. Snapshot absence does not prove future absence; recheck before removal.

Raw inventories: cleanup-audit.json, cleanup-liveness.json, storage-report-before-git.json, storage-build-targets-dry-run.json under `/private/tmp/adr0041-2b3b-20260929`; a second snapshot is retained under the controller checkout's `.git/a2a-evidence/cleanup-audit-20260930`.

## Concrete optional cleanup

Limit removal to the three target directories in the table and root `/Users/wesleyjinks/code/a2a-bridge/target/debug/incremental`, about19.8 GiB allocated in total. Keep every worktree, branch, source file, evidence directory, root dependency cache and current release. Two old top-level release binaries were copied and SHA-256 verified into `/private/tmp/adr0041-2b3b-20260929/cleanup-retained-binaries`; receipt.json binds original and retained paths. Recheck directory identities, clean/merged state and active use immediately before removal. No deletion is authorized by this audit alone.
