# Handoff - R2f1b slice 5B V3 detached storage snapshot-digest repair candidate

**Written:** 2026-09-07
**Workspace:** `/Users/wesleyjinks/code/.a2a-implement/impl-14595-hlw4mgvd`
**Pre-repair candidate:** `6193f63022ac10c8057f398deba3c9abb0c2e7ce`, tree `9d59475c49f6af3bc654d59947dfafdf312825bb`
**Prior rejected candidate:** `abb881b90850ec01ca3ba8dd274002ac23e227cd`, tree `7b924e1ecb109bc527cd050eedceee4484262ca5`
**Current repaired code candidate:** `6338bc1628fed52b772026b604a1bbdd710efa8c`, tree `44616d3e8f05a2ec4607911048128221f7914221`
**Exact base:** `43b65d42a83b02ab4041a6680ea5c85fc6d00d82` (PR #101 / Slice 5A), tree `21103adb5a4916043d8802d01ac63408a41abbd8`

## Scope

This repair keeps Slice 5B storage-only and production-unwired. No served coordinator, A2A, MCP, batch, offline CLI, implement, boot/resume, provider, session, process, backend, worktree, live smoke, compatibility, registry/image, operator mutation, publication, push, PR, merge, release, or deployment path was wired or exercised. Real `WorkflowAdmissionRequestV1` construction sites still pass `r2f1b: None`; the fresh V3 reservation/admission/CAS APIs remain unreachable from production callers.

The authorized repair continued the existing rejected artifact rather than restarting. One Tier-3 provider repair turn used execution `exec-6764d22886e376bcd58217c03784a8ed`, attempt `attempt-000e794a1e0f62aaba789b1b1f3cedf0`, immutable image `sha256:f20be11bdc3cef088fe5c69ea13428d149d282f0babc57fc2d5412255f9db380`, Codex `gpt-5.5`/`high`, and no network. Its Cargo probes failed for unavailable registry/cache inputs and are inadmissible; the controller established the behavioral RED and all GREEN evidence below in one same-host environment.

## Repair contents and final disposition

1. `AttemptReservationV3` now requires `workflow_snapshot_digest: Sha256HexV1`, minted from the same canonical `WorkflowSnapshotV3` returned by `freeze_fresh_v3`.
2. Atomic V3 admission calls the authoritative `WorkflowSnapshotV3::decode`, then requires its canonical digest to equal the reservation-bound digest before any store mutation, in addition to the prior exact attempt, delivery attempt, contract, graph, controls, workload, locator, reservation, roster, and node-order checks.
3. Memory and SQLite terminal CAS retain their exact locator, reservation, marker, typed snapshot, roster, and persisted-node checks before first apply or replay classification.
4. The new test constructs two genuine same-attempt admissions that differ only in a node prompt, proves their canonical bytes and digests differ while all old reservation-visible fields remain equal, admits A/A, and rejects B/A without partial state in both stores.
5. The earlier exact-Arc admission proof, terminal replay controls, stale-writer guard, and global resource-flight uniqueness mechanisms remain intact.
6. `graph` and `run_spec` remain mechanically located in `bridge-core` and re-exported from `bridge-workflow` to avoid a production dependency cycle.

The prior review's two deferred SMELLs remain: staged/concurrent resource-flight uniqueness coverage and upgraded pre-5B database lifecycle coverage. The final reviewer found no constructible incorrect result for either and kept both nonblocking.

Final hard-read-only review of exact docs-inclusive head `be1ff0221a247157df949f445755da462c923f1e`, tree `7d8a67564f0c7a9a2b50e5844f64209134b4973e`, used execution `exec-5a8067bfc258b48ae4925a50c38fc2f7`, attempt `attempt-b6134fa8d183d8b4839397903d966d27`, raw Codex `gpt-5.6-sol`/`xhigh`, and returned **VERDICT: REJECT / WRONG_COUNT: 1 / SMELL_COUNT: 2**. Result artifact SHA-256 is `33a44b70163c4af1c781d42de540022090bd8c84752fe0bb85332521079b61ce`.

The reviewer established one new blocker: `AttemptReservationV3` binds attempt, workflow, controls, workload fingerprint, roster, and minted resource-flight IDs, but not the canonical snapshot digest. Prompt content is absent from the workload fingerprint. Two genuine fresh admissions can therefore share the same attempt and reservation-visible fields while differing only in a node prompt. Pairing reservation A with snapshot B passes the authoritative decoder and current field comparisons, so Memory and SQLite persist a snapshot under custody minted for a different admission. The bounded repair is to add the canonical snapshot digest to `AttemptReservationV3`, mint it with the reservation, require an exact match before atomic admission, and add a two-genuine-same-attempt prompt-substitution RED for both stores.

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

Snapshot-digest behavioral RED copied only the new regression onto exact pre-change docs head `55f59950128cf9edfadb28575719d9787f3f5957`: **0 passed / 1 failed / 271 filtered** because genuine admission B's snapshot was accepted with genuine reservation A in Memory. Retained log SHA-256 is `906000da170d37c2790d85e6bfd97f10365e5e1f8ef9f54be1f6504ae66027b5`. An initial candidate invocation reused the base target without recompilation and is explicitly inadmissible.

Focused GREEN on exact code candidate `6338bc1628fed52b772026b604a1bbdd710efa8c`: the same Memory-and-SQLite selector is **1 passed / 0 failed / 271 filtered**, retained log SHA-256 `f8f2f48326fb24f641d075bdf9e95e720605c6a2e37822aa33126fe595e29068`. One first candidate run correctly exposed a test-helper assumption about absent SQLite tasks; the controller made a test-only correction and reran the selector. Production code was unchanged by that correction.

Provider result artifact SHA-256 is `49f1a6ddb192e22ae47d5d653363f020a1f7bbe7da08be342b9aadd9a54a9675`.

Full verification from exact changed HEAD `6338bc1628fed52b772026b604a1bbdd710efa8c`:

- `cargo check --workspace --all-targets --all-features`: passed;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed;
- `cargo test --workspace --all-targets --no-fail-fast`: **4,408 passed / 0 failed / 13 ignored / 726 filtered** across 86 result records, log SHA-256 `514a30ed1940a17774c72c7ccf925b075b0fdebc64b531012fcd78464d2ac01b`;
- `cargo test --workspace --doc --no-fail-fast`: **2 passed / 0 failed** across 16 crate records, log SHA-256 `bad36c95fd867b078b098fc9a2a4daca9982ca5c7819e1ddce619636d04816a2`;
- `cargo build --workspace --all-targets --all-features`: passed;
- `cargo build --release --bin a2a-bridge`: passed;
- `cargo run -p a2a-bridge -- validate --repo-hygiene`: passed (`tracked_artifacts: 41`, `validated_example_configs: 9`);
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Production Rust accounting against `43b65d42a83b02ab4041a6680ea5c85fc6d00d82` is **1,038 logical lines**: the prior 1,035 plus three snapshot-digest binding LLOC. The mechanically moved 1,094 lines and two re-export lines are excluded, as authorized, along with tests, comments, docstrings, blank lines, and delimiter-only structure. The candidate remains below the 1,050 production-LLOC cap.

## Final rereview gate

The authorized bounded continuation repair turn is exhausted. Exactly one final Sol/xhigh hard-read-only rereview remains; its result must be recorded here without silently extending either cap. Until that adjudication, code commit `6338bc1628fed52b772026b604a1bbdd710efa8c` is a local candidate only. No push, PR, merge, release, deployment, live smoke, compatibility case, registry/image mutation, or running-operator change is authorized or claimed.
