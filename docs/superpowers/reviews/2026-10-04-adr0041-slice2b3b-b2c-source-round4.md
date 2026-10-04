# ADR-0041 2B3b B2c source review — round 4/4

Controller custody: exact raw SHA-256 `ae5964ee39838ce993e76b3ccfc9c3e5f51dd21f3da67681166c2bafa81ba60a` and full transcript retained in private `b2c-source4-rejected-custody/`. Reviewer memory footer omitted from this tracked copy. Candidate remains isolated and unadopted.

Bound clean checkout `impl-70428-q52t65ks` at exact HEAD `4b941d4b032cbd652accd5fb18a59e0cb12d258a`, targeted parent `c4c60a60dfcbf1e04d0f4fe532d77233363aa82f`, and whole-B2c base `dc0b45bafd3538c93fe15254bd9f975ad34f4c71`. Both diffs contain only the two owned Rust files and implementation handoff; `git diff --check` is clean. I read the complete task, normative master, B2b approval, source history, handoff, production, tests, runner, and supplied receipts.

1. **WRONG — A tree-census failure still suppresses the separately owed digest reconciliation. Blocker. Confidence 99/100.**

   [`proof_reconcile`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8219) joins census, comparison, and digest with `and_then`. Any census or `final_unchanged` failure therefore prevents every digest from running, despite the helper’s stated requirement that every class run.

   Concrete regression: a routed final `fsck` runs real Git, adds `.git/0stray`, rewrites `HEAD` in place with different same-length bytes, then exits nonzero or times out. The post-exit gate checks mounts and chains, not repository bytes. Failure reconciliation reports the primary plus `FinalTree::Unexpected`, but the `HEAD` digest is never read and `FinalTree::Bytes` is absent. This is reachable through the admitted single-operator route-wrapper boundary without concurrency or special privileges. Stock Git will not do this, so likelihood is low; exposure is the exact zero-effect custody guarantee.

   The repair is bounded: retain the census/comparison result, independently run reconciling digests from the trusted baseline facts with the existing length-plus-one bound, then accumulate both failures. A discriminating regression must require the primary, `Unexpected`, and `Bytes`. The current combined tree/chain row explicitly expects `digests_complete=false` when census fails ([test](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:28568)), so it does not refute this failure. Confidence would collapse only if digest reconciliation were normatively conditional on a clean census; the task and helper instead require every owed tree/digest class.

2. **WRONG — A callback refusal can erase the runner’s documented dominant binary-drift failure. Blocker. Confidence 99/100.**

   The runner deliberately evaluates binary drift and the post-exit callback, then makes binary drift dominate ([custody_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_git.rs:1031)). But [`proof_child`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8139) replaces every runner error with `callback_error` whenever one was recorded.

   A final-child wrapper can run real Git, mutate its own route binary using the already tested `$0` mechanism, and swap `../.restore-work/git-bootstrap/objects`. The callback records `IdentityChanged`; the runner returns `BinaryDrift`; `callback_error.unwrap_or(error.into())` discards that runner result. The actual result contains only the identity refusal, rather than dominant `BinaryDrift` preserved beside the callback custody failure. Both operations are already independently reachable through existing routed-child tests; combining them needs no new production seam.

   The bounded repair is to substitute the callback error only for the runner’s synthetic callback `RouteRefusal`. An independently returned `BinaryDrift` must remain primary and be composed with the callback error. Add a drift-plus-chain-swap regression requiring both typed causes. Confidence would collapse if the runner never returned drift when the callback also refused, but its explicit `drift?; caller_check?` ordering proves that it does.

3. **WRONG — “Refused by a gate” is incorrectly treated as “the whole gate completed,” hiding a later owed class. Blocker. Confidence 97/100.**

   [`gates_owed`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8117) becomes false for every non-timeout callback error. Yet `proof_deadline_gate` is fail-fast: a mount-census refusal occurs before any retained-chain check.

   In a mount-capable namespace, a routed child can leave a user mount beneath the destination and swap a retained bootstrap chain. The post-exit callback stops at `MountBoundary`; `refused_by_a_gate` suppresses failure-only gate reconciliation; the bootstrap-chain `IdentityChanged` is never observed. The required result preserves the mount failure and the still-owed chain failure. This is low-likelihood on an ordinary host account but directly reachable for a single operator under FUSE, a user namespace, or a container with mount capability—the exact environment class the mount census protects.

   Repair requires tracking which gate stages completed, rather than one refusal boolean, and running the remaining owed classes once. The existing mount seam plus a simultaneous retained-chain swap supplies a realistic discriminating regression. Confidence would fall if mount refusal occurred after chain checking; source order proves the opposite.

4. **SMELL — Round-2 production-before-RED remains historical evidence debt. Nonblocker. Confidence 100/100.**

   The handoff still records that round-2 production and tests were written together and RED was reconstructed afterward ([handoff](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:70)). Round-4 tests-first evidence does not retroactively establish that checkpoint. It demonstrates no runtime error, cannot now be repaired, and remains a nonblocking process SMELL.

The source3 WRONGs are mechanically closed: a post-exit deadline refusal leaves `gates_owed=true` and enters the single failure-only helper, while a timeout first discovered during successful settlement enters that helper at [lines 8184–8185](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8184). Success-eligible work remains on the original deadline, and destination-root identity remains last without an impermissible later clock check.

The supplied exact-HEAD receipts are complete and source-bound. Fmt, workspace clippy, locked build, hygiene, all-targets (**5,192 passed, 0 failed, 13 ignored**), and default tests/doctests (**5,203 passed, 0 failed, 13 ignored**) passed. Focused evidence reports **330/0/0**, preserving 327 earlier names plus three new ones. The eight targeted variants and four controls compiled and matched their declared predictions, but they do not combine census failure with an independently changed file, binary drift with callback refusal, or mount refusal with a later chain failure. I did not run these commands.

The B2c wrapper otherwise preserves the original capsule, ledger, pins, deadline, pre-repository Empty refusal, direct-bootstrap Empty support, unborn-HEAD behavior, retained materialization, final inventory/reachability/fsck, and closing root-last order. Native macOS/ext4, aggregate and whole-G review, 2B3c, and 2B3d remain deliberately deferred.

CAP CLASSIFICATION: OPEN CLASS — at the declared 4/4 cap, three newly exposed combined-failure members remain in the same all-outcome reconciliation family; the cap is exhausted and must not be silently extended.
VERDICT: REJECT
SUMMARY: wrong=3 smell=1 blockers=3 round=4/4
