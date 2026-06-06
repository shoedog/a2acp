# Containerized Agents — Slice B2b-2 Design: the build+test verify step

**Date:** 2026-06-05
**Status:** Draft (pre review)
**Builds on:** B2b-1 (the `implement` clone+edit+commit subcommand, merged 6bca5df, ADR-0019). B2b-3
(review-the-diff lenses + APPROVE/REJECT + the review→tweak loop) follows.

## Goal

Add a **trustworthy, bridge-run build+test verify** to the `implement` loop: after the commit, the bridge
runs a configured verify suite (Rust default: fmt/clippy/build/test + coverage) in a toolchain container on
the quarantine clone, reusing a persistent build cache, and **reports the verdict** in the operator
hand-off. The impl agent moves to a toolchain image so it (latently in B2b-2; actively in B2b-3) can build
and self-fix. CI stays the authoritative post-merge gate.

## Decisions locked (from the design dialogue)

1. **Bridge-deterministic verify, after the commit** (`edit → commit → verify → hand-off-with-verdict`). The
   bridge runs `cargo …` (not the agent) so the verdict is **trustworthy** (the agent can't claim "tests
   pass" falsely). It REPORTS the verdict; it does NOT block the commit (the clone is quarantined; the
   review→tweak *loop* is B2b-3). NOT a workflow node (it's a deterministic command run, not an agent turn).
2. **The impl agent moves to the `a2a-toolchain` image** (= the reader image + Rust) so the agent has BOTH
   `claude-agent-acp` AND `cargo` — latent capability in B2b-2; B2b-3's loop uses it to self-fix a failing
   verify. (The `:ro` readers stay on the reader image.)
3. **Config-driven `[verify]` block** (image + commands + egress) — the *mechanism* is language-agnostic
   (npm/pip/go later = config, not code); B2b-2 ships + validates the **Rust instance**.
4. **A persistent build cache is the speedup — NOT parallelism.** clippy/build/test compile the same crate
   graph and lock the same `target/`, so running them concurrently serializes (or forces N cold builds);
   the win is a **warm cache** (CARGO_HOME + CARGO_TARGET_DIR persistent volumes) reused across runs. So the
   "pre-merge verify + post-merge CI" duplication is one cheap *cached* pre-merge pass + CI's own run, not
   two cold builds. (fmt parallelizes — no build — but the suite is dominated by the shared compile.)
5. **cargo-under-egress-lockdown:** the verify container runs on the locked egress net + proxy with the
   package registries allowlisted (`crates.io`/`index.crates.io`/`static.crates.io`/`github` for B2b-2;
   per-language registries later). Supports real dep installs. Trade: a small malicious-`build.rs` exfil
   surface, bounded by allowlisting only registries + containment + the operator reviewing the diff.
6. **Clone mounted `:ro` for verify** + `--locked`: cargo writes only to `CARGO_TARGET_DIR`/`CARGO_HOME`
   (the cache volumes), never the committed tree; `--locked` makes a lock-changing build a verify *failure*,
   not a silent `:ro` write error.

## Architecture

### The `a2a-toolchain` image
`deploy/containers/toolchain.Containerfile`: `FROM a2a-agent-reader:latest` + the Rust toolchain (rustup or
the official rust layer) + `rustup component add clippy rustfmt` + `cargo install cargo-llvm-cov
cargo-tarpaulin`. So both the impl agent (ACP CLI + cargo) and the verify run (cargo + the tools) use one
image, and every default command's tool is present (no "command not found" annoyance).

### The `[verify]` config block
```toml
[verify]
image    = "a2a-toolchain:latest"
egress   = "locked"
network  = "a2a-egress-internal"
proxy    = "http://a2a-egress-proxy:8888"
cache    = "a2a-verify-cache"   # persistent volume: CARGO_HOME=/cache/cargo, CARGO_TARGET_DIR=/cache/target
# Rust default (config-driven). Each is a GATE except the coverage line (reported).
commands = [
  "cargo fmt --all -- --check",
  "cargo clippy --all-targets --all-features -- -D warnings",
  "cargo build --locked",
  "cargo test --locked",
  "cargo llvm-cov --workspace --summary-only",   # reported (exit 0); add --fail-under-lines to gate
]
```
`tarpaulin` ships in the image as the alternative coverage tool (switch via `commands`). The block is
optional — absent `[verify]` → `implement` skips verify (B2b-1 behavior) + notes it.

### The verify run (bridge-deterministic, cached)
After the commit, the `implement` subcommand runs ONE container:
```
docker run --rm --network a2a-egress-internal -e HTTPS_PROXY=<proxy> \
  -v <clone>:<clone>:ro -w <clone> \
  -v a2a-verify-cache:/cache -e CARGO_HOME=/cache/cargo -e CARGO_TARGET_DIR=/cache/target \
  a2a-toolchain:latest  sh -c '<script>'
```
The `<script>` runs the configured `commands` **sequentially** (so clippy→build→test reuse the warm
`/cache/target`), emitting a `=== VERIFY <i> ===` marker before each, capturing each command's exit; it
stops at the first GATE failure (coverage is reported, never gates by default). The bridge parses the output
into a `VerifyVerdict { results: Vec<(cmd, ok)>, passed: bool, output, failed_at: Option<usize> }`.
- **Cache volumes** (`a2a-verify-cache`: `/cache/cargo` = CARGO_HOME registry+git+locks; `/cache/target` =
  build artifacts) persist across `implement` runs → first run cold, the rest incremental. (Per-repo target
  keying for multi-repo is a follow-on; B2b-2 uses one named volume — the dogfood is one repo.)
- **Network:** locked + proxy so uncached deps fetch via the allowlisted registries; cached deps reuse the
  volume.

### Integration into `implement_cmd`
The `Action::Commit` arm (B2b-1), after the host commit + before the hand-off: if `[verify]` is configured,
run the verify, then include the **verdict** in the hand-off output:
```
implement: committed <sha> "<subj>" on implement/<id>
verify: PASS  (fmt ✓ · clippy ✓ · build ✓ · test ✓ · coverage 90.0%)
   -- or --
verify: FAIL at `cargo clippy …`  (fmt ✓ · clippy ✗)   [see output below]
   <failing command output>
clone: <path>
<operator re-author/merge/reap commands>
```
Verify is a separate `bin/a2a-bridge/src/verify.rs` module: the pure verify-script builder + the
`VerifyVerdict` parser/aggregation (Docker-free unit tests); the container run is the impure step (the live
gate). `implement` always commits + hands off; the verdict is informational (the operator decides; B2b-3
adds the agent self-fix loop).

## cargo-under-lockdown (operator infra)
Add the registries to `deploy/containers/tinyproxy.filter` (anchored ERE) + rebuild the proxy:
```
(^|\.)crates\.io$
(^|\.)static\.crates\.io$
(^|\.)index\.crates\.io$
(^|\.)github\.com$
(^|\.)codeload\.github\.com$
```
The verify container uses the locked net + proxy (same posture as the `:ro` readers). The cache volume means
deps fetch once.

## Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `a2a-toolchain` image | `deploy/containers/toolchain.Containerfile` | reader + Rust + clippy/fmt + llvm-cov + tarpaulin |
| `[verify]` TOML mirror + parse | `bin/a2a-bridge/src/config.rs` | `VerifyConfig { image, commands, egress/network/proxy, cache }`, `#[serde(default)]` |
| **pure** verify-script builder + `VerifyVerdict` parse/aggregate | `bin/a2a-bridge/src/verify.rs` (new) | Docker-free unit tests |
| the verify container run (argv + capture) | `verify.rs` | temp/Docker — live-gated |
| integration (run verify after commit, verdict in hand-off) | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) | the `Action::Commit` arm |
| `impl` agent image → toolchain; registries allowlist | `examples/a2a-bridge.containerized.toml`, `deploy/containers/tinyproxy.filter` | config/infra |

## Testing
- **Unit (Docker-free) in `verify.rs`:** the script builder (the `=== VERIFY <i> ===` markers, the
  command sequence, `sh -c` shape, the cache env); the `VerifyVerdict` parser (all-pass; fail-at-N with the
  failing command + output; the coverage line reported-not-gated; gate-vs-report classification); the
  hand-off verdict line (PASS/FAIL summary).
- **Unit:** `[verify]` config parse (present/absent → Some/None; commands list; egress mirror).
- **Live gate (Docker, operator-run):** `implement` a small change to a throwaway clone of *this* repo with
  `[verify]` configured → the verdict shows ✅ per command (with the toolchain image built + registries
  allowlisted + the cache warm on the 2nd run); inject a clippy/test failure → the verdict shows ❌ at that
  command + the output; `docker events` containment. Confirm the clone is `:ro` (verify can't mutate it).
- Coverage after `cargo llvm-cov clean --workspace` (floors workspace 85, bridge-core 90, bridge-workflow 90).

## Deferred
- **B2b-3:** the review→tweak loop (on a failing verify, re-prompt the agent to fix — using its toolchain
  image), review-the-diff lenses + synth + APPROVE/REJECT.
- Other-language `[verify]` configs (npm/pip/go images + commands + registry allowlists); a coverage-floor
  gate (`--fail-under-lines`); per-repo cache-volume keying for multi-repo; a `--no-verify` implement flag.

## Firewall
Designed from the bridge's own seams (the `implement` subcommand `Action::Commit` arm, the
`compose_sandbox`/egress posture, the reader image + the egress proxy infra). Dual review = containerized
dogfood (the B1-hardened `[sandbox]` agents) PRIMARY + a2a-local `codex-review` (gpt-5.5) backstop. Once
B2b-3 lands, `implement` self-fixes against its own verify — the ultimate dogfood.
