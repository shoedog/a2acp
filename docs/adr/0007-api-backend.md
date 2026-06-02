# ADR-0007 — Vendor-neutral OpenAI-compatible API backend (`kind="api"`)

**Date:** 2026-06-01
**Status:** Accepted

**Relates to:** ADR-0002→0005 (the deferred fork-conductor-vs-greenfield decision) and ADR-0006 (which left the bridge ACP-only after retiring `bridge-claude`).

---

## Context

After ADR-0006 retired `bridge-claude`, the bridge was **ACP-only** — a single backend kind, all local-process (the bridge spawns a stdio child and supervises it). The parked **conductor** decision (fork the existing conductor codebase vs. continue greenfield) was waiting on one piece of evidence: **do the ports absorb a NON-process backend cleanly?** The originally-scoped answer was "B1" — a true HTTP Anthropic-Messages-API backend — but that needs a paid `ANTHROPIC_API_KEY`.

A cheaper, vendor-neutral path gives the same evidence at **$0**: a non-process backend that speaks the **OpenAI-compatible** HTTP API (`POST {base_url}/chat/completions`), validated live against a local **Ollama** tool-capable model (`qwen3.5:9b`). It does not have to be Claude, or paid — the conductor cares about the *port shape*, not the vendor.

## Decision

**Add `crates/bridge-api`: an `ApiBackend` implementing `bridge_core::ports::AgentBackend` over `reqwest`, registered as a new `kind="api"` entry.** It owns **no child process** — it holds a `base_url` + a `reqwest::Client`. The whole prompt turn runs inside `prompt()` and yields **only `Update::Text` and `Update::Done`** (never `Update::Permission`).

### Surface A — lifecycle/transport (the exec-centric ripple)
A process backend has a `cmd`; a non-process backend has a `base_url`. Making that honest forced (one atomic commit, ~10 sites):
- `AgentEntry.cmd: String → Option<String>`; new typed `base_url`/`api_key_env` fields.
- `registry::validate` became **kind-aware**: `Acp` requires `cmd ∈ allowed_cmds`; `Api` requires `base_url`, **forbids `cmd`**, and is **not** subject to the `allowed_cmds` exec-allowlist (a non-process backend has no command to allow). The same invariant is enforced at config-parse time (`into_snapshot`) for a friendlier boot error.
- The `main.rs` factory gained a second arm building `ApiBackend` (no `Supervised` child, no cwd); `AgentKind` re-expanded to `{ #[default] Acp, Api }`.

The ripple was **bounded and mechanical** — the dual-reviewed plan enumerated the sites, and the compiler confirmed completeness. No existing ACP behavior changed.

### Surface B — permission/policy (the silent decision)
OpenAI function-calling is a **structurally different** permission model from ACP: client-side. The model emits `tool_calls`; the *client* decides and feeds back a result — there is no agent blocking over a wire. The API backend routes each `tool_call` through the **same `PolicyEngine` port** `AcpBackend::decide_permission` uses, **internally and silently** (Approve → run the stub tool; `Err(PermissionDenied)` → denial tool-result; abstain → refusal tool-result), and **does not** emit `Update::Permission`.

**Why silent — the decisive design correction (folded from dual review).** An earlier draft emitted `Update::Permission(interactive:false)` as an "observable signal." That is unsafe: the translator (`translator.rs:140`) suspends on the policy returning **`Err`**, *not* on the `interactive` flag, and `main.rs` threads the **same policy `Arc`** into both the backend and the translator. Under a deny policy the backend would deny-and-continue while the translator independently **suspends the A2A task with an unresumable `PendingRequest`** (this backend has no resume). So the backend is the sole authority and emits nothing. This is exactly how `AcpBackend` resolves the agent's reverse `session/request_permission` — internally, never as a translator-visible `Update::Permission`. The proof is a gated test that drives an api turn through the **real `Translator::run`** with a deny policy and asserts no suspend, no pending, and the deny reaching the model as a tool result.

## Consequences

- **The conductor decision now has its non-process evidence — two backend kinds shipped:** ACP (local-process) + API (non-process HTTP). The conductor re-evaluation stays **parked**; this increment produces the evidence it was waiting on, it does not make the decision.
- **Vendor-neutral & reusable.** Any OpenAI-compatible endpoint (local Ollama, or a free-tier hosted API) is a **config change, not new code**. This generalizes the parked Claude-specific B1 into a whole class. (Note: `claude-agent-acp + ANTHROPIC_API_KEY` is *not* a substitute — it bills the API but is still process-based ACP, failing the non-process dimension.)
- **Near-zero new dependency surface.** `reqwest` was already a workspace dependency; `bridge-api` reuses it. The only new dev-dep is `wiremock` (offline mock HTTP). No new runtime dep classes; no paid API; no key for the local gate.
- **Coverage held:** `bridge-api` 97.79% (new HARD CI floor 90%), `bridge-core` 97.88%, workspace 93.84%.

## Conductor evidence (summary for the parked re-eval)

| Surface | What this increment shows |
|---|---|
| A — lifecycle/transport | The ports absorb a non-process backend; the exact exec-centric ripple (`cmd`→`Option`, the `registry::validate` allowed-cmds gate becoming kind-aware, factory + e2e-factory arms, reuse-identity) was bounded and mechanical (~10 sites, one atomic commit). |
| B — permission/policy | The `PolicyEngine` **port itself** absorbed a structurally different (client-side function-calling) permission model with **no change to the port** — the backend routes each `tool_call` through it internally, deny/abstain proven through the real translator. |
| Finding (refined by review) | The port's **`Update::Permission`/translator suspend path is NOT reusable** for non-interactive client-side denials — it keys on policy `Err`, not `interactive`, and the backend has no resume. Combined with the port's **tool-blindness** (`PermissionRequest` carries no tool name/args; `SessionContext` is empty), this is the concrete, conductor-weighable cost: **per-tool / non-interactive permission would require port enrichment** — a clean, separately-weighable follow-on. |

## Notes / follow-ons

- **Live validation is gated.** A `#[ignore]` two-turn test (`api_live_two_turns`) runs against a real local Ollama (`qwen3.5:9b`); run it manually with `cargo test -p bridge-api --test live_ollama -- --ignored`. The model used is not observable through the bridge (the response carries no model id and `usage` is dropped at the port — consistent with the documented ACP model-non-observability).
- **The corpus fixture is `SHAPE-AUTHORED`, not `REAL-CAPTURE`.** Ollama was not installed in the build environment, so `tests/fixtures/ollama-openai-compat.json` was authored from the documented OpenAI/Ollama wire shapes (+ `ollama/ollama#7881`) and **honestly marked `SHAPE-AUTHORED`**. It replays through the real SSE parser and is the single source of the tool-call wiremock stub body. Replacing it with a genuine `curl` capture against Ollama is a documented pre-production follow-on.
- **Carry-forwards (unchanged):** port enrichment (tool name/args → policy) is the documented surface-B follow-on; interactive tool-permission suspend/resume to the A2A caller; non-OpenAI-compatible vendor schemas; per-entry env support (`CLAUDE_CODE_OAUTH_TOKEN`); 3b.2 admin API; 3d fan-out.
