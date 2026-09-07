# Handoff - R2f1b slice 5B V3 detached storage repair stop

**Written:** 2026-09-07
**Workspace:** `/Users/wesleyjinks/code/.a2a-implement/impl-14595-hlw4mgvd`
**Pre-repair candidate:** `6193f63022ac10c8057f398deba3c9abb0c2e7ce`, tree `9d59475c49f6af3bc654d59947dfafdf312825bb`
**Reviewed repaired candidate:** `abb881b90850ec01ca3ba8dd274002ac23e227cd`, tree `7b924e1ecb109bc527cd050eedceee4484262ca5`
**Exact base:** `43b65d42a83b02ab4041a6680ea5c85fc6d00d82` (PR #101 / Slice 5A), tree `21103adb5a4916043d8802d01ac63408a41abbd8`
**Required commit subject:** `feat: reserve R2f1b V3 detached storage`

## Scope

This repair keeps Slice 5B storage-only and production-unwired. No served coordinator, A2A, MCP, batch, offline CLI, implement, boot/resume, provider, session, process, backend, worktree, live smoke, compatibility, registry/image, operator mutation, publication, push, PR, merge, release, or deployment path was wired or exercised. Real `WorkflowAdmissionRequestV1` construction sites still pass `r2f1b: None`; the fresh V3 reservation/admission/CAS APIs remain unreachable from production callers.

The checkout was verified clean at pre-repair HEAD `6193f63022ac10c8057f398deba3c9abb0c2e7ce` and tree `9d59475c49f6af3bc654d59947dfafdf312825bb` before edits.

## Repair contents and final disposition

1. Fresh automatic admission custody now has a public narrow verifier in `bridge-workflow::admission` that consumes the builder-owned private proof identity and requires the exact admitted run-spec `Arc` and exact admitted R2f1b contract `Arc`. `detached_v3_attempt_reservation` calls it before deriving the reservation. The final review found this mechanism closed.
2. The candidate adds JSON-shape validation and selected-field binding before atomic task mutation, but the final review proved it is not the authoritative typed `WorkflowSnapshotV3` decoder: genuine builder-encoded bytes can be refused, while fabricated selectively valid bytes can pass.
3. The candidate persists a complete atomic-admission marker in Memory and SQLite, and the initial apply path requires it. The final review proved both replay branches run before that requirement, so matching V1/V2 or staged legacy-terminal states can incorrectly return `Replayed` without V3 admission.
4. Terminal projection readiness no longer erases immutable replay evidence. Memory retains a replay copy after `mark_terminal_projection_ready`; SQLite leaves `terminal_projection_attempt_id`, `terminal_projection_json`, terminal sequence, and workflow outcome intact. Apply -> mark-ready -> exact replay returns the original sequence and writes no second journal row, including after SQLite reopen; changed payloads still conflict without mutation.
5. Resource-flight identity uniqueness is now global across existing task-side V3 reservations in Memory and SQLite. Both atomic admission and staged row reservation refuse a second task attempting to reuse an existing V3 resource flight before partial mutation.

The stale-writer regression remains explicitly guarded by the exact current-attempt predicate in `compare_set_detached_terminal_v3` and by the parity test `memory_and_sqlite_v3_stale_terminal_writer_is_rejected_without_mutation`, which asserts the successor locator remains current and no task, pending projection, journal, or node-evidence mutation occurs after a stale attempt write.

Final hard-read-only review execution `exec-d1698e074e060fcb38ab30687e9f8d27`, attempt `attempt-1a671927756a5f036c183c54a3552119`, used raw `gpt-5.6-sol`/`xhigh` and returned **VERDICT: REJECT / WRONG_COUNT: 2 / SMELL_COUNT: 2**. The two WRONG blockers are the non-authoritative snapshot decoder and replay-before-marker classification above. The two deferred SMELLs are missing staged/concurrent resource-flight uniqueness coverage and missing upgraded pre-5B database lifecycle coverage.

## Evidence

The same-environment base RED used exact production bytes from `6193f63022ac10c8057f398deba3c9abb0c2e7ce`, the test-only patch `/private/tmp/r2f1b-5b-red-tests-6193f630.patch`, immutable Tier-3 image `sha256:f20be11bdc3cef088fe5c69ea13428d149d282f0babc57fc2d5412255f9db380`, no network, and the same prewarmed Cargo volume later used for GREEN. The probes failed on the intended old behavior:

- exact fresh-authority builder/constructor test: **0 passed / 1 failed / 282 filtered**; an equal-valued rewrapped public authority was accepted;
- atomic V3 admission group: invalid or unbound snapshot bytes were accepted, and staged V3 rows without a complete admission marker returned `Applied` instead of `Conflict`;
- global resource-flight uniqueness: **0 passed / 1 failed / 270 filtered**; a second task reused the same flight;
- post-publication exact replay: **0 passed / 1 failed / 270 filtered**; exact replay returned `Conflict` instead of `Replayed { seq: 2 }`.

The worker's earlier crates.io 403 and empty-cache attempts selected zero tests and are inadmissible; they are not RED or GREEN evidence.

Changed-tree verification in the same immutable offline image:

- focused bridge-store V3 tests: **11 passed / 0 failed / 260 filtered**;
- exact coordinator fresh-authority test: **1 passed / 0 failed / 282 filtered**;
- workflow direct-construction refusal test: **1 passed / 0 failed / 14 filtered**;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed;
- `cargo test --workspace --all-targets --no-fail-fast`: **4,407 passed / 0 failed / 13 ignored / 714 filtered** across 86 result records;
- `cargo test --workspace --doc --no-fail-fast`: **2 passed / 0 failed** across 16 crate records;
- `cargo build --workspace --all-targets --all-features`: passed;
- `cargo build --release --bin a2a-bridge`: passed;
- `cargo run -p a2a-bridge -- validate --repo-hygiene`: passed (`tracked_artifacts: 41`, `validated_example_configs: 9`);
- `cargo fmt --all -- --check` and `git diff --check`: passed. Exact base `43b65d42` was formatter-clean in the same image; the inherited `6193f630` formatting drift was repaired rather than waived.

Production Rust line count against `43b65d42a83b02ab4041a6680ea5c85fc6d00d82`: **922 added production logical lines**. Method: `git diff --unified=0 43b65d42 --` over the five allowed production Rust files, exclude tests and additions after their test-module boundaries, then exclude blank lines, comments/docstrings, `#[cfg(test)]`, and delimiter-only structural lines. Documentation and test additions are excluded.

## Convergence stop

The authorized one repair turn and one final rereview are exhausted. The reviewed code commit `abb881b90850ec01ca3ba8dd274002ac23e227cd` remains rejected and must not be published or merged. No further repair, review, push, PR, merge, release, deployment, live smoke, compatibility case, or running-operator mutation is authorized or claimed. A new owner-authorized bounded repair must address both WRONG findings on this existing artifact; it must not restart from scratch.
