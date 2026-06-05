# ADR-0016 — Containerized Agents, Slice A (`:ro` readers + egress lock + api agent)

**Date:** 2026-06-04
**Status:** Accepted

**Builds on:** ADR-0013 (containment + egress as the boundary), ADR-0014 (`session_cwd`). Realises
ADR-0013's posture as a config-only, **validated** deployment; amends its "config-only" stance toward
the Slice B enforced `[sandbox]` block (next increment).

---

## Context

The bridge runs full tool-using ACP agents. The R1 finding (self-hosted-review increment) established
that agent CLIs **cannot** be reliably tool-restricted via flags (`claude-agent-acp` has none), so the
`:ro` bind mount is the **only hard read-only guarantee**. ADR-0013 proved the posture (container +
`:ro` + egress lock) works with three probes but did not ship a validated config or cover codex/kiro.

## Decision

Ship **Slice A** as **config + infra + prompts + docs, zero bridge (Rust) code**: the registry already
passes each agent's `cmd`/`args` to `Supervised::spawn`, and the ACP session cwd is sent over the
protocol at `session/new` (not the OS process cwd), so an agent becomes
`cmd="docker" args=["run", …, "<agent-cli>"]` with an **identical-path `:ro` mount**. Egress is locked
by a default-deny tinyproxy on an `--internal` Docker network. The non-process **ollama** (`kind="api"`)
agent is **uncontainerized by design** (no fs/tool surface; the bridge brokers its one HTTP call).

## Evidence (validated live, 2026-06-04, macOS + Docker Desktop)

All five gates PASS:
- **Egress:** providers reached through the proxy (`api.anthropic.com → 404`, `api.openai.com → 421`);
  `github.com`/`example.com` blocked; no direct route from the `--internal` net.
- **`:ro` integrity:** bind asserted `:ro`; a write to the repo mount → "Read-only file system".
- **Per-agent auth smokes** (single-agent workflows): **claude, codex, kiro** each authenticate
  in-container through the proxy, read the repo `:ro`, honor the ACP session cwd, and terminate;
  **ollama local** (`qwen2.5-coder:7b`) and **ollama cloud** (`qwen3-coder:480b`) each return.
- **cwd gate** (serve+A2A): `a2a-bridge.cwd=/etc` → `invalid request: a2a-bridge.cwd`; an under-root
  cwd is accepted and the containerized agent reads that repo (**multi-repo via one mount**).

## Key findings (empirical, recorded)

- **codex hits `chatgpt.com`** (its ChatGPT backend), NOT `api.openai.com` — found via the proxy's
  denied log. **kiro** needs `cognito-identity.us-east-1.amazonaws.com`, `q.us-east-1.amazonaws.com`,
  `*.kiro.dev`. All pinned as **exact** anchored-ERE hosts (no broad `amazonaws.com`).
- **kiro auth is NOT portable from macOS → Linux** (it tries to open a browser). It must be minted
  in-container via `kiro-cli login --use-device-flow`; the Linux auth state is
  `~/.local/share/kiro-cli/data.sqlite3` (a sqlite DB, **not** `~/.aws`), persisted to a writable named
  volume. Its `install.sh` needs `--force` (refuses root) `--no-confirm` (unattended).
- **Creds must be writable** isolated copies (OAuth/SSO tokens refresh by writing back; a `:ro` creds
  mount breaks refresh) — distinct from the source mount, which stays `:ro`.
- **`run-workflow` does NOT enforce the cwd gate** — it uses the static `current_dir`, so its smokes
  must be run from a dir under the mount; only the `serve`+A2A path threads a per-request `session_cwd`
  through `is_under`. `allowed_cwd_root` is opt-in and MUST equal the mount root.
- **A2A wire:** method is CamelCase `SendMessage`; requires the `A2A-Version: 1.0` header; the cwd
  metadata lives under `message.metadata`.

## Also shipped: two-pass refine

`design`/`spec-review`/`plan-review` gained a grounded **second pass** (clean-room draft `inputs=[]` →
a refine node that reads its OWN draft + a gaps register → synth unchanged). The `inputs=[]` firewall
is preserved (each refiner sees only its own draft). Config + prompts only.

## Consequences

- **claude + codex + kiro** are validated `:ro` containerized readers; **ollama** (local + cloud)
  validated as the uncontainerized api agent. `AutoPolicy` is acceptable *inside* the contained +
  egress-locked box (ADR-0013).
- **ollama cloud is host-direct egress** (the bridge calling ollama.com, non-process) — it bypasses
  the container proxy; documented, acceptable. Local ollama has no remote egress.
- **claude-only is the documented fallback** if any agent's auth/egress can't be closed.

## Follow-ons (Slice B+, directions — not locked)

- The enforced **`[sandbox]` block** (bridge composes the argv + enforces `:ro`/egress/no-`~`
  invariants so config can't silently degrade) — the codeful half.
- The write-capable **`implement`** workflow (per-task git worktree, `:rw`, verify gate) + per-task
  containers (`ContainerRwBackend`) + a per-agent `scratch:rw` volume. See the design doc.
- Rootless **podman on Linux** as the production runtime; an L3/L4 egress backstop for any agent that
  doesn't honor `HTTPS_PROXY` (not needed for claude/codex/kiro — all honor it).

## Firewall

Designed + validated from the bridge's own ports (registry passthrough, `AcpBackend::spawn`, the
`session/new` cwd, the workflow DAG/`inputs` firewall, `session_cwd`/ADR-0014) + container/network
primitives + the ADR-0013 probes + this increment's live validation. The `a2a-local-bridge` PoC was
used only black-box for the spec/plan dual-reviews; it did not inform the design.
