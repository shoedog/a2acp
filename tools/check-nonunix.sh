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
# tools/ring-stub via a `--config` override, which needs no edit to the workspace manifest.
#
# LIMITS — read before trusting a green result:
#   * This type-checks. It does not link, run, or test anything.
#   * The `ring` stub returns fixed zero bytes. Never use this target dir for anything real.
#   * It checks `bridge-core` only, matching what CI's Windows job actually compiles. If that job
#     ever widens, widen PACKAGES here to match.
#
set -euo pipefail

TARGET="${TARGET:-x86_64-pc-windows-msvc}"
PACKAGES="${PACKAGES:--p bridge-core}"

REPO_ROOT="$(git rev-parse --show-toplevel)"
STUB="${REPO_ROOT}/tools/ring-stub"

if [ ! -f "${STUB}/Cargo.toml" ]; then
  echo "error: ring stub missing at ${STUB}" >&2
  exit 1
fi

if ! rustup target list --installed 2>/dev/null | grep -qx "${TARGET}"; then
  echo "error: target ${TARGET} is not installed. Run: rustup target add ${TARGET}" >&2
  exit 1
fi

# The patch override rewrites Cargo.lock (it drops ring's real dependencies). Restore it on every
# exit path so a failed check never leaves the tree dirty.
LOCK_BACKUP="$(mktemp)"
cp "${REPO_ROOT}/Cargo.lock" "${LOCK_BACKUP}"
restore_lock() {
  cp "${LOCK_BACKUP}" "${REPO_ROOT}/Cargo.lock"
  rm -f "${LOCK_BACKUP}"
}
trap restore_lock EXIT

cd "${REPO_ROOT}"

echo "==> cargo check ${PACKAGES} --target ${TARGET} (-D warnings, ring stubbed)"
# -D warnings matters: the class shows up as dead_code on non-unix as often as E0433, and CI's
# Windows lane is warning-clean.
RUSTFLAGS="${RUSTFLAGS:-} -D warnings" \
  cargo --config "patch.crates-io.ring.path='${STUB}'" \
  check ${PACKAGES} --target "${TARGET}"

echo "==> non-unix lane OK for ${TARGET}"
