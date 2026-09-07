# Handoff - R2f1b slice 5B V3 detached storage repair candidate

**Written:** 2026-09-07
**Workspace:** `/Users/wesleyjinks/code/.a2a-implement/impl-14595-hlw4mgvd`
**Pre-repair candidate:** `6193f63022ac10c8057f398deba3c9abb0c2e7ce`, tree `9d59475c49f6af3bc654d59947dfafdf312825bb`
**Prior rejected candidate:** `abb881b90850ec01ca3ba8dd274002ac23e227cd`, tree `7b924e1ecb109bc527cd050eedceee4484262ca5`
**Current repaired code candidate:** `79eccc470793427735034eb88dffd1feeba6eef4`, tree `a57aeab356a234dfa7257ffa886945033ced58db`
**Exact base:** `43b65d42a83b02ab4041a6680ea5c85fc6d00d82` (PR #101 / Slice 5A), tree `21103adb5a4916043d8802d01ac63408a41abbd8`

## Scope

This repair keeps Slice 5B storage-only and production-unwired. No served coordinator, A2A, MCP, batch, offline CLI, implement, boot/resume, provider, session, process, backend, worktree, live smoke, compatibility, registry/image, operator mutation, publication, push, PR, merge, release, or deployment path was wired or exercised. Real `WorkflowAdmissionRequestV1` construction sites still pass `r2f1b: None`; the fresh V3 reservation/admission/CAS APIs remain unreachable from production callers.

The authorized repair continued the existing rejected artifact rather than restarting. One Tier-3 provider repair turn used execution `exec-76ec188ce092da3e964eef5fa70a2e77`, attempt `attempt-2bff0bb4bcfe73bb750d215f184e5b67`, immutable image `sha256:f20be11bdc3cef088fe5c69ea13428d149d282f0babc57fc2d5412255f9db380`, Codex `gpt-5.5`/`high`, and no network. Its Cargo probes failed for unavailable registry/cache inputs and are inadmissible; the controller established the behavioral RED and all GREEN evidence below in one same-host environment.

## Repair contents and pending disposition

1. Atomic V3 admission now calls the authoritative `WorkflowSnapshotV3::decode`, then binds its exact attempt, delivery attempt, contract activation and fingerprint, graph workflow and canonical bytes, controls, workload identity, locator, reservation, node roster, and node order before any store mutation.
2. Memory and SQLite terminal CAS now validate the exact V3 locator, reservation, atomic-admission marker, typed snapshot, roster, and persisted node rows before either first apply or replay classification. Legacy V1/V2 rows and staged-only V3 rows therefore cannot return `Replayed` without complete V3 admission.
3. The tests construct genuine V3 bytes through `WorkflowAdmissionV1::freeze_fresh_v3`, cover malformed/noncanonical/unknown-field/different-genuine snapshot substitution, and cover V1/V2 pending and ready plus staged-only V3 pending and ready replay in both stores. Exact admitted V3 apply/replay/reopen remains idempotent.
4. The earlier exact-Arc admission proof, post-readiness immutable replay evidence, stale-writer guard, and global resource-flight uniqueness mechanisms remain intact.
5. `graph` and `run_spec` moved mechanically from `bridge-workflow` to `bridge-core` to let the store use the single authoritative decoder without a production dependency cycle. `bridge-workflow` re-exports both modules; the moved bodies were audited as behaviorally identical apart from crate-path/import normalization.

The prior review's two deferred SMELLs remain outside this repair: staged/concurrent resource-flight uniqueness coverage and upgraded pre-5B database lifecycle coverage. They were not blockers and did not authorize scope expansion.

## Evidence

Same-host behavioral RED applied only the new tests to exact prior rejected candidate `abb881b90850ec01ca3ba8dd274002ac23e227cd`:

- authoritative V3 decode: **0 passed / 1 failed / 270 filtered** because unknown-field snapshot bytes were admitted; retained log SHA-256 `2216227f5f86755701aa9a50f6ab5b71acf0674bfca218f47e1ca899c03c200b`;
- legacy/staged terminal replay: **0 passed / 1 failed / 270 filtered** because memory V1 pending state returned `Replayed { seq: 2 }` instead of `Conflict`; retained log SHA-256 `6b7c6e170cd175c72d38c8d9977dd91f6463bbaf4ce92ea3b963fa6ddfecdf3f`.

An initial controller invocation used an unqualified `--exact` selector and selected zero tests. It is explicitly inadmissible and was corrected before any belief update. The worker's registry/cache failures and a sandbox-denied candidate invocation are likewise inadmissible.

Focused GREEN on exact code candidate `79eccc470793427735034eb88dffd1feeba6eef4` in the same host environment:

- authoritative/malformed atomic V3 admission: **1 passed / 0 failed / 270 filtered**, log SHA-256 `c5f95cd318fb4e12b8b6fb7a92287756449ad3cd0cf60a3849b86ae655ae0c52`;
- legacy/staged terminal replay matrix: **1 passed / 0 failed / 270 filtered**, log SHA-256 `0bdd03e706e04751bdd93a6e7f29e31f58311c54f66fa96956504f7767c864e8`;
- SQLite reopen replay: **1 passed / 0 failed / 270 filtered**, log SHA-256 `ab14d378aac60f1f121bdf279365f850f43c1a7c96b6c433a27d84a133cc32d8`;
- genuine builder and CAS positive path: **1 passed / 0 failed / 270 filtered**, log SHA-256 `4583ef478d79ee6f343a60d34323afb1ddc7f523cdc4f82930538138a1946e09`.

Full verification from exact changed HEAD `79eccc470793427735034eb88dffd1feeba6eef4`:

- `cargo check --workspace --all-targets --all-features`: passed;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed;
- `cargo test --workspace --all-targets --no-fail-fast`: **4,407 passed / 0 failed / 13 ignored / 726 filtered** across 86 result records, log SHA-256 `29e7263fbdc76f6f05765b0d46a2aae9d0946ef1cb4e8ac6d775b8795eb1dd7b`;
- `cargo test --workspace --doc --no-fail-fast`: **2 passed / 0 failed** across 16 crate records, log SHA-256 `99f1bb8fddabed40ddcbea2c367fda0a92aa06a41113f60ceb1d7ac301ac9bbf`;
- `cargo build --workspace --all-targets --all-features`: passed;
- `cargo build --release --bin a2a-bridge`: passed;
- `cargo run -p a2a-bridge -- validate --repo-hygiene`: passed (`tracked_artifacts: 41`, `validated_example_configs: 9`);
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Production Rust accounting against `43b65d42a83b02ab4041a6680ea5c85fc6d00d82` is **1,035 logical lines**: the reviewed artifact's 922 plus 113 behavioral repair LLOC. The mechanically moved 1,094 lines and two re-export lines are excluded, as authorized, along with tests, comments, docstrings, blank lines, and delimiter-only structure. The candidate remains below the 1,050 production-LLOC cap.

## Convergence gate

The one authorized repair turn is spent. One final hard-read-only Sol/xhigh rereview remains and must inspect the exact docs-inclusive candidate diff before any acceptance claim. Until that rereview returns, this artifact is neither accepted nor publishable. No further repair, push, PR, merge, release, deployment, live smoke, compatibility case, or running-operator mutation is authorized or claimed.
