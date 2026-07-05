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

### Full workspace suite — after slice 2 (stable tree)
```
cargo test --workspace -j 1
```
**1424 passed; 0 failed; 12 ignored — across 60 test binaries.** (`-j 1` per the
repo's known linker-OOM on heavy test builds.) Independently run by the
orchestrator against the post-slice-2 tree, not taken on the implementor's word.

### Per-slice detail
- **Slice 1 — scaffold (`c0be85d`):** `cargo build -p bridge-controller` clean;
  workspace member confirmed. Empty crate → build is complete verification.
- **Slice 2 — move `turn` + `review`:** files moved as pure renames (zero content
  diff); rewired via one crate-root re-export shim
  (`pub(crate) use bridge_controller::{review, turn};`) + a one-line
  `bridge-controller` path dep added to `bin/a2a-bridge/Cargo.toml`. The 28 moved
  inline `review`/`turn` tests now run in `bridge-controller`; the bin's 325 tests
  still pass. Whole workspace green (totals above).

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
