# a2a-bridge

A single Rust binary that exposes a local CLI coding agent (**Kiro**) as an
**A2A**-compliant network service. Remote A2A callers send tasks; the bridge drives the
CLI agent over **ACP** (JSON-RPC/stdio) and streams results back over SSE.

This is the **v1 "walking skeleton"** — the inbound path works end-to-end; the outbound
delegation seam is defined but stubbed (concrete impl is Increment 2.5). See
`docs/superpowers/specs/2026-05-29-a2a-bridge-v1-design.md` for the design and
`docs/superpowers/plans/2026-05-30-a2a-bridge-v1.md` for the implementation plan.

## Architecture

Hexagonal / ports-and-adapters. The domain-only `bridge-core` (Task/Session typestate,
all port traits, the translator) is driven by adapters; nothing in the core depends on an
adapter.

```
A2A caller ──HTTP/JSON-RPC/SSE──▶ bridge-a2a-inbound (axum)
                                      │  auth → route → translate → backend
                                      ▼
                                  bridge-core (Translator, typestate, ports)
                                      │  AgentBackend (streaming)
                                      ▼
                                  bridge-acp ──ACP/NDJSON/stdio──▶ kiro-cli acp
```

| Crate | Responsibility |
|-------|----------------|
| `bridge-core` | Domain types, `Task`/`Session` typestate, all port traits, the streaming translator (coalescer + anti-corruption rules) |
| `bridge-acp` | NDJSON frame reader, process supervisor (group-kill), Kiro `AgentBackend`, replay backend |
| `bridge-a2a-inbound` | A2A v1 JSON-RPC server (axum), Agent Card, SSE |
| `bridge-a2a-outbound` | `DelegationPort` stub (Increment 2.5) |
| `bridge-store` | SQLite `SessionStore` (task↔session + pending-request) |
| `bridge-policy` | `AutoPolicy` + `AlwaysGrant` auth (invoked seams) |
| `bridge-observ` | `tracing` setup + correlated task spans |
| `bin/a2a-bridge` | Composition root: config, routing, `main` |

## Protocol bindings

- **A2A v1** via the official `a2a` crate (package `a2a-lf` =0.3.0). Methods: `SendMessage`,
  `SendStreamingMessage`, `GetTask`, `CancelTask`, `SubscribeToTask`. Version pinned to
  `a2a::VERSION = "1.0"`; header `A2A-Version`.
- **ACP** via `agent-client-protocol` =0.12.1 over NDJSON/stdio.

Both SDK versions are pinned (`Cargo.lock` committed) and maintained per the
dependency-currency policy in the spec (§11.2).

## Build & run

Requires the pinned toolchain (`rust-toolchain.toml`, Rust 1.94.0).

```bash
cargo build --release
```

Create `a2a-bridge.toml` (or rely on the built-in default):

```toml
[agent]
name = "kiro"
cmd  = "kiro-cli"
args = ["acp"]

[server]
addr = "127.0.0.1:8080"
```

```bash
./target/release/a2a-bridge          # spawns `kiro-cli acp`, serves A2A on addr
```

The Agent Card is published at `GET /.well-known/agent-card.json`. `kiro-cli` must be
installed and authenticated on the host (`kiro-cli whoami`).

## Testing & coverage

```bash
cargo test --workspace                # ~68 tests, all in-process (no external agent)
```

Coverage is gated in CI (`cargo-llvm-cov`), enforced as a floor, measured per crate:

| Scope | Gate | Current |
|-------|------|---------|
| Workspace | ≥ 85% lines | ~93% |
| `bridge-core` (domain/typestate/translator) | ≥ 90% lines | ~95% |
| `bridge-acp` (parse boundary, supervisor) | ≥ 90% lines | ~93% |

```bash
cargo llvm-cov --workspace --fail-under-lines 85
cargo llvm-cov -p bridge-core --fail-under-lines 90
cargo llvm-cov -p bridge-acp  --fail-under-lines 90
```

Typestate invariants (e.g. prompting a non-ready session, resuming a terminal task) are
proven uncompilable by `trybuild` compile-fail tests.

### Gated real-agent smoke

A real end-to-end round-trip against an authenticated `kiro-cli` is `#[ignore]`-gated
(not in default CI):

```bash
cargo test -p a2a-bridge --test e2e_kiro -- --ignored --nocapture   # needs kiro-cli whoami
```

## What v1 does / doesn't do

**In:** inbound A2A (Kiro), streaming SSE with coalescing, cancellation
(prompt-result semantics), permission/auth suspend→resume, process-group reaping, the
`DelegationPort` seam, structured tracing.

**Deferred:** concrete outbound delegation + SSE stream-merge (Increment 2.5);
multi-agent adapters (Claude Code/Codex/Gemini, Increment 3); real permission policy;
`session/load` resume; MCP-over-ACP; JWT/mTLS enforcement; container isolation.

## License

Apache-2.0.
