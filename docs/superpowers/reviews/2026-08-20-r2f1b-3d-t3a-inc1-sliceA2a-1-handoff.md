# A2a-1 correctness-evidence handoff

## Scope and custody

- Task base: `2e4bba41c9d55b6d517b0379f74585192736ad84`; the accepted production refactor landed in this prior commit.
- Original reference base: `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`; `git rev-list` reports one accepted production commit between it and the task base.
- This task changes only correctness evidence in `checked_scan.rs` plus this handoff. It does not rewrite production, dependencies, or lockfiles.
- The bridge owns the next commit and this handoff intentionally does not self-name that commit.

## Pre-edit checkpoint (verbatim)

- Base identity: `HEAD = 2e4bba41c9d55b6d517b0379f74585192736ad84`; `c637e493` is its ancestor and the range count is one.
- Clean-tree identity: `git status --porcelain=v1` and both staged/unstaged diffs were empty before the first edit.
- Accepted refactor anchor: `sweep.rs:18,345,650` declares the private child and routes both public scan functions through it.
- Harness anchor: `checked_scan.rs:253` had `Script`, `sidecar`, `decoded_custody`, `temp_root`, and exactly four child `#[test]` functions.
- Canonical-decode anchor: `custody.rs:646-667` encodes canonical bytes and rejects a non-byte-identical decode.
- Selection/route anchors: `checked_scan.rs:138-170,192-234`; `provider_path.rs:140,160`; `custody.rs:694,822`.
- Decision anchors: `sweep.rs:189-204,495-536` maps `BothAbsent` to `Authorized`, all other probe results to `Refused`, and unreadable custody to refusal without probing.
- Public API anchor: `r2f1b_exact_absence_report_api.rs:14,31,73-75` already pins `effective()` item type and both public function signatures.
- Tooling anchors: Rust 1.94.0; `bridge-worktree/Cargo.toml:23-25` has exactly two dev dependencies; CI coverage is `.github/workflows/ci.yml:96`; hygiene is `AGENTS.md:34`; no `handoff-template` file exists.
- Deferred boundary anchor: A2a-2 owns the listed characterization cases; no such scenario was added here.
- Revised worksheet estimate: fixes 52/60; correctness evidence 134/190; shared harness extension 40/40; handoff 90/100; total 316/390.
- Decision: proceed. All claimed production seams and mappings were present; the only observed operational blocker was unavailable registry resolution, not a falsified repository anchor.

## Implemented correctness evidence

- `decoded_custody` canonical-fixture evidence catches noncanonical test bytes by creating a record, obtaining `encode_canonical()` bytes, then decoding them (F1).
- Pin-failure ordering evidence catches an unspecified-enumeration index assumption: injected `Script` asserts exactly two rows and pins legacy then custody-refusal at indices zero and one (F2); every injected exact projection now uses production-equivalent `canonicalize_lenient`.
- Root-refusal evidence catches an invalid fixture assumption: `""` has no file-name ancestor for `canonicalize_lenient`, proving `CannotCanonicalize` and zero opener calls (F3).
- Exact-projection retention evidence catches recomputation or loss of the engine decision by asserting production-projected rows retain it unchanged.
- Legacy/custody matrix evidence catches an incorrect production decision mapping for every observation class.
- Unreadable-custody no-probe evidence catches any assessment of a refusal row: it asserts `Refused` and zero probe calls.
- Exact-route root/return evidence catches raw-root forwarding or public-return drift: it proves the canonical root reaches the opener and the wrapper remains unit-returning.
- The existing external signature/API evidence catches visibility, exact function-type, and `effective()` item-type drift; it remains the fifth required correctness assertion.

## Source audit

| Edge or invariant | Evidence |
| --- | --- |
| Action wrapper and engine | `sweep.rs:345-366 -> checked_scan.rs:234-242 -> :192-232`; the action wrapper reaches the sole engine. |
| Exact wrapper and engine | `sweep.rs:623-644 -> checked_scan.rs:234-242 -> :192-232 -> sweep.rs:583-619`; exact drives the same engine and projection. |
| Production projections | Action maps `into_action_rows` only at `sweep.rs:349-353`; exact maps `into_exact_parts` only at `:583-589`. |
| Injected projections | Test-only `checked_scan.rs:246-251` drives the same engine; its exact projections use `crate::provider_path::canonicalize_lenient` at `:502,548,599,648,672`, matching production symlink resolution before the production projections. |
| Privacy and construction | Traits/source/session/classifier/refusal are child-private (`checked_scan.rs:31-67,180-232`); private checked-result fields are constructed only by the engine. |
| Parent access and allowances | The parent uses only authorized consuming accessors; `checked_scan.rs:80-95` contains exactly six field-scoped root-observation allowances and no broad/result allowance. |
| Classifier facts | Full lossy joined display path at `checked_scan.rs:205` is legacy-first then `is_custody_record_name` (`:180-187`; `custody.rs:694-700`): `.custody.v1.json` and `dir/.custody.v1.json` reject; `dir\.custody.v1.json` accepts. |
| Session operation count | The non-test engine has one each `next_name :199`, `read_legacy :210-212`, `read_custody :213-217`, and `finish :228`; all are inside it. |
| Exact outcome retention | `sweep.rs:583-619` retains canonical root, iterator errors, observations, checked rows/names, and stored decisions; `:589-595` retains canonical root on enumeration refusal. |
| Decision matrix | `checked_scan.rs:617-657` proves for legacy and custody: `BothAbsent -> Authorized`; `TargetPresent`, `RegisteredButAbsent`, and probe `Err -> Refused`. |
| Unreadable custody | `checked_scan.rs:661-681` proves `Refused` with zero probe calls and removes its created fixture root; the production refusal arm is `sweep.rs:603-607`. |
| Test and public API construction | Injected tests construct completed results only through `scan_checked_rows_for_test`; routing tests call production wrappers. External `r2f1b_exact_absence_report_api.rs:14,31,73-75` pins `effective()` and both public functions. |
| Event source audit | Exactly one unguarded post-`finish` event at `sweep.rs:609-613`: stored `decision`, `path = projection_row.checked.record_path()`, unchanged literal `made exact-absence decision`, no duplicate emitter. |
| Root split and deferred owner | Action receives raw `Path::new(root)` (`sweep.rs:357-366`); exact forwards canonical root (`:623-644`); A2a-2/A2b obligations remain in the deferred section below. |

## Toolchain

- `rustc --version --verbose`: rustc 1.94.0 (4a4ef493e, 2026-03-02), host `aarch64-unknown-linux-gnu`, LLVM 21.1.8.
- `cargo --version`: cargo 1.94.0 (85eff7c80, 2026-01-15).
- `rustfmt --version`: rustfmt 1.8.0-stable (4a4ef493e3, 2026-03-02).
- `cargo clippy --version`: clippy 0.1.94 (4a4ef493e3, 2026-03-02).

## Gates and guards

| Command | Result | Totals |
| --- | --- | --- |
| `cargo fmt --all -- --check` | exit 0 | formatter gate passed. |
| `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 101, blocked | crates.io proxy returned 403 fetching `a2a-lf`; no compilation, test binary, or doc-test suite started. |
| `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` | exit 101, blocked | same dependency-resolution failure; no totals produced. |
| `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` | exit 101, blocked | same dependency-resolution failure; no totals produced. |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) | exit 101, blocked | same dependency-resolution failure; guard is not a test gate. |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) | exit 101, blocked | same dependency-resolution failure; guard is not a test gate. |

The offline control also failed before compilation because `arc-swap` is absent from the local index. The blocked mandatory gates leave this evidence pending; no exclusion is treated as acceptance.

## Whitespace, manifest, and counts

- `cargo fmt --all -- --check` and the staged-equivalent `git diff --check HEAD` exit 0.
- `git diff --exit-code HEAD -- crates/bridge-worktree/Cargo.toml Cargo.lock` exits 0; both remain unchanged.
- The bridge contract forbids the implementer from creating the candidate commit, so the base-to-candidate commit-range check cannot run before bridge commit; the current diff check is recorded instead.
- Final worksheet (added nonblank physical lines versus task base): 53/60 test-fix lines; 126/190 correctness-evidence lines; 40/40 shared-harness lines (`identity` and the reusable portion of `valid_records`); 71/100 handoff lines; total 290/390. The remaining record-specific custody-claim materialization is assigned to its decision-matrix evidence.
- Reconciliation: compared with the pre-edit estimate, the canonical-root repair adds one test-fix line; measured correctness evidence is eight lines lower, shared harness is unchanged, and the compact handoff is nineteen lines lower, yielding the measured 26-line total reduction.

## Deferred ownership and protocol

- A2a-2 owns the deferred classifier, malformed-record, iterator, root-observation, action-projection, and report-side characterization scenarios listed by the task.
- A2b still owns retained-descriptor observations, report population, tracing evidence, platform matrix, and the final combined handoff.
- This inline handoff is the owner-approved implementer-side replacement for any external template; the host operator separately owns its lane template.
- Historical custody note: the already-created `cb4d9c29` combines source and handoff, so its recorded staged checks were not an isolated handoff-only protocol. This repair cannot split that commit under the current no-reset/no-commit contract; operator action must rebuild the two commits from `2e4bba41` in the required order.
