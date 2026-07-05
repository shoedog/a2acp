# VERIFICATION — bridge-controller extraction (#9)

Branch: `feat/bridge-controller-extraction`.

**This is a BEHAVIOR-PRESERVING refactor** (module extraction from the bin crate
into `bridge-controller`). No new behavior is added or fixed, so the standard
"add a test that FAILS on pre-change code, one negative/edge case per new path"
discipline does not apply — there is no new path. The correctness gate is the
inverse and stronger: **the entire pre-existing test suite passes UNCHANGED after
every slice.** The existing tests ARE the behavior contract. This ledger is
updated at each slice's verification checkpoint; the whole-branch review
(opus 4.8 + codex xhigh) runs before merge.

## Commands run + results

### Full workspace suite — after slice 5 (stable tree)
```
cargo test --workspace -j 1
```
**1424 passed; 0 failed; 12 ignored — across 60 test binaries.** (`-j 1` per the
repo's known linker-OOM on heavy test builds.) Independently run by the
orchestrator against the post-slice-5 tree. Slices 3, 4, and 5 each drew alarming
mid-edit IDE-diagnostic snapshots (phantom errors); in every case `cargo build`
against the final tree was clean — rustc is authoritative, IDE snapshots are not.
Verified from scratch each slice regardless of the implementor's report.

### Per-slice detail
- **Slice 1 — scaffold (`c0be85d`):** `cargo build -p bridge-controller` clean;
  workspace member confirmed. Empty crate → build is complete verification.
- **Slice 2 — move `turn` + `review` (`125e6b3`):** pure renames; one crate-root
  re-export shim + a one-line `bridge-controller` path dep. Full suite 1424/0/12.
- **Slice 3 — move `verify` (minus `docker_runner`) + `implement`; relocate
  `VerifyConfig` (`4824b29`):** `docker_runner` (5 impure lines) extracted to
  `main.rs`; `VerifyConfig` struct relocated `config.rs`→library `verify.rs`
  (plain `Debug, Clone`, no serde); `config.rs` keeps `VerifyToml::to_config` +
  `gate_verify_runtime` (logic untouched, resolving to the library type via a
  `use`); re-export → `{review, turn, verify, implement}`. Full suite 1424/0/12.
- **Slice 4 — move `tweak` + `implement_resume` + `resilient` (`4936cb9`):** three
  wholesale pure renames; all `crate::…` refs became intra-library; straddling
  trait impls stayed legal (`TweakEffects for ProdEffects` at bin; `CheckpointSink`/
  `TurnRunner`/`WarmRebuild` impls intra-library). Full suite 1424/0/12.
- **Slice 5 — `merge` split + relocate `MergeConfig`:** `merge.rs` moved wholesale;
  `merge_cmd` + `MERGE_USAGE` + the `merge_usage_matches_the_actual_parser` test
  extracted BACK to the bin (`main.rs`) — the CLI wrapper that parses args + the
  `RegistryConfig` file root stays at the composition root, per Fable M4. Its one
  internal call is qualified `merge::merge_clone`. `MergeConfig` struct relocated
  `config.rs`→library `merge.rs` (co-located with `OperatorIdent`); `config.rs`
  keeps `MergeToml::to_config` (resolving to the library type via a `use`).
  re-export → `{implement, implement_resume, merge, resilient, review, tweak,
  turn, verify}`. Verified the usage test now runs in the bin's `cli_tests` and
  the 11 merge-logic tests in `bridge-controller`. Full suite 1424/0/12.

- **Slice 6 — tighten + final verification:** confirmed the Fable B1
  invariant HELD — all four `verify_cfg` adapter fields still carry
  `Option<Result<verify::VerifyConfig, config::ConfigError>>` and `run_verify_step`
  still maps the full `None`/`Some(Err)`/`Some(Ok)` tri-state (no signature drift;
  the gate-rejected-runtime → non-mergeable distinction is intact). Dropped the now
  redundant `#[allow(dead_code)]` on the `pub NoopCheckpointSink`. `cargo clippy
  --workspace` clean; full suite 1424/0/12.

## Verified
- **EXTRACTION COMPLETE.** The whole controller cluster (turn, review, verify,
  implement, tweak, implement_resume, resilient, merge — minus `docker_runner` and
  `merge_cmd`/`MERGE_USAGE`) plus `VerifyConfig`/`MergeConfig` now live in
  `bridge-controller`; the bin keeps config parsing + the effects adapters + CLI
  dispatch, wired through one crate-root re-export shim.
- Full workspace test suite green (**1424 passed / 0 failed / 12 ignored**, 60 test
  binaries) on the final tree; `cargo clippy --workspace` clean.
- Behavior-preserving throughout: moved source is byte-identical (renames) or
  namespace-only edits; no logic touched; adapter signatures unchanged (Fable B1).
- Dependency set matches the dual-review corrections (`futures` added;
  `tempfile`/`tokio-stream` dev-deps); no dependency cycle; no orphan-rule break.

## Not verified (pending)
- Whole-branch review (opus 4.8 + codex xhigh) before merge, per the pipeline —
  the next gate.

## Out-of-scope failures
- None. Every slice's full workspace suite showed 0 failures; nothing was
  re-baselined or silently fixed. Slices 3/4/5 drew alarming mid-edit IDE
  diagnostics that were all stale (rustc clean each time).
