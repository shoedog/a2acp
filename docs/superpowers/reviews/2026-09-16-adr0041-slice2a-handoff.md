# Handoff — ADR-0041 Slice 2A local seal contracts

**Written:** 2026-09-16; merge reconciled 2026-09-20 · **By:** Codex `/root` · **Workspace:**
`/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/rustls-pr104-20260916`

**Integrated predecessor:** PR #104 head `015940832ea05eb9b8eba627b5460f70d13cc8a4`

**Branch:** `feat/adr0041-custody-inventory-slices-1a-1b-20260916` · **Content commit:** `64545d45`

## 0. Gating facts

- **PR #104:** merged on 2026-09-20 at `27a885f6`; Slice 2A content commit `64545d45` and documentation reconciliation
  `65a572df` are its second-parent lineage. The merge tree is identical to reviewed head `65a572df`. Owner-authorized lock
  repair `1ba72900` updates `rustls` to 0.23.45 and `rustls-webpki` to 0.103.15; local `cargo deny check`, the
  repaired branch's full/static gates, and replacement GitHub Build/Lint/Coverage, uninstrumented, macOS, Windows,
  and CLA checks were green at dependency-repair head `01594083`. Slice 2A was integrated onto exact predecessor
  `01594083`; merge is now exercised. Running-operator adoption remains separate and unexercised.
- **Implementation:** local, provider-free, and effect-free. No filesystem/Git capture, encryption, restore,
  promotion, provider, authorization, quarantine, reap, deletion, merge, or running-operator path exists.
- **Independent review:** round one Sol/high hard-read-only review completed as `REJECT`, 2 WRONG / 3 SMELL. Both
  WRONG mechanisms reproduced under focused behavioral RED and were repaired. Final round two returned `APPROVE`,
  0 WRONG / 2 SMELL. At the exhausted cap the loop was converging; both remaining nonblocking SMELLs received
  bounded test/docs repairs without a third review or production-code change.
- **Publication:** owner-authorized and exercised. Reviewed checkpoint `9b0f1c7c` was transplanted onto `01594083`
  with docs-only conflict reconciliation as content commit `64545d45`, then pushed to PR #104.
- **Review cap:** two admitted rounds. Closed enumerable WRONG findings are repaired on this artifact; an open-class
  finding parks the slice for design rather than restarting it.

## 1. Resume order

1. Bind cwd, branch, HEAD, integrated predecessor, and content commit above; require a clean worktree.
2. Read the task, ADR-0041 §§4/9/14, and this handoff. Re-run focused tests if any byte drifted.
3. Read both review artifacts recorded in §2. The cap is exhausted; do not replay either attempt or dispatch a
   third review. The two round-two SMELLs are closed by tests/docs only and the final direct gates below.
4. Preserve the exact seven-path Slice 2A population and re-run focused verification if any bound code/test/task
   digest changes.
5. Treat PR #104 merge commit `27a885f6` as the completed predecessor. Use the Slice 2B planning handoff for current
   work. Do not start Slice 2B implementation/effects, clean worktrees, or mutate the running operator without the
   next applicable gate.

## 2. State ledger

| Item | State | Evidence |
|---|---|---|
| Task | done | `docs/superpowers/plans/2026-09-16-adr0041-slice2a-local-seal-contracts-task.md` |
| Structural RED | done | Exact focused command selected the target and failed on `E0432` because `bridge_core::custody_seal` did not exist. |
| Local spec/code audit | done | Seven closed WRONG items repaired: dependency state, exclusion cross-links, symbolic/direct/unborn ref targets, conflicting object kinds, valid zero-byte artifacts, direct-ref object closure, and impossible NUL-bearing artifact names. One artifact-mapping SMELL is deferred to Slice 2B because 2A has no exporter/consumer or authority. |
| Behavioral RED 1 | done | Removing coverage completeness accepted a manifest missing `Worktree`; selected test failed, then passed restored. |
| Behavioral RED 2 | done | Ignoring unresolved coverage dropped `PrivacyHold` from sealability; selected test failed, then passed restored. |
| Behavioral RED 3 | done | Normalizing artifact lengths in seal hashing made lengths 7 and 8 collide; selected test failed, then passed restored. |
| Behavioral RED 4 | done | Removing prefix rejection accepted `pack` plus `pack/data`; selected test failed, then passed restored. |
| Review round one | done | Sol/high attempt `attempt-b5245a35e1d3fba39f2f2532692a6a66`, execution `exec-6074b045f42cee1b70e181b196a07ec5`: REJECT, 2 WRONG / 3 SMELL. Artifact 10,654 bytes, SHA-256 `b38c952f9465a06b994e52fc34674cd5565c969488e3aebf126a6a80903f5313`. |
| Round-one behavioral RED | done | Direct Serde accepted an empty-coverage manifest and reported it sealable; focused test failed 0/1. `pack`, `pack!`, `pack/data` bypassed adjacent-only prefix detection; focused test failed 0/1. Both tests pass restored after validating wire adapters and complete ancestor lookup. |
| Round-one SMELL repair | done | Constructor/error coverage now includes every named branch, all public record deserializers reject invalid specimens, every seal field has digest-sensitivity coverage, non-UTF-8 names round-trip, artifact names use an explicit portable slash namespace, and the task names the actual ADR path. |
| Review round two | done | Sol/high attempt `attempt-d48c84500d1584bcd5c1000179e7e66e`, execution `exec-5791f985be1c844bfb0beacfee3e1300`: APPROVE, 0 WRONG / 2 SMELL. Artifact 6,852 bytes, SHA-256 `2c204e8542f11eb5ef06a05d23d20808320b86ad18bacbaa65b1c0908496ce39`. |
| Cap disposition | done | Converging at cap: exact-dedup/reconstruction-ID and manifest-field digest tests close the inherited coverage SMELL; the Slice 1B handoff now records exercised rustls authority and PR head `01594083`, closing the fresh docs SMELL. No third round and no production-code change after approval. |
| Focused target | done | 22 passed / 0 failed / 0 ignored. |
| `bridge-core` package | done | 842 passed / 0 failed / 0 ignored including doctests and compile-fail targets. |
| Full workspace | done | Final direct invocation: 4,444 passed / 0 failed / 13 ignored across 88 targets; current harness list contains 4,457 tests. |
| Strict gates | done | Workspace/all-target Clippy with `-D warnings`, format check, diff check, and repository hygiene all pass; hygiene reports 41 tracked artifacts / 9 validated example configs. |
| Independent review | done | Two admitted Sol/high rounds exhausted the cap: REJECT 2 WRONG / 3 SMELL, then APPROVE 0 WRONG / 2 SMELL. The earlier policy refusal was pre-spawn/pre-prompt and is not review evidence. |
| Integration and publication | done | Reviewed checkpoint `9b0f1c7c` was committed from the original seven-path worktree, cherry-picked onto exact PR predecessor `01594083`, resolved only duplicate roadmap/Slice 1B handoff reconciliation blocks, and published as `64545d45`. |
| Post-integration verification | done | Focused 22 / 0; `bridge-core` 842 / 0 / 0; direct full workspace 4,444 / 0 / 13 ignored across 88 targets; warnings-denied Clippy, format, diff, `cargo deny check`, and hygiene 41/9 all pass. |

## 3. Full-suite probe incident

The first direct full-suite run exited 0. A later logging rerun used `cargo test ... 2>&1 | tee ...` without the
recommended pipefail wrapper and failed 31 tests in the first binary. The extra pipeline process/open descriptor is
mechanistically relevant to descriptor, lock, watcher, and local-server tests, while transient resource leakage can
produce the same symptom. A clean direct rerun after confirming no cargo process remained exited 0. This separates
the Slice 2A delta from a deterministic regression but does not distinguish pipeline shape from transient leakage;
retain `/private/tmp/a2a-bridge-adr0041-slice2a-full-test-20260916.log` as the failed probe and use only direct
commands for the completion gate.

## 4. Deferred Slice 2B boundary

Slice 2B should define a provider-free local exporter/fixture format that maps every captured coverage class to
exact sealed artifacts, proves Git object closure without alternates, and restores hidden state with hooks and
executable config disabled. It must obtain a separately reviewed spec before adding filesystem writes, snapshot
assumptions, encryption interfaces, or restore execution. Remote promotion and all destructive authority remain
later ADR stages.

The planning candidate now lives at
`docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`. It decomposes this boundary into serial
2B1 contracts, 2B2 isolated export/object closure, and 2B3 inert restore/hidden-state proof. It is not yet
independently reviewed and grants no implementation or effect authority.

## 5. Owned paths and status

- `crates/bridge-core/src/custody_seal.rs` — new pure manifest/seal records and validation.
- `crates/bridge-core/src/lib.rs` — module export only.
- `crates/bridge-core/tests/custody_seal.rs` — 22 integration tests.
- `docs/superpowers/plans/2026-09-16-adr0041-slice2a-local-seal-contracts-task.md` — bounded task.
- `docs/superpowers/reviews/2026-09-16-adr0041-slice2a-handoff.md` — this handoff.
- `docs/superpowers/reviews/2026-09-15-adr0041-slice1b-handoff.md` — PR #104 CI reconciliation only.
- `docs/reliability-execution-roadmap.md` — current program cursor reconciliation.

Final SHA-256 bindings: production module
`60f2a4644ee7160f234ce461d7189f904d54b7806aeea71fb10b90334f2f2a8d`; integration tests
`cc0511a3a5a3fff537a8c2e1436a18c8b2a8d7fd116e29844efbdd85d97118ec`; task
`469831ee5fe5af6618a339a94a6e57e88fcf404de2cab11945032d5b7d280a37`.

STOP on schema widening outside these records, any source discovery/capture/write path, provider or remote effect,
authority token, restore execution, CLI/store wiring, merge, destructive action, or review-cap overflow.
