# R2f0p parallel implementor flight — corrected native verification

- **Date:** 2026-07-30
- **Candidate:** `0e41e4d3aee52141ebc5c3f9719ae4af065bc549`
- **Tree:** `5afbf96fee2d2beccb8724a716f0ffbc83cc9455`
- **Base and merge base:** `bc8c153d2f108566a01a36ee68be9e45ece628c4`
- **Host tools:** rustc/cargo 1.94.0; Apple Git 2.50.1
- **Release executable:** `target/release/a2a-bridge`, 31,470,704 bytes, SHA-256
  `9e9a798e0b09d16898a3e22930a75ce24ad3fdbd1d796a35f911d24638640e68`

This candidate folds both `WRONG` findings from the
[initial Sol review](2026-07-30-r2f0p-sol-review.md): reusable run-operation locks now retain one persistent
pathname/inode, while crash-liveness leases retain their removable clean-drop behavior; and an already-integrated
tree now crosses a verify-only ref transaction instead of treating an up-to-date push as a compare-and-swap.

## Fail-first and focused correction evidence

- The operation-lock regression deterministically opens inode I before the prior guard drops, reacquires the named
  lock, and proves the earlier opener cannot lock a detached predecessor. The old unlink-on-drop mechanism permits
  that split. Complete liveness focus: **5 passed / 0 failed**, including the unchanged clean-drop and crash-residue
  lease controls.
- The already-integrated production-path regression installs a `reference-transaction` hook and requires
  `prepared` plus `committed`. The old same-value push sends no update command and cannot satisfy that assertion.
  A movement edge case advances the destination before the compare, returns `Unlanded`, leaves the new destination
  untouched, and retains the reviewed clone. Complete Git-backed merge focus: **15 passed / 0 failed**.
- The controller same-run resume/merge exclusion test passes **1 / 0** with the persistent guard.
- Strict Clippy first rejected repair commit `dce05ab` because the test-hook helper had eight arguments. Exact
  pre-repair commit `8754d693` passed the same full-workspace Clippy command in a separate same-host worktree,
  establishing repair attribution. Commit `0e41e4d` groups `force` and `mode` into one private options value; all 15
  Git-backed tests and the exact warnings-denied Clippy gate then passed. The detached control worktree was removed.

## Exact corrected-candidate gates

| Gate | Result |
|---|---|
| `git diff --check origin/main...HEAD` | PASS, no diagnostics |
| `cargo fmt --all -- --check` | PASS, no diagnostics |
| `cargo check --workspace --all-targets --all-features --locked` | PASS |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | PASS, no warnings |
| `cargo test --workspace --all-features --locked -- --test-threads=1 --quiet` | PASS: **2,998 passed / 0 failed / 12 ignored / 0 measured / 0 filtered**, 77 emitted result groups, 61 nonempty |
| `cargo build --release --workspace --all-targets --all-features --locked` | PASS, no warnings; 1m19s |
| `cargo run -p a2a-bridge --locked -- validate --repo-hygiene` | PASS: **38 tracked artifacts / 7 example configs** |
| final candidate/ancestry/clean-state authentication | PASS |

The first invocation of the canonical full suite crossed the command tool's yield boundary without retaining its
output handle. Its cargo process was allowed to finish alone and was classified inadmissible: no exit or totals from
that invocation support this report. The separately retained evidence run above captured all 18,912 output
characters, exited 0, and was parsed from every `test result` record; no failed group or failure marker occurred.

The 12 ignored cases remain repository-declared authenticated-provider/local-runtime tests. No provider, container,
compatibility, production server, installed client, running operator, release, deployment, or live controller was
exercised or changed. This deterministic evidence does not replace the pending independent closure review.
