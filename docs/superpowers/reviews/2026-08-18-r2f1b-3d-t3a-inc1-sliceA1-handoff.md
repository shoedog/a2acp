## Summary

This revision deliberately splits the former slice A into A1 and A2. A1 adds public reporting vocabulary only: it has no behavioral red and changes no production traversal or decision.
## What changed

- Added the fifteen public report types, raw decision projection, custody-state snapshot, and borrowed snapshot-eligibility view.
- The external integration test imports and compile-asserts all fifteen names through `bridge_worktree::sweep::*`; its never-called signature-check function type-checks every promised public accessor and the public authority-refusal constructor.
- Production policy readiness remains false. The explicit-ready private predicate is test-only mechanism evidence. A2 owns production construction, report return wiring, exact names, characterization, and the concrete mutation audit.
## Evidence

A1 pre-change failure is compiler/API-shape evidence, not decision-behavior evidence. The source now contains the specified external public-API assertion and projection tests. `cargo fmt --all` completed successfully. The focused external integration-test command was blocked by a crates.io 403 while fetching `a2a-lf`; an offline focused report-unit attempt was blocked because `arc-swap` is absent from the local index.
## OPERATOR EVIDENCE — SUPPLIED
- [x] `cargo fmt --all -- --check` — exit 0 (operator, host, macOS)
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings` — exit 0 (operator, host)
- [x] `cargo test --workspace --locked --no-fail-fast` — exit 0; 4,163 passed / 0 failed / 13 ignored across 75 test binaries + 16 doc-test suites (raw sum 4,164 includes one nested filtered-subprocess line)
## Limits and disclosures

`effective()` is snapshot eligibility, not retained action authority; borrowing is ergonomic coupling, not type-enforced inseparability. A returned report owns no live scan authority. Under its own lock, T3b must re-open and identify the root, re-read the exact enumerated record, re-establish placement and source/root/worktree/common-directory binding, apply current admission, repeat exact-absence observation against target and Git registration, refuse any changed observation, and retain action-time authority through the effect. No T3b implementation may remove, prune, settle, transition, or publish solely because a row appeared in `effective()`.

A1 production constructs none of these values. The four temporary unconditional dead-code allowances cover the report, scan-status, entry, and custody-record constructors. Non-Unix lint allowances and Unix-only tests: none. The `sweep.rs` audit adds only the module declaration and re-exports. Final numstat and clean-tree evidence belong in an external receipt keyed to the final SHA, because a committed handoff cannot attest its own commit.
## Sizing

Final `git diff --numstat 9aedf175` evidence is 698 added-plus-deleted lines, below the 700-line A1 cap. Operator totals count test binaries plus doc-test suites without nested-harness double counting. No green Windows all-target baseline is claimed.
