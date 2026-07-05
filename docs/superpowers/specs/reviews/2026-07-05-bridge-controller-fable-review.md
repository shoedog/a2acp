# Fable review — `bridge-controller` extraction spec (2026-07-05)

_Read-only adversarial review, repo-rooted, dispatched as the deepest-reasoning
lens for a one-way-door decision. Verdict REVISE. This lens uniquely caught the
security regression (B1) that the codex lens missed; both caught the
slice-ordering blocker (B2)._

## BLOCKER

**B1 — Slice 5's ConfigError-free mechanism is behavior-changing and weakens the merge gate.**
The spec (§Config, slice 5) changes `ProdEffects`/`run_*_step` from
`&Option<Result<VerifyConfig, ConfigError>>` to resolved `Option<&VerifyConfig>`.
That collapse destroys a load-bearing tri-state. `run_verify_step`
(main.rs:1263-1268) maps `None → NotConfigured`, `Some(Err) → VerifyOutcome::ConfigError`;
`tweak::classify` treats these oppositely — `NotConfigured => verify_ok=true`,
`ConfigError => verify_ok=false` (tweak.rs:61-65); same for `ReviewOutcome`
(tweak.rs:67-70). Under the collapsed signature, a `[verify]` config error
(including a **disallowed runtime rejected by `gate_verify_runtime`**,
config.rs:730-733 — whose doc comment explicitly promises "flows into the
existing `VerifyOutcome::ConfigError` path, no container spawns") becomes `None`
→ `NotConfigured` → `verify_ok=true`. With an approving review the loop then
stops `Success` → phase `Approved` → `--merge` lands (main.rs:1981-1989), where
today it stops `NotActionable` → `LoopStopped` → merge refuses without `--force`
(merge.rs:51-61). That is a silent weakening of the runtime-allowlist gate
(ADR-0017/0030 lineage) disguised as a refactor. The alternative reading — bin
aborts on `Err` pre-adapter — is also a behavior change (today a bad `[verify]`
block degrades soft; the run still completes with a hand-off). Same tri-state
issue in `warm_lsp_deps_step` (main.rs:1314-1324) and `merge_after_loop`'s lazy
`transpose()` (main.rs:1983-1985).
**The fix is trivial and makes the change unnecessary:** the boundary is
*already* ConfigError-free. No moved module names `ConfigError` — verified
exhaustively: the only `config::` refs in movable files are `verify.rs:6`
(`VerifyConfig`), `merge.rs:367` (`Option<&MergeConfig>`), `merge.rs:546`
(`RegistryConfig::parse`, inside `merge_cmd` which stays). Library entry points
already take resolved values. `Option<Result<_, ConfigError>>` is a
*bin-internal* type in bin-side adapters; a bin freely combines its own
`ConfigError` with library `VerifyConfig`. Delete the signature change.

**B2 — The slice order does not compile at slices 2, 3, and 4.**
Each slice is gated "green on the full test suite," but the order moves modules
before their dependencies:
- **Slice 2** moves `verify.rs`, which imports `crate::config::VerifyConfig`
  (verify.rs:6, param :146) — `VerifyConfig` stays at bin until slice 5. Hard break.
- **Slice 3** moves `implement_resume.rs`, which does
  `impl crate::tweak::CheckpointSink for ProdCheckpoint` (:92) and uses it in
  tests (:257) — `tweak` stays until slice 4. Hard break.
- **Slice 4** moves `merge_clone`, whose signature names `crate::config::MergeConfig`
  (merge.rs:367) — configs move in slice 5. Hard break.
- **Slice 2** also moves `resilient.rs`, whose test calls
  `crate::implement::reset_worktree_to_head` (:528) — `implement` moves in slice 3.
  Test-compile break.
Valid topological order: **(a)** turn + review; **(b)** `VerifyConfig`(+`LoopConfig`)
+ verify + implement; **(c)** tweak + implement_resume + resilient; **(d)**
`MergeConfig` + `ReviewConfig` + merge split; **(e)** tighten. Rule the spec
missed: *a resolved config must move with its first library consumer, not last* —
and `ReviewConfig` can't move before `review.rs` (field `default_depth:
crate::review::Depth`, config.rs:817).

## MAJOR

**M1 — The integration-test claim is false.** "~29 integration-test binaries …
switch to `bridge_controller::…`" — there are **11** test files, and **zero**
reference controller internals. They *cannot*: the package has only a `[[bin]]`
target, so `tests/*.rs` has no lib to link. Migration work there is zero. But a
spec that greps its own repo wrong on a checkable claim should be re-verified
elsewhere. ("118 dispatch call sites" is ~156 matching lines in main.rs;
immaterial.)

**M2 — Dependency list is wrong in three places (slice 1).** (a) **`futures` is
missing** — `turn.rs:13` uses `futures::StreamExt`. (b) **`tempfile` is listed
as a regular dep** but is used only in `#[cfg(test)]` (dev-only in the bin
today). (c) **`tokio-stream` missing from dev-deps** — resilient.rs:133
`tokio_stream::iter` in tests. Correct: deps = {bridge-core, bridge-workflow,
serde(derive), serde_json, tokio, async-trait, futures}; dev-deps = {tempfile,
tokio-stream}.

**M3 — config.rs is a missed rewrite site.** `config.rs` (stays at bin) names
moved types in *product code*: `ReviewConfig.default_depth: crate::review::Depth`
(config.rs:817, constructed :851) and `MergeConfig.author:
Option<crate::merge::OperatorIdent>` (config.rs:967, :982), plus its tests
(:3826, :3873). Rewrite to `bridge_controller::` at the slice where `review`/
`merge` move.

**M4 — merge.rs test modules split across the crate line.** `mod tests` in
merge.rs mixes moved-item tests (`gate_matrix`, `resolve_target_…`) with
`merge_usage_matches_the_actual_parser` (merge.rs:579-589), which pins
`MERGE_USAGE` — a constant that must stay bin-side with `merge_cmd` (main.rs's
dispatcher prints it, merge.rs:503-506). The split slice must divide this module.

## MINOR
- gate_verify_runtime "3 call sites 2142/2482/2888" conflates the `to_config`
  lines with the gate lines (actual gates: 2168, 2498, 2931; 4th in tests 7037).
  Substance (3 product sites) holds.
- `turn::drain_turn` hardcodes the `[implement]` log prefix (turn.rs:26) into a
  generic library port — cosmetic, covered by the eprintln seam.
- `LoopConfig` and `ReviewConfig` have **no library consumer** — all fields read
  in bin adapters. Moving them is #10-speculative, not a #9 need; harmless, but
  D1's "settled" overstates.
- `NoopCheckpointSink` carries `#[allow(dead_code)]` (tweak.rs:165) — drop it
  post-move (pub lib items aren't dead).
- Spec line refs otherwise verified accurate.

## ANSWERS
1. **Adapter-stays-at-bin: RIGHT.** `ProdEffects`/`run_review_step`/`build_warm_impl`
   transitively require bin-only machinery: `slice::SliceRunner`+`ProdSliceRunner`
   (main.rs:1523/1686), `bridge_container::ContainerRwBackend`/`ContainerSpawn`,
   `PolicyEngine`, `AcpContainerSpawn`, `container_owner` (main.rs:1714-1731).
   Moving them drags bridge-container/bridge-policy in + 3 more ports — scope
   creep, no MCP payoff (`a2a-bridge mcp` runs in-process; ports let embedders
   supply their own effects).
2. **ConfigError boundary: already achieved; the spec's mechanism is the bug.**
   No moved path constructs/returns/matches `ConfigError`; no lazy `to_config`
   in moved code. Drop the slice-5 rewrite (B1).
3. **Orphan rule: clean everywhere — the spec worried about the wrong hazard.**
   Every straddling impl is foreign-trait-on-**local**-type
   (`impl tweak::TweakEffects for ProdEffects` main.rs:1670;
   `impl TurnRunner for ResilientWarm` resilient.rs:69;
   `impl CheckpointSink for ProdCheckpoint` implement_resume.rs:92 — the latter
   two intra-library). No borrow changes. The real breakage is reverse-direction
   module references (library→bin) = plain unresolved paths (B2).
4. **Keep `eprintln!`: acceptable** for a behavior-preserving #9.
5. **Cargo entanglement: lighter than feared, dep list wrong (M2).** No cycle
   (bin-only package); no bundled SQLite (rusqlite only in bridge-store); no
   bridge-observ/bridge-policy needed.
6. **Granularity: right size, wrong order.** Six slices, re-sequenced
   topologically (B2's order), co-locating each config with its first consumer.

## VERDICT
**REVISE.** The crate boundary is sound — no cycles, no coherence breaks,
adapters correctly at the composition root — but two spec-level defects must be
fixed: **(1)** delete the slice-5 adapter-signature change (the boundary is
already ConfigError-free; as written it silently converts verify-config errors,
including gate-rejected runtimes, into `Approved`+mergeable runs); **(2)**
re-sequence the slices topologically. Also correct the dep list, the
integration-test claim, and add config.rs + MERGE_USAGE-test to the rewrite plan.
