# Slice B — offline-completeness proof (Task 1)

**Date:** 2026-06-14 · **Verdict: READ-ONLY cache works.**

Proves the egress-locked impl container's rust-analyzer can index a deps-heavy repo (a2a-bridge) fully offline from a bridge-managed cache, and decides read-only vs writable for the plan's Tasks 5–6.

## Method

1. Throwaway image `a2a-ra-spike` = `FROM a2a-toolchain:latest` + `rustup component add rust-analyzer`.
2. **Warm** a dedicated `CARGO_HOME` via a no-creds fetch through the verify egress:
   `docker run --network a2a-verify-egress -e HTTPS_PROXY=…verify-proxy:8888 -e CARGO_HOME=/cargo -v clone:/work -v cache:/cargo … cargo fetch --locked` → 644 MB, exit 0.
3. **Index offline, cache read-only, no egress, no host cache:**
   `docker run --network a2a-egress-internal -e CARGO_HOME=/cargo -e CARGO_NET_OFFLINE=true -e CARGO_TARGET_DIR=/target -v clone:/work -v cache:/cargo:ro -v target:/target … rust-analyzer analysis-stats .`

## Result

- **`Database loaded: 4.14s`, 33 crates, ~99 MB metadata** — full dependency graph resolved.
- **0 dep-resolution failures** (`failed to resolve` / `unresolved import` / `can't find crate` / `registry not found` = 0). The `:ro` cache is sufficient for deps; cargo did **not** need a writable `CARGO_HOME` for registry source consumption.
- Peak RSS ≈ 3 GB (consistent with the in-container RA spike).

## Decisions for the plan

1. **Mount the impl-lsp dep cache READ-ONLY** (`-v <cache>:/cargo:ro`). No writable cache → no poisoning vector at all; the spec's "writable fallback" path is not needed.
2. **The toolchain image needs `rust-src` too** — RA logged `ERROR can't load standard library, try installing rust-src`. Add `--component rust-src` alongside `--component rust-analyzer` in Task 2, or `std`/`core` types won't resolve (degraded nav on `Vec`/`Option`/etc.).
3. Benign: `Failed to create perf counter: Operation not permitted` (×9) is a container-capability limitation in RA's timing, not an indexing error — ignore.
4. Env that worked: `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR=/target` (the last is set by lsp-mcp's `--target-cache`).

## Not separately tested (note for the live gate)

This repo's deps are all crates.io (no git deps), so the git-dep path (`~/.cargo/git`) wasn't exercised. If a target repo has git deps, the warm `cargo fetch --locked` should still populate them under the same `CARGO_HOME`; confirm at the live gate on such a repo.
