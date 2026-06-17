# ADR-0031: C2b — polyglot `serve` / `run-workflow` (per-turn ContainerRw)

**Status:** Accepted (mechanism shipped; two in-container-nav gaps deferred to the nav-hardening track)
**Date:** 2026-06-16
**Context:** LSP-MCP Slice C2, step C2b. Follows C2a (ADRs for the `cache_binding` seam + config-driven `[[languages]]` profiles + the Go implementor). Spec: `docs/superpowers/specs/2026-06-15-lsp-mcp-slice-c2-design.md` §4.

## Decision

C2b makes the **per-turn `ContainerRw` path** (the `impl` agent under `serve` / `run-workflow`, as opposed to the warm `implement` loop) polyglot, so **one serve handles mixed-language sessions** — a Go session edits + navigates Go, a Rust session edits + navigates Rust, selected per-session-cwd. Per spec §4 this needs **no loop code**: the per-turn path (`container_rw_cfg_from_entry` → `ContainerRwBackend::new`, `main.rs`) builds its config straight from the agent entry, so C2a's config-only flips already apply — the impl agent's in-container lsp is `--lang auto` and the image is the combined Rust+Go `a2a-toolchain`. So C2b is a **live gate + this ADR**, not new code.

A `c2b-nav` example workflow (a single `container_rw` `impl` node + `prompts/c2b-nav.md`, which forces a type-resolving lsp lookup) is added to `examples/a2a-bridge.containerized.toml` as the gate vehicle and regression.

## Evidence (live gate)

Run on ONE `containerized.toml` (codex impl) via `run-workflow c2b-nav --session-cwd <repo>`:
- **Go session** (`--session-cwd` a `go.mod` repo): the in-container lsp `--lang auto` detected go → gopls → returned the **type-resolved** signature `func Greet(name string) (string, bool)` @ `svc.go:4`. PASS — end-to-end polyglot serve-nav for Go.
- The detection → profile-selection → combined-image → in-container-lsp mechanism is therefore proven under the per-turn path.

## Known gaps (deferred to the in-container-nav-hardening track, NOT C2b regressions)

Both are pre-existing (they predate C2; the C2b gate merely surfaced them) and are about **which agent can navigate in-container**, not about language support:

1. **`claude`/`kiro` `container_rw` agents get no in-container MCP.** `bridge-container` (lib.rs:~228) delivers MCP **only** for `McpDelivery::CodexNative` (the `-c mcp_servers.*` override appended to the codex argv). The default `McpDelivery::Acp` (claude) / `KiroNative` paths are never wired to the inner agent's session, so a `claude-agent-acp` `container_rw` impl (e.g. the `sonnet` dogfood config) can EDIT in-container but cannot reach prism/lsp. Consequence: **in-container nav requires the codex impl today.** Fix: deliver `cfg.mcp` to the inner ACP session (`NewSessionRequest.mcp_servers`) for `Acp`-delivery container_rw agents.

2. **Rust serve-nav: rust-analyzer readiness under the per-turn path.** With the codex impl, a Rust session's agent reported "no lsp tool available," while the same config navigated Go. The lsp-mcp shim itself comes up for rust in-container (`[lsp-mcp] root=… lang=rust`, MCP `initialize` OK), so the cause is rust-analyzer **cold-index latency**: gopls serves nav near-instantly, but RA indexes slowly and the warm cargo dep cache (`/cargo`, mounted only in the warm `implement` path) is absent under the per-turn path, so RA isn't ready within the agent's MCP-handshake window. Fix candidates: a per-session prewarm/mount for serve, or a readiness wait. (The spec §4 deferred "third-party-resolving nav under serve" already; this extends that to RA readiness for workspace-only nav.)

## Consequences

- One serve handles polyglot sessions; the polyglot detection/profile/image machinery is proven (Go).
- In-container NAV is currently **codex-only** and **Go-reliable**; the two gaps above are folded into the next track (in-container nav hardening: ACP MCP delivery for claude container_rw + rust-analyzer readiness under serve — alongside the dogfood-loop hardening: cred-expiry preflight + task-via-file).
- C2c (multi-language within one cwd) remains deferred per the spec.
