# `bridge-controller` extraction — architecture spec v2 (roadmap #9)

**Status:** IMPLEMENTATION-READY. v2 folds the dual review (codex gpt-5.5 xhigh +
Fable), both verdict REVISE. All findings dispositioned in §Reviews; the crate
boundary itself was validated by both lenses (no dependency cycle, no
orphan-rule break, adapters correctly at the composition root). The revisions
are sequencing + scope + dependency fixes, plus **deletion of a
security-regressing "fix" from v1** (Fable B1).
**Supersedes:** the v1 draft + `2026-07-05-config-split-prototype.md`.
**Date:** 2026-07-05.

## Goal & non-goals

**Goal.** Extract the controller-loop *primitives* (~6,400 lines in
`bin/a2a-bridge`) into a `bridge-controller` library crate with a public API,
leaving `main.rs` as composition + adapters. Payoffs: testability (pure logic +
ports unit-testable without a binary/live agents), reuse (the #10 Coordinator
can compose the loops), and the biggest dev-loop fix (the bin stops being the
largest serial link unit + the `--all-targets` OOM's prime suspect).

**Non-goals.** (1) **Behavior-preserving** — no logic changes; the existing suite
must pass unchanged. (2) NOT #10 — does not relocate the composition
(`run_warm_loop` et al.) into the Coordinator; it gives #10 a library to compose.
(3) No new features, no CLI changes. (4) **No adapter signature changes** — see
§Config (the v1 signature change is deleted).

## The load-bearing finding: the ports seam already exists

The `tweak` loop is already ports-and-adapters, and the pattern generalizes:

```rust
// LIBRARY (tweak.rs): PORT + pure loop, driven by injected effects.
pub trait TweakEffects {
    async fn verify(&mut self, attempt: u32) -> VerifyOutcome;
    async fn review(&mut self, attempt: u32, head_sha: &str) -> (ReviewOutcome, String);
    async fn fix(&mut self, attempt: u32, input: &str) -> bool;
}
pub async fn run_tweak_loop(.., eff: &mut dyn TweakEffects, ckpt: &mut dyn CheckpointSink) -> LoopFinal;

// BIN (main.rs): production ADAPTER, wiring the port to the live executor + warm session.
struct ProdEffects<'a> { /* executor, TurnRunner, resolved profile, session ids, verify_cfg, review_cfg … */ }
impl tweak::TweakEffects for ProdEffects<'_> { /* delegates to run_verify_step / run_review_step / fix turn */ }
```

**Four ports** live at this seam (v1 missed one — codex): `TweakEffects`,
`TurnRunner`, `CheckpointSink`, **`WarmRebuild`** (`resilient::ResilientWarm::new`
takes `Arc<dyn WarmRebuild>`; the bin impls `ContainerWarmRebuild`,
main.rs:1714/1721). The library owns the ports + pure loops + primitives; the bin
owns the adapters that compose them with the live backend.

## Target architecture

### Dependency direction & crate deps
`bin/a2a-bridge` → `bridge-controller` → lower crates. Never the reverse.
**Verified acyclic by both lenses:** the bin is a `[[bin]]`-only package (nothing
can depend on it — Fable M1); `bridge-workflow` depends down on `bridge-core`
only. No bundled SQLite in this subtree (rusqlite is isolated to `bridge-store`);
no `bridge-observ`/`bridge-policy` needed (`bridge_observ::init` is in
`implement_cmd`, which stays).

```toml
# crates/bridge-controller/Cargo.toml
[dependencies]
bridge-core = { path = "../bridge-core" }
bridge-workflow = { path = "../bridge-workflow" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"                 # implement_resume checkpoints
tokio = { version = "1", features = [...] }
async-trait = "0.1"              # the async-fn ports
futures = "0.3"                  # turn::drain_turn uses futures::StreamExt  (v1 MISSED — both lenses)
[dev-dependencies]
tempfile = "3"                   # git-heavy tests only (NOT a prod dep — Fable M2)
tokio-stream = "0.1"             # resilient tests
```

### What moves to `bridge-controller`

| unit | disposition | note |
|---|---|---|
| `turn.rs` | wholesale | `TurnRunner` port; needs `futures` |
| `review.rs` | wholesale | pure (`select_tier`, `parse_verdict`, `reduce`, `Depth`) |
| `verify.rs` | wholesale **minus `docker_runner`** | `run_verify(&VerifyConfig, .., runner: &Runner, ..)` injects its runner; `docker_runner` (concrete spawn) **stays** and is passed in by `run_verify_step` (codex MINOR) |
| `implement.rs` | wholesale | pure git/argv/decision fns |
| `implement_resume.rs` | wholesale | checkpoint persistence; **brings serde_json** (so the crate is not serde-free; the resolved *config structs* are) |
| `tweak.rs` | wholesale | `TweakEffects` + `run_tweak_loop` + `classify` |
| `resilient.rs` | wholesale | `classify_death`, `ResilientWarm` (impls `TurnRunner`), the `WarmRebuild` port |
| `merge.rs` | **split** | pure + `merge_clone(Option<&MergeConfig>, ..)` move; **`merge_cmd` + `MERGE_USAGE` stay** (merge.rs:518/546; the dispatcher prints `MERGE_USAGE`) |
| **`VerifyConfig`, `MergeConfig`** | **move** | the only resolved configs with a **library consumer** (`run_verify`, `merge_clone`). Plain `Debug, Clone` (already no `Deserialize` — codex). `MergeConfig` embeds `merge::OperatorIdent` (moves with merge) |
| **`ReviewConfig`, `LoopConfig`** | **STAY at bin** | Fable refinement — no library consumer; only bin adapters read them (`run_review_step`; loop bounds in `implement_cmd`). `ReviewConfig` references `bridge_controller::review::Depth` after review moves (bin→lib, fine) |

### What stays at the bin (composition root)

- **All config parsing:** the 19 `*Toml` DTOs, `RegistryConfig`, `parse()`,
  `into_snapshot()`, every `to_config()` resolver, `gate_verify_runtime`
  (gate call sites main.rs:2168/2498/2931 — Fable MINOR corrects v1's line refs),
  `ConfigError`, `FileConfigSource`, `ReviewConfig`, `LoopConfig`, all file/env IO.
- **The effects-adapter layer:** `ProdEffects`, `run_verify_step`,
  `run_review_step`, `run_warm_loop`, `build_warm_impl`, `merge_after_loop`, the
  LSP-warm helpers, `docker_runner`, `ContainerWarmRebuild`, `ProdSliceRunner`.
  These transitively require bin-only machinery — `slice::SliceRunner`,
  `bridge_container::ContainerRwBackend`, `PolicyEngine`, `AcpContainerSpawn`,
  `container_owner` (Fable, main.rs:1523/1686/1714-1731). This is the layer #10
  relocates into the Coordinator.
- **Arg-parse + dispatch:** `implement_cmd`, `implement_resume_cmd`, `merge_cmd`,
  `run_workflow_cmd`, `parse_*_args`, `type BoxError`.

## The config boundary — the v1 "fix" is DELETED (Fable B1, security-critical)

**The library boundary is ALREADY `ConfigError`-free. No signature change.**

The only `config::` references in movable files are `verify.rs` (`VerifyConfig`
type), `merge.rs:367` (`Option<&MergeConfig>`), and `merge.rs:546`
(`RegistryConfig::parse`, inside the staying `merge_cmd`). No moved module
constructs/returns/matches `ConfigError`; no moved module calls `to_config`
(every resolver call is bin-side and eager). Library entry points already take
resolved values (`run_verify(&VerifyConfig)`, `merge_clone(Option<&MergeConfig>)`).

`ProdEffects` and `run_verify_step`/`run_review_step` **stay at the bin** and keep
their `&Option<Result<VerifyConfig, ConfigError>>` fields unchanged — a bin
adapter freely names its own `ConfigError` alongside a library `VerifyConfig`.

⚠️ **v1 proposed collapsing that to `Option<&VerifyConfig>` "for cleanliness."
That was a security regression and is DELETED.** The tri-state is load-bearing
(verified main.rs:1263-1268, tweak.rs:60-64):

```
run_verify_step:  None => NotConfigured ;  Some(Err) => ConfigError ;  Some(Ok) => run
classify:         NotConfigured => verify_ok=TRUE ;  ConfigError => verify_ok=FALSE
```

Collapsing `Some(Err)`→`None` turns a `[verify]` **config error — including a
`gate_verify_runtime`-rejected disallowed runtime** (config.rs `gate_verify_runtime`
doc: "flows into `VerifyOutcome::ConfigError`, no container spawns") — from
`verify_ok=false` (loop stops `NotActionable`→merge refuses without `--force`,
merge.rs:51-61) into `verify_ok=true` (loop stops `Success`→`Approved`→`--merge`
lands, main.rs:1981-1989). That silently weakens the ADR-0017/0030 runtime
allowlist. Keep the adapters exactly as they are; the boundary needs no help.

## Public API surface

- **Ports:** `TweakEffects`, `TurnRunner`, `CheckpointSink`, `WarmRebuild`.
- **Loops:** `run_tweak_loop`, `merge_clone`, `implement::{decide, compose_warm_fetch}`.
- **Primitives (the free testability win):** `parse_verdict`,
  `parse_diff_for_depth`, `select_tier`, `classify`, `aggregate`,
  `failure_digest`, git argv/host-commit/guard fns, checkpoint load/save.
- **Types:** `VerifyConfig`, `MergeConfig`, `VerifyOutcome`, `ReviewOutcome`,
  `LoopReport`/`LoopFinal`, `Depth`, `OperatorIdent`, `ImplementCheckpoint`,
  the `Runner` fn alias.

## Embeddability (both lenses: accept + document)

Moved code prints directly (~14 sites: `merge_clone` merge.rs:483,
`turn::drain_turn` turn.rs:26, `resilient` 101/107/112, checkpoints 99/109).
Direct git/fs IO in the library is accepted (it is a git-orchestration library;
its pure fns are the test surface). `eprintln!` diagnostics are **kept as-is**
(behavior-preserving) and **documented as part of the public API**, with a
recorded seam: a follow-up may route them through `tracing`/an injected sink if
an embedder needs quiet/asserted output. Not marketed as a quiet embeddable API
until that seam lands.

## Test migration (corrected — Fable M1/M3/M4)

- **`tests/` integration binaries: ZERO work.** The package is `[[bin]]`-only (no
  `[lib]`), so the 11 `tests/*.rs` e2e files link nothing internal and reference
  no controller symbols. v1's "~29 binaries switch to `bridge_controller::`" was
  false; deleted.
- **Inline `#[cfg(test)]` blocks move with their modules.** Cross-module test
  refs become intra-library (`implement` tests → `crate::verify`, etc.).
- **`config.rs` is a rewrite site (M3):** it stays at the bin but names moved
  types in product code — `ReviewConfig.default_depth: crate::review::Depth`
  (config.rs:817/851) and `MergeConfig` construction (config.rs:967/982) — plus
  its own tests. Rewrite these to `bridge_controller::` at the slice where
  `review`/`merge` move. (Generalizes to: the `*Toml::to_config` resolvers
  construct library config types as those types move.)
- **`merge.rs` test-module split (M4):** `mod tests` mixes moved-item tests
  (`gate_matrix`, `resolve_target_*`) with `merge_usage_matches_the_actual_parser`
  (merge.rs:579-589), which pins the staying `MERGE_USAGE`. Divide the module:
  moved tests to the library, the usage test stays with `merge_cmd`.
- Drop `NoopCheckpointSink`'s `#[allow(dead_code)]` post-move (pub lib items
  aren't dead — Fable MINOR).

## Build / OOM impact

Controller changes recompile `bridge-controller` (+ its unit tests) without
linking the bin. The library's tests are a fast separate unit. This overlaps the
`cargo test --all-targets` OOM fix (test-binary consolidation is an additive
follow-up).

## Slice plan v2 — topologically re-sequenced (both lenses: BLOCKER B2)

**Rule (Fable):** a module moves only after every movable thing it references,
and *a resolved config moves with its first library consumer, not last.* Each
slice ends green on the full existing suite (`cargo test -j 1`) + `clippy`; no
behavior change. Cross-module dep map (verified): implement→verify(test);
implement_resume→{implement,tweak}; tweak→{implement,review,verify};
resilient→{turn,implement(test)}; merge→{implement,implement_resume,MergeConfig}.

1. **Scaffold** `crates/bridge-controller` with the deps above. Empty, compiles.
2. **`turn` + `review`** (no movable deps; `turn` needs `futures`). Rewire
   `main.rs` imports; rewrite `config.rs` `ReviewConfig.default_depth` →
   `bridge_controller::review::Depth`.
3. **`VerifyConfig` + `verify` + `implement`** (verify needs `VerifyConfig`;
   implement's only movable ref is `verify` in a test). `docker_runner` stays;
   `run_verify_step` passes it. Rewrite `config.rs` `VerifyToml::to_config` to
   build `bridge_controller::VerifyConfig`.
4. **`tweak` + `implement_resume` + `resilient`** (deps turn/review/verify/
   implement now present; `tweak` before `implement_resume` for the
   `CheckpointSink` impl).
5. **`MergeConfig` + `merge` split** (move `merge_clone` + pure fns +
   `MergeConfig` together; keep `merge_cmd` + `MERGE_USAGE` + its usage test at
   the bin; divide `mod tests`). Rewrite `config.rs` `MergeToml::to_config`.
6. **Tighten:** confirm the ~156 `main.rs` call lines route through
   `bridge_controller::`; **adapters keep `Option<Result<_, ConfigError>>`
   unchanged (NO signature change — §Config)**; drop dead-code allows; sanity the
   public surface.

## Relationship to #10

#9 extracts primitives + ports. The composition (`run_warm_loop` = build session
→ `run_tweak_loop` → `merge_after_loop`, and `ProdEffects`) stays at the bin as
the reference adapter; #10 relocates it into the Coordinator, calling this same
library — which is why the library must stay composition-agnostic (ports, not a
hardwired executor).

## Reviews — every finding dispositioned

**Both verdicts REVISE; boundary validated by both (no cycle, no orphan break,
adapters-at-bin correct). Each lens caught what the other missed.**

- **Fable B1 (security, codex missed):** delete the v1 adapter signature change —
  it converts `gate_verify_runtime`-rejected runtimes into mergeable runs. §Config. ✔
- **B2 (both): slice order didn't compile** (moved modules before their
  types/traits). §Slice plan v2, topological. ✔
- **codex MAJOR (Fable missed): `WarmRebuild` port** absent from the API list.
  §Ports, §Public API. ✔
- **Both M2: deps** — add `futures` (hard); `tempfile`/`tokio-stream` → dev-deps.
  §Crate deps. ✔
- **Fable M1: integration-test claim false** (bin-only; 11 files; zero refs).
  §Test migration. ✔
- **Fable M3/M4: `config.rs` + `merge.rs` test-split** rewrite sites. §Test. ✔
- **Fable refinement: `ReviewConfig`/`LoopConfig` stay** (no library consumer).
  §Moves. ✔
- **codex MINOR: `docker_runner`** stays explicitly; `run_verify_step` passes it.
  §Moves. ✔
- **Both: embeddability** — keep `eprintln!`, document, record the seam. §Embeddability. ✔
- Line-ref corrections (gate sites, dead_code allow, drain_turn prefix): folded.

**No open design questions remain.** The v1 questions were answered by the dual
review; the one genuine disagreement (config-boundary mechanism) was adjudicated
against primary source in Fable's favor. Implementation may proceed on the
six-slice plan.
