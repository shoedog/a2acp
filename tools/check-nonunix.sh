#!/usr/bin/env bash
#
# Local pre-CI gate for the NON-UNIX lane.
#
# Why this exists: `bridge-core` gates `liveness` and `namespace_transaction` behind
# `#[cfg(unix)]` while `fs_custody` is unconditional, so any new `fs_custody` helper that reaches
# into those modules compiles fine on unix and fails with E0433 on Windows. `bridge-store` depends
# on `bridge-core`, so CI's Windows job compiles it — but every LOCAL gate (fmt, clippy
# --all-targets, the 90-target suite) runs unix-only, which made this class structurally invisible
# until CI. It has cost a landing round in 3a, 3b1, 3c1, 3c2 and 3d-T2.
#
# The obstacle to just running `cargo check --target x86_64-pc-windows-msvc` is `ring`: its C build
# script cannot cross-compile from macOS. So this script patches in the signature-only stub at
# tools/ring-stub via a `--config` override.
#
# ISOLATION: the override makes Cargo rewrite `Cargo.lock`. An earlier version backed the file up
# and restored it in place, which loses lockfile custody — two concurrent runs can interleave
# backup/restore and leave the stub lock behind, a concurrent legitimate edit is clobbered, and a
# SIGKILL bypasses the trap entirely. This version instead copies the workspace into a throwaway
# directory and runs there, so the repository's own lockfile is never touched.
#
# LIMITS — read before trusting a green result:
#   * This type-checks. It does not link, run, or test anything.
#   * The `ring` stub returns fixed zero bytes. Never use its target dir for anything real.
#   * It checks `bridge-core` only, matching what CI's Windows job actually compiles. If that job
#     ever widens, widen PACKAGES below to match.
#
set -euo pipefail

# Hard-coded, NOT read from the environment. These were once `${TARGET:-...}` / `${PACKAGES:-...}`,
# which meant an exported `TARGET=x86_64-apple-darwin` from unrelated cross-build work would make
# this gate check a UNIX target — where the very reference it exists to catch is legal — and then
# print "non-unix lane OK". A gate whose contract can be silently redirected by ambient state is
# worse than no gate, because it manufactures false confidence.
readonly TARGET="x86_64-pc-windows-msvc"
readonly PACKAGES=(-p bridge-core)

REPO_ROOT="$(git rev-parse --show-toplevel)"
readonly REPO_ROOT

if [ ! -f "${REPO_ROOT}/tools/ring-stub/Cargo.toml" ]; then
  echo "error: ring stub missing at ${REPO_ROOT}/tools/ring-stub" >&2
  exit 1
fi

if ! rustup target list --installed 2>/dev/null | grep -qx "${TARGET}"; then
  echo "error: target ${TARGET} is not installed. Run: rustup target add ${TARGET}" >&2
  exit 1
fi

WORK="$(mktemp -d)"
readonly WORK
cleanup() { rm -rf "${WORK}"; }
trap cleanup EXIT INT TERM HUP

# Copy the workspace source, INCLUDING uncommitted changes (this is a pre-commit gate, so checking
# only committed state would miss exactly the edit being gated). Exclude the build and VCS
# directories, which are large and irrelevant to a type-check.
echo "==> staging an isolated workspace copy"
tar -C "${REPO_ROOT}" \
  --exclude=./target \
  --exclude=./.git \
  --exclude=./.claude \
  -cf - . | tar -C "${WORK}" -xf -

echo "==> cargo check ${PACKAGES[*]} --target ${TARGET} (-D warnings, ring stubbed, isolated)"
# -D warnings matters: the class shows up as dead_code on non-unix as often as E0433, and CI's
# Windows lane is warning-clean.
(
  cd "${WORK}"
  RUSTFLAGS="${RUSTFLAGS:-} -D warnings" \
  CARGO_TARGET_DIR="${WORK}/target" \
    cargo --config "patch.crates-io.ring.path='${WORK}/tools/ring-stub'" \
    check "${PACKAGES[@]}" --target "${TARGET}"
)

echo "==> non-unix lane OK for ${TARGET}"
