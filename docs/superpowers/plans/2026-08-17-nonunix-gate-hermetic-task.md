# A hermetic non-unix gate — withdrawn attempt, and what a sound one needs

Status: **WITHDRAWN, not dispatched.** The first attempt was written, reviewed
three times, and pulled. This records why, so the next attempt starts from the
finding rather than repeating it.

## The problem the gate exists to solve (still unsolved)

`crates/bridge-core` gates `liveness` and `namespace_transaction` behind
`#[cfg(unix)]` while `fs_custody` is unconditional. A new `fs_custody` helper
that reaches into them type-checks on a developer's unix machine and fails CI's
Windows lane with `E0433`, or with a non-unix `dead_code` warning under
`-D warnings`. CI compiles `bridge-core` for Windows via `bridge-store`; every
local gate — fmt, `clippy --workspace --all-targets`, the 90-target suite — is
unix-only. The class has cost a landing round **five times**: 3a, 3b1, 3c1, 3c2,
3d-T2.

## Why the attempt was withdrawn

Three counted review rounds produced **eight valid findings**, and by the third
round the pattern was the finding: two consecutive rounds surfaced *different
instances of the same class* — ambient state silently changing what the gate
checks, or whether it reports success. The third round was asked to enumerate
the class rather than re-check the fixes, and it did not close:

- **Cargo profile.** `CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=false` plus a reference
  under `#[cfg(all(not(unix), debug_assertions))]` — local check excludes it and
  prints success; CI's Windows test profile includes it and fails `E0433`.
- **Compiler substitution.** `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`,
  `RUSTC`, and their `CARGO_BUILD_*` equivalents can append `--cap-lints allow`,
  inject cfgs, or select a different compiler *after* the gate's own flags.
- **Toolchain selection.** `RUSTUP_TOOLCHAIN` overrides `rust-toolchain.toml`,
  and `PATH` can select a non-rustup cargo, so the compiler need not match CI's
  pinned 1.94.0.
- **Config discovery.** Cargo reads `.cargo/config{,.toml}` from the copied
  workspace *and its temporary-directory ancestors*, plus `$CARGO_HOME` and
  `$HOME`. A `--config` override for the `ring` patch does not disable profile,
  wrapper, compiler, `[env]`, source, patch, or unstable settings from those.

Only `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS` and `CARGO_TARGET_DIR` were ever
owned. Everything else was inherited.

**The withdrawal was pre-committed.** Before the third review was dispatched, the
operator stated that if the class audit came back open the script would be
withdrawn rather than patched again — the same rule applied to the path-identity
defect earlier the same day (stop patching at the fifth instance, design the
primitive). Applying it to production code and exempting one's own tooling
would not have been consistent.

**A false-green gate is worse than no gate**, because it manufactures confidence
in exactly the situation it was built to catch. That is what made this a blocker
rather than a smell.

## What a sound implementation requires

1. **A hermetic environment**, not an inherited one: `env -i` with an explicit
   allowlist, rather than unsetting known-bad variables one at a time. The
   findings arrived one variable per round precisely because the design enumerated
   villains instead of admitting only known-good inputs.
2. **Pinned toolchain, verified**: invoke through `rustup run <pinned>` and
   assert the resolved `rustc -vV` matches CI's pin before compiling anything.
3. **Controlled config discovery**: a `CARGO_HOME` the gate owns, a workspace
   copy whose ancestors cannot contribute config, and explicit rejection of any
   discovered `[env]`/wrapper/profile override.
4. **Profile parity with CI**, so `debug_assertions`-conditional code is
   compiled the same way the Windows job compiles it.
5. **Refuse rather than proceed** whenever an input cannot be controlled — and
   never print a success banner on a run whose inputs were not verified.
6. **A hostile-environment regression battery** as an acceptance criterion:
   profile, wrapper, global-config, `RUSTC_BOOTSTRAP`, toolchain and `PATH`
   cases, each against a real non-unix compile failure, each required to refuse
   before compilation or fail without a success banner.

## The `ring` obstacle, and the preserved recipe

`cargo check --target x86_64-pc-windows-msvc` cannot run from macOS because
`ring`'s C build script will not cross-compile. A signature-only stub patched in
via `--config` works; making it *clean* is the fiddly part and it was rebuilt
from scratch twice before being preserved. The working recipe is committed as
`docs/superpowers/reviews/2026-08-16-ring-stub-probe.rs.txt` and
`…-probe.Cargo.toml.txt` (`.txt` so cargo can never pick it up). It needs:

- the feature names dependents request — `quinn-proto` asks for
  `wasm32_unknown_unknown_js`, and a missing name fails resolution before any
  type-checking happens;
- `ring::hmac` (`Key`, `sign`, `Tag`, `HMAC_SHA256`) alongside `digest`/`rand`;
- `#[derive(Clone, Copy)]` on the hmac algorithm, or `E0507`.

A clean probe yields exactly one error — the real one. An unclean probe mixes
stub artifacts with the defect and cannot serve as a control.

## Interim posture

None. There is no local gate; `CONTRIBUTING.md` now says so plainly rather than
promising one. Anyone touching `crates/bridge-core` should expect the Windows
lane to be the first thing that tells them, and the
`#[cfg(unix)]` / `#[cfg_attr(not(unix), allow(dead_code))]` pattern from
`790b4191` is the established fix shape.
