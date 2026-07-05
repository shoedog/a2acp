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

## Verified
- Full workspace test suite green (1424/0/12) on the post-slice-2 tree.
- Slice 2 is a behavior-preserving move: source files are byte-identical renames;
  the shim re-points every `crate::review::`/`crate::turn::` reference with zero
  call-site edits; no logic touched.
- Slice 1 scaffold builds; dependency set matches the dual-review corrections
  (`futures` added; `tempfile`/`tokio-stream` dev-deps).

## Not verified (pending)
- Slices 3–6 behavior preservation (each gated by the full suite before its
  commit, recorded here).
- Whole-branch review (opus 4.8 + codex xhigh) before merge, per the pipeline.

## Out-of-scope failures
- None. The full workspace suite showed 0 failures; nothing was re-baselined or
  silently fixed.
