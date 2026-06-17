# Anti-pattern: the containerized MCP-subprocess environment trap

**TL;DR — the rule.** A containerized coding agent (codex-acp, claude, …) that spawns an MCP server as a
subprocess gives that subprocess a **minimal, stripped environment**. It forwards *only* the env you
configured under `mcp_servers.<name>.env.*` (in the bridge: the profile's `lsp_env`) plus a PATH it
manages itself — **NOT the container image's `ENV`**. Therefore:

1. **Every environment variable the MCP server (and any tool it spawns) needs MUST be in `lsp_env`.**
   Do not rely on the image's `ENV` reaching the server — it won't.
2. **Expose tools as real binaries on a stable PATH (symlink into `/usr/local/bin`).** Never rely on a
   runtime **shim** (mise) or **proxy** (rustup) that resolves what to exec by reading the environment —
   that environment is stripped, so the shim/proxy fails.
3. **Don't try to set `PATH` via `lsp_env`.** The agent manages PATH itself (it prepends its own arg0
   dir). `/usr/local/bin`, `/usr/bin`, the language toolchain bins (`/usr/local/cargo/bin`,
   `/usr/local/go/bin`, …) are already on the stripped PATH — so a real binary symlinked into
   `/usr/local/bin` resolves. A `PATH` override in `lsp_env` is ignored (observed) and is dead weight.

This doc records the investigation that established the rule, so the next person doesn't re-derive it.

## Why it's a trap

The failure is **opaque and misattributed**. The agent doesn't say "your MCP server's interpreter
couldn't start"; it says **"no lsp tool"** (or the tool call returns nothing). The MCP server process
*started* and even answered `initialize`/`tools/list` in isolation, so every layer looks healthy until
you inspect the subprocess's actual environment. Worse, a *direct-binary* server (gopls) works fine under
the same setup, so "Go works, Rust doesn't" sends you hunting for a Rust/RA-specific bug that isn't there.

## The investigation (#1d, 2026-06-17)

**Symptom.** Under the per-turn containerized path (`run-workflow`/`serve`), a codex agent reported
**"no lsp tool"** for a **Rust** repo, while **Go** navigated fine. lsp-mcp (the nav shim) ran standalone
in the toolchain image and served `tools/list` in 0.25s.

**Wrong hypotheses (ruled out, in order):**
- *RA cold-index / egress / readiness window.* `tools/list` is static (doesn't depend on the language
  server), so "no lsp tool" can't be RA-not-ready. Disproven.
- *PATH can't find rust-analyzer.* A minimal-PATH repro *did* fail — but that was a red herring; see below.
  Symlinking RA into `/usr/local/bin` did **not** fix it. Reverted.

**The diagnostic that cracked it.** Replace the MCP server `command` with a shell wrapper that dumps the
*actual* subprocess environment to a file, then `exec`s the real server:

```toml
command = "/bin/sh"
args = ["-c", "mkdir -p {cwd}/.git/a2a-bridge; echo PATH=$PATH > {cwd}/.git/a2a-bridge/lsp-spawn.txt; env >> {cwd}/.git/a2a-bridge/lsp-spawn.txt; exec /usr/local/bin/lsp-mcp --repo {cwd} --lang auto --target-cache /lsp-target 2>> {cwd}/.git/a2a-bridge/lsp-spawn.txt"]
```
**Gotcha:** the bridge's MCP-arg validator rejects `{…}` template tokens other than `{cwd}` — so the shell
wrapper must **not** use `{ … }` brace groups (use `;`-chained commands instead). `{cwd}` resolves to the
session repo (the `.git/` is on the shared `:rw` mount, so the dump survives the `--rm` container).

**What the dump showed.** codex's MCP-subprocess env was just:
`HOME=/root`, a PATH it prepended (`/root/.codex/tmp/arg0/…:/usr/local/go/bin:/root/go/bin:/usr/local/cargo/bin:/root/.local/bin:/usr/local/sbin:/usr/local/bin:…`), and the **forwarded `lsp_env`** (`CARGO_HOME`,
`CARGO_NET_OFFLINE`, `LSP_MCP_LOG`). The image's `ENV` (notably **`RUSTUP_HOME=/usr/local/rustup`**) was
**absent**, and lsp-mcp's stderr showed `Error: LSP request initialize timed out`.

**Root cause.** `rust-analyzer` at `/usr/local/cargo/bin/rust-analyzer` is a **rustup proxy symlink**.
Without `RUSTUP_HOME`, the proxy looks for a toolchain at the default `~/.rustup` (absent) →
`"rustup could not choose a version of rust-analyzer to run"` → the proxy exits → lsp-mcp's `initialize`
LSP request gets no response → 30s timeout → **lsp-mcp exits** → codex sees the MCP server die → reports
**"no lsp tool"**. **Go worked** because `gopls` is a *direct binary* needing no env to resolve a version.
Confirmed by isolation: `rust-analyzer --version` **without** `RUSTUP_HOME` → the rustup error;
**with** `RUSTUP_HOME=/usr/local/rustup` → `rust-analyzer 1.94.0`.

**The fix.** One config line — `RUSTUP_HOME=/usr/local/rustup` in the rust profile's `lsp_env` (it
forwards exactly like `CARGO_HOME` already did). No image rebuild, no code change.

## How this generalizes (the pattern for every language)

| Server | Direct binary? | Env it needs in `lsp_env` | Exposure |
|---|---|---|---|
| gopls (Go) | yes | `GOMODCACHE`, `GOFLAGS` | symlinked `/usr/local/bin/gopls` |
| rust-analyzer (Rust) | **no** (rustup proxy) | `CARGO_HOME`, `CARGO_NET_OFFLINE`, **`RUSTUP_HOME`** | rustup proxy on PATH (`/usr/local/cargo/bin`) |
| basedpyright (Python) | node CLI | **`LSP_MCP_PYTHON_PATH`** (→ the warmed venv interpreter), `PYTHONDONTWRITEBYTECODE` | symlink `basedpyright`/`-langserver`; node already at `/usr/local/bin` |
| typescript-language-server (JS/TS, future) | node CLI | (TBD — `tsserver`/`typescript.tsdk` pointer, node) | symlink the launcher; node on PATH |

**When adding a language, ask:** (1) Is the server a proxy/shim or a direct binary? A proxy/shim needs its
resolver env in `lsp_env`. (2) What does the server read at startup that lives in the image `ENV`? Put it
in `lsp_env`. (3) Are the binaries reachable as real files on the stripped PATH? If not, symlink them into
`/usr/local/bin` — never depend on mise shims or `mise activate`. (4) Validate with the env-dump wrapper
above **before** assuming a deeper bug.

## mise specifically

mise is fine as an **installer** (it places real executables at
`~/.local/share/mise/installs/<tool>/<version>/bin/<bin>`, absolute-path reachable without activation).
But mise **shims** (`~/.local/share/mise/shims/*` → the mise binary) resolve the tool version at runtime
**from mise's environment/config** — which the stripped MCP-subprocess env doesn't carry. So a shim is the
rustup-proxy trap all over again. **Install with mise; expose the real binary (symlink to `/usr/local/bin`);
never put a mise shim on the runtime path.**

## References

- #1d spec/plan: `docs/superpowers/specs/2026-06-17-lsp-mcp-1d-ra-readiness-per-turn.md`,
  `docs/superpowers/plans/2026-06-17-lsp-mcp-1d-ra-readiness.md` (the "Outcome" section).
- Fix commit: `2f4b648` (RUSTUP_HOME keystone), branch merged at `90c7557`.
- Python slice (applies the rule): `docs/superpowers/specs/2026-06-17-lsp-mcp-python-polyglot.md`.
