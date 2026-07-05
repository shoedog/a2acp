# Config-split prototype — the linchpin decision for bin extraction (#9)

**Status:** PROTOTYPE for review (codex gpt-5.5 xhigh). Not implementation.
**Scope:** ONLY the config boundary. The full `bridge-controller` extraction
architecture is a follow-up spec that folds this decision in.
**Date:** 2026-07-05.

## Why this is the linchpin

Roadmap #9 extracts ~6,400 lines of controller loops
(`implement`/`merge`/`review`/`tweak`/`verify`/`resilient`/`implement_resume`/`turn`)
out of the `bin/a2a-bridge` crate into a `bridge-controller` library with a
public API, leaving `main.rs` as thin composition. A coupling probe showed the
move is otherwise **mechanical** (the cluster references only itself +
`config` + `turn` + `bridge_core` + `bridge_workflow`; `main.rs` calls into it,
never the reverse). The **one non-mechanical decision** is
`config.rs` (3,892 lines), which is shared between the CLI/serve composition
root and the controllers. This doc pins where the split line falls.

## Evidence

### The config type inventory (32 pub types in `config.rs`)

There is a latent, consistent **`*Toml` (parse DTO) → `*Config` (resolved
runtime)** pattern. `config.rs` carries **115 serde derives and 75
file/env/TOML IO operations** — essentially all of it is the parsing/IO layer.

- **19 `*Toml` DTOs** (serde `Deserialize`, the on-disk file shape):
  `BatchToml, WorktreesToml, WorkflowToml, PanelTomlSection, RetryToml,
  WorkflowNodeToml, PromptEntryToml, RegistrySection, AgentEntryToml, McpToml,
  EnvToml, SandboxToml, WatchdogToml, LanguageVerifyToml, LanguageToml,
  VerifyToml, ReviewToml, ImplementToml, MergeToml`.
- **9 `*Config` resolved types:** `ServerConfig, StoreConfig, DelegationConfig,
  RegistryConfig, BatchConfig, VerifyConfig, ReviewConfig, LoopConfig,
  MergeConfig`.
- **4 other:** `ConfigError` (error enum), `PromptSource`/`ResolvedPrompt`
  (workflow/prompt registry), `FileConfigSource` (file IO source).

### Which resolved configs the controllers actually consume

The controllers touch **zero `*Toml` types** (grep count 0 across all six). They
consume only resolved `*Config`, and only **five** of the nine:
`VerifyConfig, ReviewConfig, MergeConfig, LoopConfig` — plus `RegistryConfig`
(referenced by `merge.rs`). The other four (`Server/Store/Delegation/Batch`)
are serve/coordinator/batch config, not controller config.

### Field-level cleanliness of the four narrow resolved configs

All four are plain structs whose fields are std types + `bridge_core` +
sibling-controller types that **move with the extraction**:

```rust
pub struct VerifyConfig {           // clean: std + bridge_core
    pub runtime: Option<String>,
    pub image: String,
    pub cache: String,
    pub egress: bridge_core::domain::EgressPolicy,
}
pub struct LoopConfig {             // clean: std + bridge_core
    pub max_attempts: u32,
    pub fix_workflow: bridge_core::ids::WorkflowId,
    pub max_session_respawns: u32,
}
pub struct ReviewConfig {           // clean once review.rs moves: default_depth: review::Depth
    pub workflow: bridge_core::ids::WorkflowId,
    pub timeout: std::time::Duration, /* + slice_*, light_*, thorough_* primitives */
    pub default_depth: crate::review::Depth,
}
pub struct MergeConfig {            // clean once merge.rs moves: author: merge::OperatorIdent
    pub target_ref: Option<String>,
    pub author: Option<crate::merge::OperatorIdent>,
}
```

`review::Depth` and `merge::OperatorIdent` live in controller modules that move
to the library, so those references become **intra-library** after the move.
Each already has a `*Toml::to_config()` resolver in `config.rs` (validation +
env/tilde expansion), e.g. `VerifyToml::to_config() -> VerifyConfig`.

### The apparent `RegistryConfig` trap — and why it dissolves

`RegistryConfig` is **misnamed**: it is the whole-file config *root* — serde
`Deserialize`, embedding every `*Toml` DTO plus `ServerConfig`/`StoreConfig`/…,
with `parse()` and `into_snapshot()` methods. If a controller depended on it,
the entire serde/TOML/IO layer would be dragged into the library.

`merge.rs` is the only controller that names it — but **only in the CLI
wrapper**, not the orchestration:

```rust
// STAYS AT BIN — composition root: parse args, load+resolve file, delegate.
pub async fn merge_cmd(args: &[String]) -> Result<(), crate::BoxError> {
    /* parse --config/--onto/--force/<id> from args */
    let cfg = crate::config::RegistryConfig::parse(&raw)?;      // <-- only RegistryConfig touch
    let root = cfg.allowed_cwd_root...;                          //     bin-side
    let mcfg = cfg.merge.as_ref().map(|m| m.to_config()).transpose()?; // resolve -> narrow
    let outcome = merge_clone(mcfg.as_ref(), &clone, &root, onto.as_deref(), force); // -> library
    ...
}

// MOVES TO LIBRARY — takes the NARROW resolved config + primitives already.
pub fn merge_clone(
    mcfg: Option<&MergeConfig>, clone: &Path, root: &Path,
    onto: Option<&str>, force: bool,
) -> MergeOutcome { ... }               // reads only mcfg.target_ref / mcfg.author
```

`verify::run_verify` is the same story — already library-shaped, with the
side-effecting runner **injected**:

```rust
pub fn run_verify(
    cfg: &VerifyConfig,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,          // <-- effect injection already present
    max_bytes: usize,
) -> VerifyOutcome { ... }
```

**Conclusion from the evidence:** the controllers' real entry points already
take narrow resolved configs + primitives; the only `RegistryConfig` coupling
lives in the `*_cmd(args)` CLI wrappers, which are composition-root code that
stays at the bin.

## The proposed split

**Split line:** *parse/resolve stays; resolved-value-consumption moves.*

### Stays at the bin (`bin/a2a-bridge`, the composition root)

- All 19 `*Toml` DTOs; `RegistryConfig` (file root); `parse()`,
  `into_snapshot()`, all `to_config()` resolvers; `ConfigError`;
  `FileConfigSource`; every file/env/TOML IO op.
- `gate_verify_runtime()` (gates a resolved `VerifyConfig` against the
  snapshot's `allowed_cmds` — a composition-root policy concern).
- The `*_cmd(args: &[String])` CLI wrappers (`merge_cmd`, and the
  `implement`/`review`/`tweak` command entries currently inline in `main.rs`):
  parse args → load+resolve config → call the library.
- `type BoxError` (only the bin wrappers return it).

### Moves to `bridge-controller` (library)

- The controller modules: `implement, merge, review, tweak, verify, resilient,
  implement_resume, turn` (with their inline `#[cfg(test)]` unit tests).
- The **four narrow resolved config structs** as plain input contracts:
  `VerifyConfig, ReviewConfig, LoopConfig, MergeConfig`.
- The library's public API = the existing narrow entry points
  (`merge_clone`, `run_verify`, `run_tweak_loop`, `implement::decide`,
  `compose_warm_fetch`, …) plus the many already-pure helpers
  (`parse_verdict`, `parse_diff_for_depth`, `aggregate`, argv builders) that
  become unit-testable for free.

### How the two sides connect

The bin depends on `bridge-controller`. The bin's `to_config()` resolvers
**construct** library types: `impl VerifyToml { fn to_config(&self) ->
Result<bridge_controller::VerifyConfig, ConfigError> }`. Library config structs
expose `pub` fields (they already do) so the resolver can build them; no
serde/TOML crosses the boundary.

## The two decisions I want the review to pressure-test

**D1 — Home + serde-derive of the four resolved configs.**
Recommendation: co-locate them **in `bridge-controller`** (they are the
controllers' input contract, used nowhere else) and **strip their
`serde::Deserialize` derive** (they are constructed by the bin's resolvers, not
parsed — deserialize is vestigial). Rejected alternatives: (a) a separate
`bridge-config` crate — overkill for four tiny structs used by one consumer;
(b) keep them in the bin and have the library define its own `*Params` structs
with a bin-side translation layer — pure hexagon, but adds boilerplate + drift
risk for zero practical gain since the resolved configs are *already* narrow,
serde-free-in-spirit, controller-shaped contracts. Risk to check: does anything
outside the controllers deserialize these four resolved types directly (vs the
`*Toml`)? Probe says no, but the reviewer should verify the `Deserialize` is
truly vestigial.

**D2 — Effect injection boundary.**
`run_verify` already injects its `runner`. But `merge_clone`/`implement` call
`run_git`, `load_checkpoint`, `std::fs::canonicalize` **directly** inside what
becomes library code. Recommendation: **accept direct git/fs IO in the
library** — `bridge-controller` is legitimately a git-orchestration library, and
its pure decision functions (`decide`, `decide_merge`, `classify`,
`parse_verdict`) are already the unit-test surface; forcing git/fs behind
injected ports is over-abstraction at this stage. Note the seam for later. Risk
to check: does any controller reach a *global/process* effect (env, cwd,
stdout) that would make library behavior order-dependent or untestable in
parallel?

## Open questions for the reviewer

1. Is the split line correct — is there any controller path (beyond `merge_cmd`)
   that reaches `RegistryConfig` or a `*Toml` type, that the greps missed
   (macros, re-exports, trait impls)?
2. D1: strip serde from the four resolved configs and move them into
   `bridge-controller` — right call, or is a `bridge-config` crate warranted for
   a reason the probe doesn't show (e.g., serve/coordinator also consuming them
   later)?
3. D2: is accepting direct git/fs IO in the library acceptable, or does the
   testability goal demand injected ports now?
4. `ConfigError` stays at the bin, but library `to_config` construction happens
   bin-side while validation errors (`review::Depth::parse_flag`,
   `WorkflowId::parse`) originate from library types — any awkward error-type
   ownership this creates?
5. Anything about `gate_verify_runtime` / `into_snapshot` ordering that breaks
   if `VerifyConfig` is defined in the library but gated in the bin?
6. Is `turn.rs` (`TurnRunner`) correctly library-side, or is it composition
   glue that should stay?
