# ADR-001 — Language Choice: Rust

**Date:** 2026-05-30
**Status:** Accepted

---

## Context

The A2A bridge is a long-running, concurrent Rust binary that must:

- Supervise multiple child processes (ACP agents over stdio) without zombie accumulation.
- Manage many concurrent async sessions and task lifecycle state machines.
- Translate between two JSON-RPC protocols (A2A inbound, ACP outbound) with strict framing requirements.
- Present a stable, auditable seam for protocol upgrades and charter-level governance.

The analysis document (`a2a-bridge-analysis.md`, §7) evaluated three candidates — Rust, Go, TypeScript — against weighted criteria: long-term maintenance cost (25%), concurrency correctness (20%), ecosystem fit (15%), CLI subprocess ergonomics (15%), operational simplicity (10%), type system/error model (10%), development speed (5%).

Key findings from that analysis:

- The canonical ACP SDK is the `agent-client-protocol` Rust crate (`agentclientprotocol/rust-sdk`). ACP-side type safety and protocol-version negotiation are first-class in Rust; the Go and TypeScript ACP SDKs are less mature.
- Rust's borrow checker eliminates whole classes of data races in a system with N subprocess pipes read concurrently. This matters more than development velocity for a bridge whose correctness is the product.
- `tokio::process` provides the most ergonomic async subprocess API in any systems language for the ACP-side process group + kill_on_drop pattern.
- Single static binary with no GC pause concerns is a deployment requirement for a tool run as a local process alongside `kiro-cli`.
- `Result<T, E>` and exhaustive `match` make the error model unambiguous across a complex multi-protocol translation surface.

The seam-discipline companion (`seam-discipline.md`, v3 §3.2) reinforces Rust specifically because the hexagonal-port pattern requires compile-time enforcement of adapter boundaries; Rust's trait system provides this without runtime overhead.

---

## Decision

The implementation language for the A2A bridge is **Rust**, using the stable toolchain (pinned at 1.83 in `rust-toolchain.toml`).

The async runtime is `tokio` with the `full` feature set. Formatting and linting are enforced by `rustfmt` and `clippy -D warnings` in CI.

---

## Consequences

**Positive:**

- Compile-time enforcement of port seams via traits; invalid state transitions in typestate machines fail at compile time (spec §4.3, `trybuild` tests).
- Zero-zombie guarantee for subprocess management is testable (spec §3.3, success criterion S3).
- The canonical ACP SDK and `a2a-lf` (the official A2A Rust crate) are first-party or first-party-adjacent; no community-only SDK risk on the ACP side.
- Single static binary; trivial cross-compilation.

**Negative:**

- Longer time-to-first-working-version vs. Go or TypeScript. Accepted: development speed is explicitly deprioritized (5% weight in evaluation).
- Steeper onboarding curve for engineers new to Rust. Mitigated by: clear port boundaries, comprehensive doc comments, and the hexagonal architecture keeping domain logic isolated from protocol complexity.
- Build times are long on cold CI. Mitigated by `sccache` and the `actions/cache` step in `.github/workflows/ci.yml`.

**Neutral:**

- Language choice does not foreclose the Increment-3 conductor-adoption decision (ADR-002). If the conductor is adopted, it is also written in Rust and the codebase composition is straightforward.
