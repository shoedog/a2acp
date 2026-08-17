I’ll review only `a3e10e3d..589bbf77` and the named in-repo artifacts, with no builds or tests. I’m using the Prism and LSP navigation skills because the contract specifically requires structural and type-resolved caller inventories; I’ll first freeze commit/diff identity, then read every requested artifact line-by-line, trace production causality and tests, and close the ambient-state class explicitly.The navigation instructions confirm a targeted approach: query only changed symbols, verify structural edges by reading each site, and treat “no callers” as repo-local evidence. I’m now freezing the two commit objects and enumerating the exact delta before judging any of the four claimed repairs.The committed delta is exactly the three declared files and the checkout is clean at `589bbf771d6d425b3a4a7844a6106bebcf230271`. Neither configured navigation server is exposed in this session, so I’m following the mandated fallback: literal inventories plus direct reads of every matched caller and test site.The four repair mechanisms themselves are now localized. I’m expanding the timeout hook through all three blocking gates and every arming/release test, because instance-local storage fixes cross-test consumption only if the same instance remains coupled to the gated work and its owning test through final drop.Three of the claimed repairs are mechanically closed at their stated scope: hook timeout state is instance-local across all three gates, signal handlers cannot fall through to the success banner, and the document now makes only the necessary-condition inference. The exact `RUSTFLAGS` repair also closes those two named variables, but the broader ambient-state class is still open; I’m distinguishing concrete false-green channels from variables that can only cause noise or a false red before adjudicating severity.## WRONG findings

1. **WRONG — BLOCKER — the ambient-state class remains open.**

   [check-nonunix.sh:82](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/tools/check-nonunix.sh:82) inherits the complete environment and Cargo configuration graph; only `CARGO_ENCODED_RUSTFLAGS`, `RUSTFLAGS`, and `CARGO_TARGET_DIR` are owned.

   Constructible false green: export `CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=false`, then add an invalid `crate::liveness` reference under `#[cfg(all(not(unix), debug_assertions))]`. The local dev-profile check excludes it and prints success; CI’s Windows test profile includes it and fails E0433. Two other independent channels remain:

   - `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `RUSTC`, or their `CARGO_BUILD_*` equivalents can append `--cap-lints allow`, inject cfgs, or select another compiler after the gate-owned flags.
   - `RUSTUP_TOOLCHAIN` overrides [rust-toolchain.toml:2](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/rust-toolchain.toml:2); `PATH` can select a non-rustup Cargo. The resulting compiler need not match CI’s pinned 1.94.0 at [.github/workflows/ci.yml:146](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/.github/workflows/ci.yml:146).

   Cargo also discovers `.cargo/config{,.toml}` in the copied workspace and its temporary-directory ancestors, plus `$CARGO_HOME/config{,.toml}` or `$HOME/.cargo/config{,.toml}`. The line-87 `--config` overrides only the `ring` patch; it does not disable profile, wrapper, compiler, `[env]`, source, patch, or unstable settings from those files.

   - Trigger/likelihood: developer shells using toolchain overrides, compiler wrappers, profile overrides, or global Cargo config; **plausible**, because this standalone script has no sanitized caller.
   - Exposure/impact: local verification users receive false-green evidence and still lose the Windows CI round; moderate tooling/evidence severity, no production runtime exposure.
   - Fix/cost: withdraw the success-producing gate now, or perform a medium tooling rewrite with an allowlisted environment, exact toolchain/compiler verification, controlled Cargo-home/config discovery, and explicit rejection of semantic overrides.
   - Red regression: exercise hostile profile, wrapper, global-config, `RUSTC_BOOTSTRAP`, toolchain, and `PATH` cases against real non-Unix compile failures; every case must either refuse before compilation or fail without a success banner.
   - Decision: **BLOCKER**. The acceptance criterion expressly makes landing contingent on closing this class. It is not closed.

2. **WRONG — DEFER — signalling only the script still does not cancel foreground Cargo.**

   At [check-nonunix.sh:59](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/tools/check-nonunix.sh:59), Bash defers a trapped signal while waiting for the foreground command at line 87. If Cargo hangs and an operator sends `TERM` only to the script PID, the handler cannot exit 143 until Cargo returns. A watchdog that subsequently SIGKILLs the shell can leave both the Cargo child and temporary workspace behind.

   - Trigger/likelihood: PID-only cancellation during a long or hung Cargo invocation; **rare**, because terminal interrupts and many supervisors signal the process group.
   - Exposure/impact: operators can observe failed cancellation, orphan work, and lost cleanup; no false success once the handler actually runs.
   - Fix/cost: supervise Cargo as an explicit child, forward the signal, wait for it, then exit with the conventional status. Small shell change, small-to-medium harness work.
   - Red regression: block a fake Cargo child, signal only the wrapper PID, and require bounded child termination, status 143, no banner, and cleanup; include an ignoring-child edge case.
   - Decision: **DEFER**. The original fall-through false green is fixed, while this narrower cancellation defect has lower risk.

## SMELL findings

1. **SMELL — DEFER — none of the four repairs has a committed fail-first regression.**

   The delta adds no test. There is no short-bound owner/sibling timeout test, hostile Cargo-environment harness, signal harness, or encoded-flags edge case. The supplied operator probes corroborate the repairs but are not durable regressions; the reported full suite exercises normal paths.

   - Trigger/likelihood: later test-hook or script refactoring; **plausible**.
   - Exposure/impact: maintainers can reintroduce incorrect verification evidence.
   - Fix/cost: add an injectable short timeout with owner/sibling and spurious-wake cases, plus a fake-tool shell harness covering environment isolation, all three signals, cleanup, and the success path. Small-to-medium test-only blast radius.
   - Decision: **DEFER** because this is a regression-evidence gap, not another demonstrated present false result.

## Evidence assessment

The four requested dispositions are:

1. **FIXED:** timeout state is instance-local at [backend.rs:1009](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/crates/bridge-worktree/src/backend.rs:1009), stored at line 1061, consumed from that instance at lines 1037–1053, and passed by all three gates at lines 1131, 1171, and 1241.
2. **FIXED narrowly:** exact `RUSTFLAGS` and clearing `CARGO_ENCODED_RUSTFLAGS` at [check-nonunix.sh:82](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/tools/check-nonunix.sh:82) close those two named channels. The broader ambient-state acceptance criterion is **NOT-FIXED**.
3. **FIXED for false-success attribution:** lines 59–62 ensure a handled `INT`/`TERM`/`HUP` cannot fall through to line 91’s banner. The deferred cancellation defect is separately reported above.
4. **FIXED:** [coverage-lane-flake-family-investigation.md:108](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/docs/superpowers/reviews/2026-08-17-coverage-lane-flake-family-investigation.md:108) now states only that plain-lane recurrence disproves instrumentation as a necessary condition and requires assertion plus same-environment controls before attribution.

Ambient inventory:

- Explicit CLI target/package and exact `RUSTFLAGS` neutralize `CARGO_BUILD_TARGET`, `CARGO_BUILD_RUSTFLAGS`, `CARGO_TARGET_*_RUSTFLAGS`, and corresponding Cargo-config rustflags. The fresh explicit `CARGO_TARGET_DIR` prevents inherited target-directory or incremental artifacts from supplying a green.
- Still semantic and inherited: `CARGO_PROFILE_DEV_DEBUG_ASSERTIONS`, `CARGO_PROFILE_DEV_PANIC`, other `CARGO_PROFILE_DEV_*`; `RUSTC`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `RUSTC_BOOTSTRAP`; `CARGO_BUILD_RUSTC*`; `RUSTUP_TOOLCHAIN`, `RUSTUP_HOME`; `CARGO_HOME`, `HOME`, `TMPDIR`, and all discovered Cargo config files; `PATH`; repository-selection inputs `GIT_DIR`, `GIT_WORK_TREE`, and invocation cwd; shell/tool inputs such as `BASH_ENV`, exported functions, and `TAR_OPTIONS`.
- Primarily availability, resource, or output channels: `CARGO_NET_*`, `CARGO_HTTP_*`, `CARGO_REGISTRIES_*`, proxy/credential settings, `CARGO_BUILD_JOBS`, non-semantic profile/codegen fields, linker/runner settings, `CARGO_TERM_*`, `CARGO_LOG`, `RUST_BACKTRACE`, `RUST_LOG`, `RUSTC_LOG`, `RUSTDOC*`, and `RUST_TEST_*`. These can still change completion or diagnostics, but do not independently establish another current false green without a substituted compiler/tool/source mechanism already counted above.

I read the complete delta and both historical review artifacts. The checkout was clean at exact head `589bbf771d6d425b3a4a7844a6106bebcf230271`. The Rust change is entirely `#[cfg(test)]`; the shell script has no checked-in caller; the documentation has no executable or served projection. Prism/LSP navigation was unavailable, so the caller inventory used direct source search. No build, test, script, provider, network, or write operation was performed.

VERDICT: REJECT
SUMMARY: The four narrow repairs work, but inherited Cargo, compiler, toolchain, PATH, profile, and config inputs leave the expressly gating false-green class open; signal cancellation and missing durable regressions are deferred.