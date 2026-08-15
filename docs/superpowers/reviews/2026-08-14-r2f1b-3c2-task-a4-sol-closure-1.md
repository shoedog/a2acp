I’ll perform the single authorized review round, strictly read-only: freeze the commit/diff identity, read both changed files and the handoff in full, then trace production callers, persistence, projections, and regressions with repository searches. Prism/LSP skills live outside this repository, so the repo-only boundary prevents loading their required instructions; I’ll use read-only Git and `rg` without probing those tools.The checkout matches the declared clean head and exact three-commit line; `Cargo.lock` has no diff, and the cumulative diff is clean. I’m now reading the complete changed artifacts and will then inventory every journal mutator and external caller, with special attention to debt ordering, admission capacity, reserved names, recovery clearing, and the A3 commitment rider.The owned journal implementation is fully in view. Its mutator ordering does put recorded debt ahead of reserved-name and fallible checks, and the 4,096 census logic reserves stage/append/replace/retire footprints. I’m moving through the transaction engine and its recovery table now, then I’ll cross-check deleted symbols and all non-test call sites so the conclusions do not rest on the colocated tests alone.The transaction engine closes the previously reported debt-ordering bug, and the A3 recovery commitment recheck is present on both live and recovery completion paths. One blocking mismatch remains: the namespace transaction methods implement the disclosed reserved-object repin, but the three direct journal mutators still return ordinary `Refused` before consulting the on-disk reserved census; their existing test explicitly pins that contrary result. I’m doing the final privacy/caller/status checks and then will report the bounded verdict.## WRONG findings

1. **WRONG — BLOCKER: direct journal mutators bypass the reserved-object repin.**

   Constructible state: open a fresh `JournalRootCustodyV2` over a root containing `.a2a-v2-x`, begin an operation, and call `stage`, `publish`, or `append` with that same reserved target. The fresh handle starts with `protective_debt = 0`.

   Incorrect result: all three return ordinary `Refused("reserved target")`, leaving the debt flag clear, instead of reporting `ProtectiveDebt` for the on-disk residue.

   Mechanism: in [fs_custody.rs:1212](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/fs_custody.rs:1212), `stage`, `publish_with`, and `append` call `refuse_debt`, then `refuse_reserved_target`, and only afterward call `guard`, where the reserved census occurs. The existing test at [fs_custody.rs:5312](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/fs_custody.rs:5312) explicitly pins this contrary `Refused` result. The namespace transaction counterparts correctly run admission first and distinguish object-present protective debt from clean-root `NoEffect` at [namespace_transaction.rs:446](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/namespace_transaction.rs:446).

   Trigger likelihood: **rare** before B–G because there is no current production caller or V3 arming; realistically reachable once a request-journal caller feeds a crash/corruption residue name back into the public journal surface.

   Exposure and impact: future protected request attempts or any direct bridge-core consumer can misread a residue-bearing root as an ordinary caller refusal and omit mandatory recovery. This call does not itself mutate the namespace, but its typed authority result is wrong.

   Bounded fix: perform a residue/capacity admission before returning the name-level refusal; for `publish`, do not whitelist the derived staging name when the target itself is reserved. Rough cost is localized, approximately 15–30 production lines plus tests in `fs_custody.rs`.

   Red regression: change the existing object-present test to require `ProtectiveDebt` for all three mutators, add clean-root cases requiring `Refused`, and include a publish case where only the derived reserved staging object exists. These tests fail on `7a973866`.

   **BLOCKER:** this directly contradicts the disclosed reserved-object-present repin and leaves one required protective projection undelivered.

## SMELL findings

1. **SMELL — DEFER: initial owned-surface red evidence was compile-only.**

   The handoff’s initial A4 receipt says the test failed because the six methods did not exist, which is not behavioral fail-first evidence under this review contract. Head tests cover happy, refusal, capacity, and failure paths, but the historical red does not demonstrate their behavioral discrimination.

   Trigger likelihood is **plausible** during later B–G refactoring; future protected runs are exposed, with potentially high impact if a test remains green after weakening durability or identity logic. Perform bounded mutation checks for stage, publish, append rollback, read identity, enumeration bounds, and sync, retaining nonzero behavioral receipts. Test-only blast radius; **DEFER** because no current wrong behavior follows from the evidence gap.

2. **SMELL — DEFER: replace capacity lacks its exact positive boundary.**

   The capacity test at [namespace_transaction.rs:1112](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/namespace_transaction.rs:1112) checks replace refusal at 4,095 and 4,096 entries but not successful admission at 4,094, the highest safe count for footprint two. The implementation’s `len + 2 > 4096` calculation is correct, so no current incorrect result is established.

   Trigger likelihood is **rare** now and **plausible** after high-volume B–G activation; impact would be availability through needless refusal, not over-cap mutation. Add the 4,094 `Complete` case and mutation-check it against footprint three or an inclusive boundary. Tiny test-only cost; **DEFER**.

## Adjudications and evidence

- Debt-flag mechanics: **FIXED** for outcomes actually classified as protective. Engine-level `record` covers hook-driven transaction paths, and clean empty-census recovery clears only after root sync and fresh route proof.
- Capacity headroom: **FIXED**, subject to the deferred 4,094 positive-boundary test. Stage/append reserve one, publish/sync zero, replace two, and retire one; over-cap enumeration is protective.
- Reserved-target self-poisoning: **FIXED** for mutation safety—reserved targets do not reach namespace mutation. The separate object-present result projection is **OPEN** as WRONG 1.
- Debt domination: **FIXED** once debt is recorded; all six mutators check it before fallible work, while recovery remains callable.
- Route check versus syscall: **ACCEPTED-RESIDUAL** under the owner-supplied cooperating-participant lease threat model. Descriptor-relative operations retain the root, and target races classify protectively; an uncooperative exact-child swap remains theoretical-only and outside that ruling.
- Debt durability adjudication: **PARTIAL**. Residue-backed cross-handle/restart behavior and residue-free live-handle behavior are coherent, and the planned `open_recovered` boundary can self-heal residue-free uncertainty. WRONG 1 is the exception: present disk residue is not consulted before three direct refusals.
- Reserved-object-present repin: **OPEN**.
- Cap/size adjudication: **ACCEPTED**. The exact diff touches only the two owned Rust files and handoff; no silent scope appeared.

Deletion and integration evidence is otherwise satisfactory: candidate V1 symbols have zero Rust references; the only matches are historical handoff text. New Task A types have no callers outside `fs_custody.rs` and `namespace_transaction.rs`. The operation-lock file remains private; the root `File` projection is crate-private and used only by the transaction engine. Production still assigns `resource_flight_route_v3 = None`; the sole `Some(...)` construction is inside `backend.rs`’s test module. `Cargo.lock` is unchanged, persistence wire fields are unchanged, the A3 recovery commitment recheck is present and behaviorally discriminating, and no new `rustfmt::skip` was introduced.

The supplied green gates are corroborative but cannot close WRONG 1 because the current green test asserts the incorrect ordinary-refusal contract. I did not build or run tests under the review boundary; the worktree remained clean, and read-only `git diff --check` passed.

Confidence: **96/100**. A red-before/green-after regression covering all three direct mutators would raise confidence after repair. An authoritative restriction of the repin solely to `NamespaceTransactionV2` would lower it. The conclusion would collapse only with mechanism-level proof that a fresh custody handle cannot have `protective_debt == 0` while a reserved entry exists, which the current initialization and ordering contradict.

VERDICT: REJECT
SUMMARY: One BLOCKER remains: direct journal stage, publish, and append misclassify existing reserved residue as ordinary Refused before the protective census; two coverage smells are deferred.