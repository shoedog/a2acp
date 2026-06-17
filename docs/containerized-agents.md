# Containerized Agents — Operator Runbook (Slice A)

Run the a2a-bridge's review/design agents (claude, codex, kiro) as **`:ro` containerized readers**
behind an **egress-locked proxy**, plus a non-process **ollama** (`kind="api"`) agent. Containment is
the security boundary (ADR-0013/ADR-0016): the `:ro` mount is the only hard read-only guarantee, and
egress is locked to the model providers only.

**Validated:** macOS + Docker Desktop (the container runtime is a Linux VM). Production target is
rootless **podman on Linux** (CLI-compatible; bind-mount I/O is ~native vs slower on macOS).

> ⚠️ **Adding an in-container LSP/MCP server (a new language, a new tool)?** Read
> [`containerized-mcp-env-trap.md`](containerized-mcp-env-trap.md) FIRST. A containerized agent hands its
> spawned MCP subprocesses a **stripped env** (not the image `ENV`), so anything the server needs must be
> in the profile `lsp_env`, tools must be **real binaries on PATH (no mise/rustup shims)**, and the
> failure mode is an opaque **"no lsp tool"**. That doc has the diagnostic + the rule.

---

## 1. Build the reader image

```bash
docker build -t a2a-agent-reader:latest -f deploy/containers/reader.Containerfile deploy/containers
# smoke: all three CLIs resolve
docker run --rm a2a-agent-reader:latest sh -c \
  'command -v claude-agent-acp && command -v codex-acp && command -v kiro-cli'
```
The image is `node:24-slim` + `claude-agent-acp` + `codex-acp` (npm) + `kiro-cli` (Linux build via the
official zip; installed with `--force --no-confirm` since it runs as root unattended).

## 2. Bring up the egress lockdown

```bash
docker compose -f deploy/containers/compose.egress.yaml up -d --build
```
Two networks: agents live on `a2a-egress-internal` (`internal: true` → no route out); the
`a2a-egress-proxy` (tinyproxy, default-deny) straddles that net + `a2a-egress-external` to reach the
providers. The allowlist (`deploy/containers/tinyproxy.filter`) is **anchored POSIX-ERE host regexes**
(tinyproxy `Filter` is ERE — a literal `*.anthropic.com` is invalid). Edit the filter then re-run
`up -d --build` to apply (the filter is baked into the proxy image).

## 3. Credentials (per agent)

**Never mount `~`.** Use isolated, **writable** copies (tokens refresh by writing back — a `:ro` creds
mount breaks refresh):

- **claude** (OAuth subscription) and **codex** (ChatGPT auth) — single-file copies:
  ```bash
  mkdir -p ~/.config/a2a-creds/claude ~/.config/a2a-creds/codex
  cp ~/.claude/.credentials.json ~/.config/a2a-creds/claude/.credentials.json
  cp ~/.codex/auth.json          ~/.config/a2a-creds/codex/auth.json
  chmod -R u+rw ~/.config/a2a-creds
  ```

> **Token rotation (re-sync before each session).** OAuth/SSO tokens **rotate on refresh** (the refresh
> token is single-use), so a copy goes **stale** when you use the agent *on the host* — the host rotates
> the lineage and the copy's refresh token dies, surfacing as `session/prompt failed: transport error`.
> Run the pre-flight sync before a containerized session so each short turn borrows the host's *current*
> token (no mid-turn refresh → no rotation → the host stays valid too):
> ```bash
> deploy/containers/sync-creds.sh && a2a-bridge serve --config examples/a2a-bridge.containerized.toml
> ```
> (claude/codex are host-file copies; **kiro** is the `a2a-kiro-data` volume — re-run its device-flow
> login if it has fully expired, not a host sync.)
>
> **Automate it (optional, macOS launchd).** Instead of running the pre-flight sync by hand, keep the
> copies continuously fresh with a LaunchAgent that runs `sync-creds.sh` every 5 min (token TTLs are hours,
> so the copy is always valid; short container turns don't refresh, so no rotation). A version-controlled
> template lives at `deploy/containers/com.a2a-bridge.creds-refresh.plist`:
> ```bash
> cp deploy/containers/com.a2a-bridge.creds-refresh.plist ~/Library/LaunchAgents/
> launchctl load -w ~/Library/LaunchAgents/com.a2a-bridge.creds-refresh.plist   # runs now + every 5 min
> # logs: /tmp/a2a-creds-refresh.log   •   remove: launchctl unload -w … && rm ~/Library/LaunchAgents/com.a2a-bridge.creds-refresh.plist
> ```
> (Edit the `sync-creds.sh` path in the plist if your checkout isn't at `~/code/a2a-bridge`.)
- **kiro** — a one-time in-container **device-flow** login (the host's macOS auth is NOT portable to
  Linux). Auth lives in `~/.local/share/kiro-cli/data.sqlite3`, persisted to a named volume:
  ```bash
  docker volume create a2a-kiro-data
  docker run -it --rm -v a2a-kiro-data:/root/.local/share \
    a2a-agent-reader:latest kiro-cli login --use-device-flow
  # pick a sign-in method, open the printed URL, enter the code. Verify:
  echo "Reply with exactly: KIRO_OK" | docker run -i --rm -v a2a-kiro-data:/root/.local/share \
    a2a-agent-reader:latest kiro-cli chat --no-interactive --wrap never
  ```
- **ollama** — local needs a pulled model (`ollama pull qwen2.5-coder:7b`); cloud needs
  `OLLAMA_API_KEY` in the serve process env (`base_url = https://ollama.com/v1`).

## 4. Serve / run

```bash
# one serve drives every repo under the mount root (via per-request session_cwd):
a2a-bridge serve --config examples/a2a-bridge.containerized.toml      # reads from CWD; run from the repo root
```
**Load-bearing rule:** `allowed_cwd_root` MUST equal the `:ro` mount root (it's opt-in — unset means
**no cwd gate**). The identical-path mount (`-v /Users/wesleyjinks/code:/Users/wesleyjinks/code:ro`)
makes the per-request `session_cwd` resolve unchanged inside the container.

> **Slice B1 (ADR-0017):** with the enforced `[sandbox]` block, this rule is now a **load error, not just
> operator discipline** — `into_snapshot` rejects a sandboxed agent whose `mount != allowed_cwd_root` (S2),
> and the bridge composes the `docker run` argv itself (so `:ro`/egress/`--network` can't be forgotten).
> See the `[sandbox]` form in `examples/a2a-bridge.containerized.toml`. (`mount`/`allowed_cwd_root` are
> boot-fixed — changing them needs a restart.)

## 5. Validation gates (all PASS as of 2026-06-04)

**(a) Egress curl-triad** — providers allowed, everything else blocked:
```bash
docker run --rm --network a2a-egress-internal -e HTTPS_PROXY=http://a2a-egress-proxy:8888 \
  a2a-agent-reader:latest sh -c '
    for h in api.anthropic.com api.openai.com github.com example.com; do
      echo "$h -> $(curl -sS -o /dev/null -w "%{http_code}" --max-time 15 https://$h 2>/dev/null || echo BLOCKED)"; done'
# expect: anthropic/openai a real code; github/example BLOCKED
```

**(b) `:ro` integrity** — capture the CID WHILE running (`--rm` deletes it on exit):
```bash
docker run -d --name roprobe -v /Users/wesleyjinks/code:/Users/wesleyjinks/code:ro a2a-agent-reader:latest sleep 30
docker inspect roprobe --format '{{json .HostConfig.Binds}}'   # must contain :ro
docker exec roprobe sh -c 'echo x > /Users/wesleyjinks/code/__x__ 2>&1 || echo "repo write blocked (expected)"'
docker rm -f roprobe
```

**(c) Per-agent auth smoke** (single-agent workflows — `design`/`code-review` would run BOTH agents).
**Run from a dir under the mount** (`run-workflow` uses the static cwd, not a per-request `session_cwd`):
```bash
a2a-bridge run-workflow smoke-claude --input README.md --config examples/a2a-bridge.containerized.toml   # -> SMOKE_OK: <repo files>
a2a-bridge run-workflow smoke-codex  --input README.md --config examples/a2a-bridge.containerized.toml
a2a-bridge run-workflow smoke-kiro   --input README.md --config examples/a2a-bridge.containerized.toml
OLLAMA_API_KEY=... a2a-bridge run-workflow smoke-ollama       --input README.md --config examples/a2a-bridge.containerized.toml
OLLAMA_API_KEY=... a2a-bridge run-workflow smoke-ollama-cloud --input README.md --config examples/a2a-bridge.containerized.toml
```

**(d) cwd gate** (only the `serve`+A2A path enforces it — method is CamelCase `SendMessage`, needs the
`A2A-Version: 1.0` header, and the cwd lives under `message.metadata`):
```bash
curl -sS -X POST http://127.0.0.1:8080/ -H 'content-type: application/json' -H 'A2A-Version: 1.0' -d '{
  "jsonrpc":"2.0","id":1,"method":"SendMessage",
  "params":{"message":{"role":"user","parts":[{"kind":"text","text":"hi"}],
                       "metadata":{"a2a-bridge.cwd":"/etc"}}}}'
# expect: {"error":{"code":-32600,"message":"invalid request: a2a-bridge.cwd"}}  (outside mount root)
# a cwd UNDER the root is accepted, and the containerized agent reads THAT repo (multi-repo via one mount).
```

## 6. Egress allowlist discovery (the method, not a guess)

The default-deny proxy **is** the discovery tool. Run an agent behind it and read the denied log:
```bash
docker logs a2a-egress-proxy 2>&1 | grep -i "refused on filtered" | sed -E 's/.*filtered domain "([^"]+)".*/\1/' | sort -u
```
Add the **exact** hosts as anchored ERE regexes (NOT broad globs like `amazonaws.com`), then
`up -d --build`. Discovered so far: claude `*.anthropic.com`; **codex `chatgpt.com`** (ChatGPT
backend, not `api.openai.com`); **kiro `cognito-identity.us-east-1.amazonaws.com`,
`q.us-east-1.amazonaws.com`, `*.kiro.dev`**.

## 7. Fallbacks & caveats

- **claude-only fallback:** if any agent's in-box auth / egress won't close, drop it and run
  claude-only containerized (claude is the proven baseline, ADR-0013).
- **ollama cloud is host-direct egress** — a cloud `base_url` (ollama.com) is the **bridge** calling
  out (non-process), so it bypasses the container proxy. Safe, but note it; local ollama has no remote
  egress at all.
- **macOS Docker Desktop:** add `~/.config` under Settings → Resources → File Sharing if a creds mount
  is rejected; bind-mount I/O is slower than Linux.
- The egress proxy + the agent image are **operator-maintained infra** (not bridge code).

## 8. Write-capable agents (`container_rw`, Slice B2a)

`kind="container_rw"` unlocks a **write-capable** agent: the bridge spawns a **fresh `:rw` container per
turn** (composing the same ACP machinery as the `:ro` readers) and reliably reaps it (an explicit
`docker rm -f` on every terminal path, since killing the `docker run` client doesn't remove the `--rm`
container). Config mirrors a sandboxed `:ro` agent but with `kind="container_rw"` + `access="rw"`;
validation requires `cmd` + `[sandbox]` and PERMITS `access=rw` (the `acp` kind still rejects it). The
`:rw` mount is the **per-request session cwd** (a scratch dir; in B2b a per-task git clone), gated
`is_under` the **canonicalized** mount root — symlinks are resolved, so a `:rw` target can't escape the
root via a symlink.

> **Per-turn memory asymmetry.** Unlike the warm `:ro` reader (one long-lived container, conversational
> memory across turns), a `container_rw` agent mints a fresh container + ACP session **each turn**, so it
> does NOT retain conversational memory across turns in interactive `serve`. Work continuity comes from the
> shared `:rw` target (the clone/scratch on the host), not the container. (A warm-pool for writers is a
> separate future slice.)

Set the per-request `:rw` target via **`serve` + A2A** (`message.metadata` cwd) or, for **`run-workflow`**,
the `--session-cwd <dir>` flag — without it, agents run in the LAUNCH cwd, not the target repo.

## 9. Podman (macOS `podman machine`)

The runtime is config-selected (the bridge is runtime-agnostic). Docker stays the default; podman is opt-in.
This section is validated on macOS `podman machine`; **Linux rootless is a separate follow-up** (uid/SELinux
semantics differ). Minimum **podman ≥ 4.5** (netavark ≥ 1.6) for DNS on `--internal` networks.

**1. Select podman.** Use `examples/a2a-bridge.containerized.podman.toml` — or, in your own config, the
two-line rule: add `"podman"` to `allowed_cmds` **and** `runtime = "podman"` to every `[agents.sandbox]`
block **and** `[verify]`.

**2. Machine.** `podman` resolves via `PATH` (a launchd-launched `serve` needs `podman` on its `PATH`).

```bash
podman machine init --cpus 6 --memory 8192 --disk-size 100 && podman machine start
podman machine inspect | grep -i mount   # confirm /Users is mounted (the identical-path -v {m}:{m} bind)
```

**3. Build the images** — podman has a **separate image store** (it cannot see docker-built images), so
build in order:

```bash
podman build -t a2a-agent-reader:latest -f deploy/containers/reader.Containerfile deploy/containers
# toolchain uses the REPO-ROOT context (L3 Slice B): its lspbuild stage compiles crates/lsp-mcp from the
# workspace. The repo-root .dockerignore keeps the context small (excludes target/, ~99G).
podman build -t a2a-toolchain:latest    -f deploy/containers/toolchain.Containerfile .  # reader image + RA + lsp-mcp
podman build -t a2a-egress-proxy:latest -f deploy/containers/proxy.Containerfile  deploy/containers
```

> **Per-repo cache volumes (verify + L3 Slice B impl-lsp).** `implement`/`verify` create per-source-repo
> named volumes — `a2a-verify-cache-<hash>`, `a2a-impl-lsp-cache-<hash>` (warmed RA deps, mounted `:ro`),
> `a2a-impl-lsp-target-<hash>` (RA's `CARGO_TARGET_DIR`). They are keyed on the **source repo** so they're
> reused across runs (bounded to one set per repo), but nothing reaps them automatically and the target
> cache grows over time. To reclaim disk: `docker volume rm $(docker volume ls -q | grep -E 'a2a-(verify-cache|impl-lsp)')`.

**4. Egress.** Use the podman script (NOT `docker compose`). **Re-run it after every `podman machine start`**
— `--restart` does not survive a daemonless machine restart.

```bash
deploy/containers/podman-egress.sh up       # idempotent; re-run after a machine restart
deploy/containers/podman-egress.sh status   # 3 networks + 2 proxies
deploy/containers/podman-egress.sh down
```

If name resolution of `a2a-egress-proxy` from the internal net fails (old podman), use the IP-pinning
fallback documented in the script header (`--subnet`/`--ip` + `proxy = "http://<ip>:8888"` in the config).

**5. Kiro creds.** The `a2a-kiro-data` volume does **not** carry over from docker — re-mint under podman:
`podman run -it --rm -v a2a-kiro-data:/root/.local/share a2a-agent-reader:latest kiro-cli login --use-device-flow`
(`sync-creds.sh` prints this hint honoring `CONTAINER_RUNTIME=podman`).

**6. Run.** `a2a-bridge serve --config examples/a2a-bridge.containerized.podman.toml` (or `run-workflow` /
`implement` with that config). A configured-but-unresponsive runtime is warned about at startup (the hard
gate is the `allowed_cmds`/S3 allowlist + the verify-runtime gate).

**Caveats.** `containers list|reap` does **not** see verify containers (no `a2a.managed=1` label — true on
docker too). `podman rm` is **synchronous**, so expect **0** containers immediately after a run (unlike
Docker Desktop's ~2 s async removal). A disallowed `[verify].runtime` (not in `allowed_cmds`) makes verify
fail with `ConfigError` rather than running on the wrong engine.
