# Containerized Agents — Operator Runbook (Slice A)

Run the a2a-bridge's review/design agents (claude, codex, kiro) as **`:ro` containerized readers**
behind an **egress-locked proxy**, plus a non-process **ollama** (`kind="api"`) agent. Containment is
the security boundary (ADR-0013/ADR-0016): the `:ro` mount is the only hard read-only guarantee, and
egress is locked to the model providers only.

**Validated:** macOS + Docker Desktop (the container runtime is a Linux VM). Production target is
rootless **podman on Linux** (CLI-compatible; bind-mount I/O is ~native vs slower on macOS).

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
