# R2f0a final native verification — 2026-07-25

## Frozen boundary

- Repository: `/Users/wesleyjinks/code/a2a-bridge-r2f-design`
- Branch: `agent/r2f0a-identity-ledger`
- Verified HEAD: `7b01ab4bae167d3640050dfda5de7e1478728497`
- Verified tree: `7d0b14aa1d39ca36fdc68a9ad69df4fc8442e64e`
- Parent: `4359dc9c0b6042cbc78acc2ab77ae57f47c429c2`
- R2f0a correction-stack base: `1cd4d92990d26cbde04f5c800e9fe8c415a35891`
- `origin/main` observed: `345941db91a7d898884bfe79e573433484ccafcc`
- Worktree: clean after all commands.
- `git diff --check 1cd4d929..HEAD`: passed.

Changed paths from the correction-stack base:

- `crates/bridge-a2a-inbound/src/server.rs`
- `crates/bridge-a2a-inbound/tests/workflow_producer.rs`
- `crates/bridge-coordinator/src/coordinator.rs`
- `crates/bridge-coordinator/src/detached.rs`
- `crates/bridge-core/src/ports.rs`
- `crates/bridge-mcp/src/server.rs`
- `crates/bridge-mcp/tests/mcp_client.rs`
- `crates/bridge-store/src/sqlite.rs`

## Integrated reviewed stacks

- `4a6fcb90da6aba26339e9ddbed8e18095b56c03f` — API/handoff, content imported from approved
  `0cb1090386465ab62eae2f5fcfe47b7c694228f2`.
- `f145535a3aa3f02f68c8ccb3dd4fa2b502b0c599` — recovery ordering, content imported from approved
  `7b8fa3765585830324af3a7b5d99108eb8d4a14f`.
- `4359dc9c0b6042cbc78acc2ab77ae57f47c429c2` — deterministic test-hook/coverage stack, folded
  from approved `6d34edcb`, `0b77ed87`, and `04b5792e`.
- `7b01ab4bae167d3640050dfda5de7e1478728497` — served-lineage and Platform authority, folded
  from approved `a1481ed`, `dea817be`, and test-only `24fd4b8a`.

Content comparisons after integration were exact for the final recovery files, API files outside the
superseded coordinator path, and final SQLite file.

## Native macOS gates rerun at final HEAD

All commands ran directly in the repository on macOS.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS, exit 0 |
| `cargo check --workspace --all-targets --all-features --locked` | PASS, exit 0 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | PASS, exit 0 |
| `cargo build --workspace --all-targets --all-features --locked` | PASS, exit 0 |
| `cargo build --release --workspace --all-targets --all-features --locked` | PASS, exit 0 |
| exact macOS alias regression | PASS: 1 passed, 0 failed, 0 ignored, 211 filtered |
| complete workspace suite | PASS: 73 emitted suites, 2,785 passed, 0 failed, 12 ignored, 0 measured, 0 filtered |
| `cargo run -p a2a-bridge --locked -- validate --repo-hygiene` | PASS: 37 tracked artifacts, 7 example configs |
| final `git diff --check` and clean status | PASS |

Exact alias command:

```sh
cargo test -p bridge-store --all-features --locked \
  sqlite::r2f0a_history_tests::public_open_retains_configured_locator_requirement \
  -- --exact --test-threads=1
```

Complete suite command:

```sh
cargo test --workspace --all-features --locked -- --test-threads=1 --quiet
```

The 12 ignored tests are repository-declared live/external-provider or multi-bridge cases; no test was
newly ignored or command-line skipped for this run.

## Fail-first and edge/negative coverage

- API/handoff: direct admission precedes route execution; route runs once; terminal failure takes
  precedence; explicit pre-default routing remains a negative control.
- Recovery ordering: one-shot primary terminal failure replays on the second boot; exact pending row
  survives ambiguous commit; post-commit pending-read failure settles on the next scan; checkpoint
  ordering and hidden-pending controls remain present.
- Test harness: identical global hook arms fail loudly; mutation failpoints are isolated by store;
  mismatched keys do not consume an exact entry; native coverage/umask proof passed before integration.
- Lineage: configured and colocated-Platform false-parent attempts return `Collision`; malformed,
  non-text, and mismatched locators return `Corruption`; configured and colocated-Platform missing
  locators return `Corruption`; the legitimate one-summary gap succeeds; standalone Platform with no
  primary `tasks` row remains fail-open.
- macOS alias regression: the first final native attempt failed because the candidate test compared
  canonical `/private/var/...` to caller-spelled `/var/...`. Before the test-only correction, 47
  suites emitted 2,541 passed / 1 failed / 12 ignored and `bridge-store` reported 211 passed / 1
  failed. The corrected test now compares two canonical paths and the entire native suite passes.

## Explicitly not verified here

- No ignored live/provider test was forced; doing so would be billable or require external services.
- No production served bridge was stopped, rebuilt, or modified.
- The isolated Linux verifier for the final six-line macOS test correction could not fetch a missing
  cached `a2a-lf` dependency because locked egress returned HTTP 403. Native macOS compile/test gates
  above provide the final proof for that test-only change; the failure is disclosed rather than
  represented as Linux success.
- No release, push, PR, or production promotion has occurred.
