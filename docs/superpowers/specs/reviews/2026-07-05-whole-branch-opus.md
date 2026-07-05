# Opus 4.8 whole-branch review — `feat/bridge-controller-extraction` (6e97ecd) vs `main` (622fe09)

_Independent lens (fresh context), read-only. Verified against the rename-aware
diff (`git diff main...HEAD -M`), full source reads, and a clean `cargo build` /
`cargo test` / `cargo clippy --workspace -j 1`._

## 1. Behavior preservation — CLEAN
- 100%-similarity renames (`turn`, `review`, `implement`, `implement_resume`,
  `resilient`) have **0 content ± lines** — pure moves.
- `tweak.rs` (99%): only change is dropping `#[allow(dead_code)]` on
  `NoopCheckpointSink`. Behavior-neutral; clippy silent → genuinely reachable.
- `verify.rs` (97%): `use crate::config::VerifyConfig` replaced by the moved
  struct def; `docker_runner` cut out. No logic touched.
- `merge.rs` (90%): `MergeConfig` struct inserted; `merge_clone` signature
  `crate::config::MergeConfig`→`MergeConfig`; `MERGE_USAGE`/`merge_cmd`/its test
  cut out. No logic touched.
- Relocated struct defs byte-faithful: `VerifyConfig` identical; `MergeConfig`
  only `crate::merge::OperatorIdent`→`OperatorIdent`. Both keep `Debug, Clone`.

## 2. Security invariant (tri-state) — PRESERVED
- `ProdEffects.verify_cfg`, `run_verify_step`, `warm_lsp_deps_step`,
  `run_warm_loop` all still carry `&Option<Result<verify::VerifyConfig,
  config::ConfigError>>` — not collapsed (main.rs:1248/1302/1651/1902).
- `merge_after_loop` still `Option<Result<merge::MergeConfig, config::ConfigError>>`
  (main.rs:1977).
- `run_verify_step` still maps `None => NotConfigured`, `Some(Err) => ConfigError`,
  `Some(Ok) => run` (main.rs:1266-1270); `tweak::classify` still
  `NotConfigured => verify_ok=true`, `ConfigError => verify_ok=false`
  (tweak.rs:63/65). Config error stays non-Approved → non-mergeable. Intact.

## 3. Extracted-back code — FAITHFUL
- `docker_runner` (main.rs:1243): identical body; `pub fn`→`fn` (correctly
  narrowed, bin-local); 3 call sites re-pointed.
- `merge_cmd` (main.rs:2673): faithful; `crate::`-prefixes dropped for now-local
  items; internal call correctly qualified `merge::merge_clone` (main.rs:2723).
- `MERGE_USAGE` (main.rs:2665) + `merge_usage_matches_the_actual_parser`
  (main.rs:6725): verbatim; both dispatcher refs updated to bare `MERGE_USAGE`.

## 4. Boundary vs spec — CORRECT
`VerifyConfig`+`MergeConfig` → lib; `ReviewConfig`/`LoopConfig`, all `*Toml`/
`to_config`/`gate_verify_runtime`/`RegistryConfig`/`ConfigError` stayed at bin.
Ports + pure loops + primitives moved. No misplacement.

## 5. Structure — SOUND
One-way dep edge bin→`bridge-controller`→`bridge-core`/`bridge-workflow`; no
cycle. Re-export shim cleanly replaces the removed `mod` decls; no name clash.
`impl TweakEffects for ProdEffects` stays at the bin — foreign trait + local
type, orphan rule satisfied.

## 6. Tests — NONE LOST
Per-module `#[test]` counts identical except merge 13→12; the one is exactly
`merge_usage_matches_the_actual_parser`, now in the bin's `cli_tests`. Net zero.
`cargo test -p bridge-controller -j 1`: 108/0. Bin usage test passed.
`cargo clippy --workspace -j 1`: clean.

---
- **BLOCKER:** none. **MAJOR:** none. **MINOR:** none material.
- **BRANCH VERDICT: SHIP** — a faithful, behavior-preserving module move; the
  previously-regressed verify/merge tri-state is intact and non-mergeable-on-
  config-error is preserved; all tests and clippy green.
