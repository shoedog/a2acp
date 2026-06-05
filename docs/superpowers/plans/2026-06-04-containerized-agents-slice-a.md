# Containerized Agents — Slice A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the existing review/design agents (codex, claude, kiro) as `:ro` containerized readers behind an egress-locked proxy, plus a non-process `ollama` (`kind="api"`) agent — validated end-to-end against this repo, with **zero bridge (Rust) code**.

**Architecture:** Config + infra + prompts + docs only. The registry already passes each agent's `cmd`/`args` straight to `Supervised::spawn`, and the ACP session cwd is sent over the protocol at `session/new` (not the OS process cwd), so wrapping an agent as `cmd="docker" args=["run", …, "<agent-cli>"]` with an **identical-path `:ro` mount** is a pure config change. Egress is locked by a default-deny tinyproxy on an `--internal` Docker network. Verification is a set of **falsifiable manual gates** (Docker-gated, not CI).

**Tech Stack:** Docker (Desktop, macOS — 29.4.0), `node:24-slim` base image, npm-installed ACP CLIs, tinyproxy (default-deny, POSIX-ERE host allowlist), the existing `a2a-bridge` binary + workflows (`code-review`/`design`), `bridge-api` for ollama.

**Scope note (from grounding):** the reader image is **Linux** — on macOS, Docker Desktop runs all
containers in a Linux VM — so the host's `kiro-cli` (a macOS Mach-O at `/Applications/Kiro CLI.app`)
can't run in it; we install kiro's **Linux** build *into* the image via the official arch-aware
zip method (Task 1). All three agents — `claude-agent-acp` + `codex-acp` (npm) + `kiro-cli` (zip
installer) — are first-class containerized readers; **kiro is no longer deferred.** kiro auth is an
interactive **device flow** done **once** inside a container, persisted to a writable named volume
(Task 9). Claude has prior container evidence (ADR-0013); codex/kiro carry **four unproven
assumptions** (auth / egress allowlist / `HTTPS_PROXY` honoring / ACP-cwd honoring) retired during
validation (Task 8/9). **claude-only is the documented fallback** if any agent fails those.

**Branch:** `feat/containerized-agents` (already created; spec committed at `2357a97`).

**Commit trailers:** controller docs (this plan, ADR-0016) carry `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; task/artifact commits do NOT.

---

## File Structure

**Create:**
- `examples/sample-input.md` — a tiny problem-statement input for validation runs (Task 0).
- `prompts/smoke-read.md`, `prompts/smoke-reply.md` — single-agent auth-smoke prompts (Task 8).
- `deploy/containers/reader.Containerfile` — the `:ro` reader image (node + ACP CLIs + read tools).
- `deploy/containers/proxy.Containerfile` — a pinned tinyproxy image.
- `deploy/containers/tinyproxy.conf` — default-deny proxy config.
- `deploy/containers/tinyproxy.filter` — anchored POSIX-ERE host allowlist.
- `deploy/containers/compose.egress.yaml` — two networks + the proxy service.
- `examples/a2a-bridge.containerized.toml` — the containerized agent config (the deliverable).
- `prompts/design-executability-refine.md`, `prompts/design-structure-refine.md` — two-pass design refine prompts.
- `prompts/spec-review-rigor-refine.md`, `prompts/spec-review-soundness-refine.md` — two-pass spec-review refine.
- `prompts/plan-review-exec-refine.md`, `prompts/plan-review-coverage-refine.md` — two-pass plan-review refine.
- `docs/containerized-agents.md` — the operator runbook (build, bring up egress, copy creds, run, the gates).
- `docs/adr/0016-containerized-agents-slice-a.md` — ADR.

**Modify:**
- `examples/a2a-bridge.multi-agent.toml` — restructure `design`/`spec-review`/`plan-review` into draft→refine (Phase D), mirrored into the containerized config.

**No Rust files change.** If any task discovers a required code change, STOP and escalate — that breaks the zero-code premise and belongs in Slice B.

---

## Phase A — The reader image

### Task 0: Author the workflow input file (dual-review gap — nothing else creates it)

**Files:** Create `examples/sample-input.md`.

- [ ] **Step 1: Write a tiny problem-statement input** (used as `--input` by the config-parse /
  two-pass validations; the smoke workflows ignore their input and can use `README.md`):

```markdown
<!-- examples/sample-input.md -->
Problem statement (sample): Evaluate whether the a2a-bridge's workflow executor cleanly separates
node fan-out from the synthesis step. Keep it brief.
```

- [ ] **Step 2: Commit**

```bash
git add examples/sample-input.md
git commit -m "examples: sample workflow input for containerized validation runs"
```

### Task 1: Build the `:ro` reader image (codex + claude + kiro)

**Files:**
- Create: `deploy/containers/reader.Containerfile`

- [ ] **Step 1: Write the Containerfile**

```dockerfile
# a2a-bridge reader image: portable ACP agent CLIs + read-only exploration tools. NO build toolchain
# (readers verify via read/grep/git diff; they don't compile — that's the Slice B implement image).
FROM node:24-slim

# Read tools the review/design lenses use, + curl for the egress gate + the kiro installer,
# + unzip/ca-certificates for installers, + git/ripgrep for read/grep.
RUN apt-get update && apt-get install -y --no-install-recommends \
      git ripgrep ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

# Pin the ACP agent CLIs (portable Node packages; versions match the host as of 2026-06-04).
# claude-agent-acp pulls @anthropic-ai/claude-agent-sdk, whose optional dep is the platform `claude`
# binary — the LINUX build resolves here, not the host's macOS one.
RUN npm install -g \
      @agentclientprotocol/claude-agent-acp@0.39.0 \
      @zed-industries/codex-acp@0.15.0

# kiro-cli: install the LINUX build (the host's macOS binary can't run in this Linux image). Official
# zip method (https://kiro.dev/docs/cli/installation/#with-a-zip-file); arch-aware so it works whether
# Docker Desktop runs amd64 or arm64 (Apple Silicon → arm64). node:24-slim is bookworm/glibc 2.36 (>=2.34
# required). install.sh drops the binary under ~/.local/bin (root → /root/.local/bin).
RUN set -eux; \
    case "$(dpkg --print-architecture)" in \
      amd64) url="https://desktop-release.q.us-east-1.amazonaws.com/latest/kirocli-x86_64-linux.zip" ;; \
      arm64) url="https://desktop-release.q.us-east-1.amazonaws.com/latest/kirocli-aarch64-linux.zip" ;; \
      *) echo "unsupported arch" >&2; exit 1 ;; \
    esac; \
    curl --proto '=https' --tlsv1.2 -sSf "$url" -o /tmp/kirocli.zip; \
    unzip -q /tmp/kirocli.zip -d /tmp; \
    /tmp/kirocli/install.sh --force --no-confirm; \
    rm -rf /tmp/kirocli /tmp/kirocli.zip
# NOTE: kiro's install.sh needs `--force` (it refuses to run as root, which we are in the image)
# AND `--no-confirm` (unattended/no TTY during docker build).
ENV PATH="/root/.local/bin:${PATH}"

# Workdir is cosmetic: the ACP session cwd arrives over the protocol (session/new); the repo is
# bind-mounted at its identical host path at run time.
WORKDIR /work
```

- [ ] **Step 2: Build the image**

Run: `docker build -t a2a-agent-reader:latest -f deploy/containers/reader.Containerfile deploy/containers`
Expected: build succeeds; final line `naming to docker.io/library/a2a-agent-reader:latest`.

- [ ] **Step 3: Smoke the tools (no hang — just resolve the binaries)**

Run:
```bash
docker run --rm a2a-agent-reader:latest sh -c \
  'command -v claude-agent-acp && command -v codex-acp && command -v kiro-cli && git --version && rg --version | head -1'
```
Expected: prints all three CLI paths, a git version, and an `ripgrep 1.x` line. If a npm CLI is
missing, fix the package name/version against `npm ls -g` on the host. **If `kiro-cli` is missing**,
the installer used a different dir — find it (`docker run --rm a2a-agent-reader:latest sh -c 'ls -R /root/.local /usr/local/bin 2>/dev/null | grep -i kiro'`) and fix the `ENV PATH`; if the installer
itself failed (network/license), capture its output and, if unrecoverable, drop kiro to the
claude+codex core (it's used primarily at work) and note it in ADR-0016.

- [ ] **Step 4: Commit**

```bash
git add deploy/containers/reader.Containerfile
git commit -m "containers: reader image (node + codex-acp + claude-agent-acp + read tools)"
```

---

## Phase B — Egress lockdown

### Task 2: tinyproxy default-deny config + anchored ERE allowlist

**Files:**
- Create: `deploy/containers/tinyproxy.conf`
- Create: `deploy/containers/tinyproxy.filter`

- [ ] **Step 1: Write the proxy config**

```text
# deploy/containers/tinyproxy.conf — content-blind CONNECT allowlist (no MITM).
Port 8888
Listen 0.0.0.0
Timeout 600
# Default-deny: the Filter file is an ALLOWLIST; everything not matched is refused.
FilterDefaultDeny Yes
Filter "/etc/tinyproxy/filter"
FilterExtended On          # POSIX ERE (so anchored host regexes work)
FilterCaseSensitive Off
# Allow CONNECT tunnels only to 443 (HTTPS to the providers).
ConnectPort 443
```

- [ ] **Step 2: Write the allowlist as ANCHORED ERE host regexes (NOT globs)**

```text
# deploy/containers/tinyproxy.filter — one POSIX-ERE per line, matched against the CONNECT host.
# Anchored so `evil-anthropic.com.attacker.net` does NOT match. `*.anthropic.com` would be an
# INVALID regex — this is the dual-review fix.
(^|\.)anthropic\.com$
(^|\.)openai\.com$
# kiro (Amazon Q / CodeWhisperer + AWS SSO/Cognito) — added empirically in Task 9 via the proxy log.
```

- [ ] **Step 3: Commit**

```bash
git add deploy/containers/tinyproxy.conf deploy/containers/tinyproxy.filter
git commit -m "containers: tinyproxy default-deny + anchored ERE host allowlist"
```

### Task 3: Pinned proxy image + two-network compose

**Files:**
- Create: `deploy/containers/proxy.Containerfile`
- Create: `deploy/containers/compose.egress.yaml`

- [ ] **Step 1: Write the proxy image (pin tinyproxy via debian, not a random hub image)**

```dockerfile
# deploy/containers/proxy.Containerfile
FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends tinyproxy curl \
    && rm -rf /var/lib/apt/lists/*
COPY tinyproxy.conf /etc/tinyproxy/tinyproxy.conf
COPY tinyproxy.filter /etc/tinyproxy/filter
EXPOSE 8888
CMD ["tinyproxy", "-d", "-c", "/etc/tinyproxy/tinyproxy.conf"]
```

- [ ] **Step 2: Write the compose (internal net = no route out; proxy straddles both)**

```yaml
# deploy/containers/compose.egress.yaml
# Bring up:  docker compose -f deploy/containers/compose.egress.yaml up -d --build
networks:
  a2a-egress-internal:
    name: a2a-egress-internal
    internal: true          # no gateway → agents on this net have NO direct internet route
  a2a-egress-external:
    name: a2a-egress-external

services:
  egress-proxy:
    build:
      context: .
      dockerfile: proxy.Containerfile
    image: a2a-egress-proxy:latest
    container_name: a2a-egress-proxy
    networks:
      - a2a-egress-internal   # reachable by agents
      - a2a-egress-external   # can reach the providers
    restart: unless-stopped
```

- [ ] **Step 3: Bring it up**

Run: `docker compose -f deploy/containers/compose.egress.yaml up -d --build`
Expected: `a2a-egress-internal` + `a2a-egress-external` networks created; `a2a-egress-proxy` running (`docker ps` shows it Up).

- [ ] **Step 4: Commit**

```bash
git add deploy/containers/proxy.Containerfile deploy/containers/compose.egress.yaml
git commit -m "containers: pinned tinyproxy image + two-network egress compose"
```

### Task 4: GATE — egress curl-triad (validation gate 3)

**Files:** none (verification only).

- [ ] **Step 1: From inside the agent network, prove allow + deny**

Run:
```bash
docker run --rm --network a2a-egress-internal \
  -e HTTPS_PROXY=http://a2a-egress-proxy:8888 -e HTTP_PROXY=http://a2a-egress-proxy:8888 \
  a2a-agent-reader:latest sh -c '
    for h in api.anthropic.com api.openai.com github.com example.com; do
      code=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 15 "https://$h" 2>/dev/null || echo CONNFAIL)
      echo "$h -> $code"
    done'
```
Expected: `api.anthropic.com` and `api.openai.com` return a real HTTP code (e.g. `401`/`404`/`200` — reached the provider); `github.com` and `example.com` return `CONNFAIL` or a tinyproxy `403` (blocked). **If a provider is blocked or a denied host connects, the filter regex is wrong — fix `tinyproxy.filter` and re-run `up -d --build`.**

- [ ] **Step 2: Prove no direct route (defense-in-depth)**

Run:
```bash
docker run --rm --network a2a-egress-internal a2a-agent-reader:latest \
  sh -c 'curl -sS --max-time 8 https://api.anthropic.com -o /dev/null && echo LEAK || echo "no direct route (expected)"'
```
Expected: `no direct route (expected)` (no proxy env → the `--internal` net has no path out).

- [ ] **Step 3: Record the gate result in the runbook stub**

(Defer the runbook prose to Task 11; just note PASS/FAIL here in the commit message.)

```bash
git commit --allow-empty -m "validate: egress curl-triad PASS (anthropic/openai allowed; github/example blocked; no direct route)"
```

---

## Phase C — Containerized config + per-agent validation

### Task 5: The containerized config (`examples/a2a-bridge.containerized.toml`)

**Files:**
- Create: `examples/a2a-bridge.containerized.toml`

- [ ] **Step 1: Write the config — with EVERY dual-review must-fix baked in**

```toml
# Containerized :ro readers (codex + claude) behind the egress lock, + the non-process ollama agent.
# Run:  a2a-bridge serve --config examples/a2a-bridge.containerized.toml   (from the repo root)
# Prereqs: `docker compose -f deploy/containers/compose.egress.yaml up -d --build`,
#          the reader image built, and per-agent creds copied (see docs/containerized-agents.md).

default = "claude"

# MUST-FIX (dual-review): the cwd gate is OPT-IN — it fires only when set, and MUST equal the mount
# root, or readers ship with NO cwd gate. Identical-path mount makes session/new cwd resolve in-box.
allowed_cwd_root = "/Users/wesleyjinks/code"

[registry]
# The spawned program is `docker`; validate() requires it allowlisted.
allowed_cmds = ["docker"]

[server]
addr = "127.0.0.1:8080"

# ── claude: the proven containerized agent (ADR-0013). Default + first to validate. ──
[[agents]]
id   = "claude"
cmd  = "docker"
args = [
  "run", "-i", "--rm",
  "--network", "a2a-egress-internal",
  "-e", "HTTPS_PROXY=http://a2a-egress-proxy:8888",
  "-e", "HTTP_PROXY=http://a2a-egress-proxy:8888",
  "-v", "/Users/wesleyjinks/code:/Users/wesleyjinks/code:ro",                          # identical-path :ro source
  "-v", "/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json",  # WRITABLE single-file creds
  "a2a-agent-reader:latest",
  "claude-agent-acp",
]

# ── codex: second to validate (auth + openai egress unproven in-container). ──
[[agents]]
id   = "codex"
cmd  = "docker"
args = [
  "run", "-i", "--rm",
  "--network", "a2a-egress-internal",
  "-e", "HTTPS_PROXY=http://a2a-egress-proxy:8888",
  "-e", "HTTP_PROXY=http://a2a-egress-proxy:8888",
  "-v", "/Users/wesleyjinks/code:/Users/wesleyjinks/code:ro",
  "-v", "/Users/wesleyjinks/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json",  # WRITABLE single-file creds
  "a2a-agent-reader:latest",
  "codex-acp",
]

# ── kiro: Linux build baked into the image (Task 1). Device-flow auth done ONCE in-container
#    (`kiro-cli login --use-device-flow`) and persisted to the WRITABLE named volume `a2a-kiro-data`.
#    VERIFIED LIVE: kiro's Linux auth state is `~/.local/share/kiro-cli/data.sqlite3` (NOT ~/.aws);
#    the token refreshes into the volume. See Task 9. ──
[[agents]]
id   = "kiro"
cmd  = "docker"
args = [
  "run", "-i", "--rm",
  "--network", "a2a-egress-internal",
  "-e", "HTTPS_PROXY=http://a2a-egress-proxy:8888",
  "-e", "HTTP_PROXY=http://a2a-egress-proxy:8888",
  "-v", "/Users/wesleyjinks/code:/Users/wesleyjinks/code:ro",
  "-v", "a2a-kiro-data:/root/.local/share",   # WRITABLE: kiro Linux auth (kiro-cli/data.sqlite3)
  "a2a-agent-reader:latest",
  "kiro-cli", "acp",
]

# ── ollama: non-process api agent (kind="api"). Uncontainerized by design — no mount/proxy/creds.
#    Tools: NOT tool-free — it advertises a deterministic side-effect-free stub tool (no fs/shell).
#    Role: inlined-context nodes (synth/draft/inlined review). Local => no remote egress. ──
[[agents]]
id          = "ollama"
kind        = "api"
base_url    = "http://localhost:11434/v1"
api_key_env = "OLLAMA_API_KEY"            # NAME of the env var; export it in the serve process env
model       = "qwen2.5-coder:7b"          # any installed `ollama list` model; adjust to taste

# ── Workflows: reuse the existing review/design lenses (../prompts). Design two-pass added in Phase D. ──
# (Copy the [[workflows]] code-review / spec-review / plan-review / design blocks verbatim from
#  examples/a2a-bridge.multi-agent.toml lines 64-138. They reference ../prompts/*.md and are agent-id
#  driven, so they run unchanged through the containerized agents.)
```

- [ ] **Step 2: Copy the four `[[workflows]]` blocks from the multi-agent reference**

Run: open `examples/a2a-bridge.multi-agent.toml`, copy the `code-review`, `spec-review`, `plan-review`, and `design` `[[workflows]]` blocks (verbatim) into the new file under the comment in Step 1. Verify the `agent =` ids are `codex`/`claude` (which now exist in this config).

- [ ] **Step 3: Validate the config parses** (run from a dir UNDER the mount root — see Task 0)

Run (from the repo root, which is under `/Users/wesleyjinks/code`): `cargo run -q -p a2a-bridge -- run-workflow design --input examples/sample-input.md --config examples/a2a-bridge.containerized.toml 2>&1 | head -5`
Expected: it begins running (nodes started) — proving the config + the agents parse and resolve. (Ctrl-C after the start lines; the real per-agent run is Task 8.) Note: omitting `[registry] allowed_cmds` would NOT error — the allowlist defaults to the union of entry `cmd`s (which includes `docker`); the explicit `allowed_cmds=["docker"]` is still desirable as a tightening, so keep it.

- [ ] **Step 4: Commit**

```bash
git add examples/a2a-bridge.containerized.toml
git commit -m "config: containerized :ro readers (claude+codex) + ollama api agent + cwd gate + allowed_cmds"
```

### Task 6: Per-agent credential copies (WRITABLE, single-file)

**Files:** none in-repo (host setup + runbook prose lands in Task 11).

- [ ] **Step 1: Create isolated, WRITABLE creds copies (NOT `:ro` — token refresh writes back)**

Run:
```bash
mkdir -p ~/.config/a2a-creds/claude ~/.config/a2a-creds/codex
cp ~/.claude/.credentials.json ~/.config/a2a-creds/claude/.credentials.json
cp ~/.codex/auth.json          ~/.config/a2a-creds/codex/auth.json
chmod -R u+rw ~/.config/a2a-creds
```
Expected: both files exist and are writable. These are mounted writable (no `:ro` in the config) so an in-container OAuth refresh updates the COPY, never the host's creds.

- [ ] **Step 2: Verify Docker Desktop can bind-mount the paths**

Run: `docker run --rm -v ~/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json a2a-agent-reader:latest sh -c 'test -s /root/.claude/.credentials.json && echo "creds mounted"'`
Expected: `creds mounted`. (If Docker Desktop file-sharing rejects `~/.config`, add it under Settings → Resources → File Sharing.)

- [ ] **Step 3: No commit** (host-only setup; documented in Task 11).

### Task 7: GATE — `:ro` integrity, falsifiable (validation gate 1)

**Files:** none (verification only).

- [ ] **Step 1: Run a dedicated probe container with the SAME source mount and inspect it WHILE RUNNING**

(Per dual-review: `docker inspect` after `--rm` finds nothing, and "a write fails" is too broad — only the repo mount is `:ro`, `/tmp`/`$HOME` are writable. So use a non-`--rm` probe + the mechanical Binds assertion + a write *to the repo path*.)

Run:
```bash
docker run -d --name a2a-roprobe \
  -v /Users/wesleyjinks/code:/Users/wesleyjinks/code:ro a2a-agent-reader:latest sleep 60
echo "--- Binds (must contain :ro for the repo) ---"
docker inspect a2a-roprobe --format '{{json .HostConfig.Binds}}'
echo "--- write to the REPO mount must fail ---"
docker exec a2a-roprobe sh -c 'echo x > /Users/wesleyjinks/code/__roprobe__ 2>&1 || echo "repo write blocked (expected)"'
echo "--- write to /tmp must succeed (sanity: not a fully-readonly box) ---"
docker exec a2a-roprobe sh -c 'echo x > /tmp/ok && echo "/tmp writable (expected)"'
docker rm -f a2a-roprobe
```
Expected: Binds JSON contains `…/code:…/code:ro`; `repo write blocked (expected)`; `/tmp writable (expected)`. The **Binds `:ro` assertion is the integrity proof.**

- [ ] **Step 2: Commit the gate result**

```bash
git commit --allow-empty -m "validate: :ro integrity PASS (repo bind :ro asserted; repo write blocked)"
```

### Task 8: GATE — per-AGENT auth smoke via SINGLE-agent workflows (validation gate 2)

> **Dual-review BLOCKER fix:** `design`/`code-review` start BOTH agents (both root lenses are
> `inputs=[]`, and the executor schedules every ready root), so they can NOT isolate one agent. Use
> one-node, one-agent **smoke** workflows instead. **Also (dual-review):** `run-workflow` drives the
> agent via the *static* `current_dir` cwd — NOT a per-request `session_cwd` — so this gate proves
> spawn + auth + `:ro` read + cwd-honoring + terminate, **not** the `allowed_cwd_root` gate (that's
> Task 10 via `serve`). **MUST run from the repo root** (a dir under the mount) so the `session/new`
> cwd exists inside the container.

**Files:** Create `prompts/smoke-read.md`, `prompts/smoke-reply.md`; add four one-node smoke workflows
to `examples/a2a-bridge.containerized.toml`.

- [ ] **Step 1: Write the smoke prompts**

```markdown
<!-- prompts/smoke-read.md -->
You are a READ-ONLY agent. Using read-only tools, list the top-level entries of the repository at your
current working directory, then output the list on one line prefixed with `SMOKE_OK:` and STOP. Do not
modify anything, run no builds, and make no network calls beyond your model.
```
```markdown
<!-- prompts/smoke-reply.md -->
Reply with exactly this line and nothing else: SMOKE_OK
```

- [ ] **Step 2: Add four single-agent smoke workflows to the containerized config**

```toml
[[workflows]]
id = "smoke-claude"
[[workflows.nodes]]
id = "go"
agent = "claude"
prompt_file = "../prompts/smoke-read.md"
inputs = []

[[workflows]]
id = "smoke-codex"
[[workflows.nodes]]
id = "go"
agent = "codex"
prompt_file = "../prompts/smoke-read.md"
inputs = []

[[workflows]]
id = "smoke-kiro"
[[workflows.nodes]]
id = "go"
agent = "kiro"
prompt_file = "../prompts/smoke-read.md"
inputs = []

[[workflows]]
id = "smoke-ollama"
[[workflows.nodes]]
id = "go"
agent = "ollama"
prompt_file = "../prompts/smoke-reply.md"
inputs = []
```

- [ ] **Step 3: claude smoke (the proven path) — RUN FROM THE REPO ROOT**

Run (cwd = `/Users/wesleyjinks/code/a2a-bridge`, under the mount):
```bash
cargo run -q -p a2a-bridge -- run-workflow smoke-claude --input /dev/null \
  --out /tmp/smoke-claude.txt --config examples/a2a-bridge.containerized.toml
echo "EXIT=$?"; grep -o 'SMOKE_OK.*' /tmp/smoke-claude.txt | head -1
```
Expected: `SMOKE_OK: <repo top-level files>`, EXIT=0 → claude authenticates through the proxy in-box,
reads the repo `:ro`, honors the ACP session cwd, terminates. (If `--input /dev/null` is rejected,
point `--input` at any tiny file, e.g. `README.md`; the smoke prompt ignores the input.)

- [ ] **Step 4: codex smoke — retire its in-box auth + `*.openai.com` egress**

Run: same as Step 3 with `smoke-codex` → `/tmp/smoke-codex.txt`. Expected `SMOKE_OK: …`. **If codex
fails auth/proxy-honoring:** record it, drop to claude-only, note the failing assumption in ADR-0016.

- [ ] **Step 5: ollama smoke — retire the api-agent reachability (dual-review gap)**

Ensure `OLLAMA_API_KEY` is exported in the serve/run env and a model is pulled (`ollama list`; match
`model =` in the config). Run: same as Step 3 with `smoke-ollama` → expect `SMOKE_OK`. **If no local
model is available**, record ollama as *config-only / unvalidated* in ADR-0016 (do not block).

- [ ] **Step 6: Confirm egress went through the lock**

Run: `docker logs a2a-egress-proxy 2>&1 | grep -iE "connect|deny|filter" | tail -20`
Expected: CONNECT to `*.anthropic.com` / `*.openai.com` only; nothing off-allowlist.

- [ ] **Step 7: Commit the gate result (per agent)**

```bash
git add prompts/smoke-read.md prompts/smoke-reply.md examples/a2a-bridge.containerized.toml
git commit -m "validate: per-agent auth smokes (single-agent workflows) — record claude/codex/ollama outcomes"
```

### Task 9: kiro validation — device-flow login (once) + egress allowlist discovery

(kiro is already in the image (Task 1) and config (Task 5). This task retires its two unknowns:
device-flow auth into a persistent volume, and which egress hosts it needs. **VERIFIED LIVE
2026-06-04** — the steps below are confirmed working, not hypothetical.)

**Files:** modify `deploy/containers/tinyproxy.filter` (add kiro's discovered hosts).

- [ ] **Step 1: One-time device-flow login, persisted to the `a2a-kiro-data` volume**

The host's macOS kiro auth state is **NOT** portable to Linux (kiro tries to open a browser and fails),
so the auth must be minted in-container. Use **`--use-device-flow`** (prints a URL+code; no browser):
```bash
docker volume create a2a-kiro-data >/dev/null
docker run -it --rm -v a2a-kiro-data:/root/.local/share \
  a2a-agent-reader:latest kiro-cli login --use-device-flow
```
Pick a sign-in method, open the printed URL, enter the one-time code. **VERIFIED:** kiro writes its
Linux auth state to `~/.local/share/kiro-cli/data.sqlite3` (a sqlite DB — **NOT** `~/.aws`), which the
volume persists; the token refreshes there on use. Login on a **plain network** (full egress) is
simplest — the egress-locked path is exercised by the Step 3 smoke. *(The agent entry in Task 5 mounts
exactly `a2a-kiro-data:/root/.local/share`.)*

- [ ] **Step 2: Verify auth from the volume (no browser) before wiring the bridge**

```bash
echo "Reply with exactly: KIRO_OK" | docker run -i --rm \
  -v a2a-kiro-data:/root/.local/share \
  a2a-agent-reader:latest kiro-cli chat --no-interactive --wrap never
```
Expected: a `KIRO_OK` reply (VERIFIED). If it instead prints "Opening browser…", the volume is empty /
mis-mounted — re-do Step 1.

- [ ] **Step 3: kiro through the egress-locked bridge — discover + pin EXACT hosts**

Run the single-agent kiro smoke (from the repo root, through the proxy):
```bash
cargo run -q -p a2a-bridge -- run-workflow smoke-kiro --input README.md \
  --out /tmp/smoke-kiro.txt --config examples/a2a-bridge.containerized.toml
echo "EXIT=$?"; grep -o 'SMOKE_OK.*' /tmp/smoke-kiro.txt | head -1
docker logs a2a-egress-proxy 2>&1 | grep -iE "deny|filter|connect" | tail -30
```
If it's blocked, the proxy log names the **exact** denied hosts. Append them as **anchored,
host-specific ERE** regexes (NOT the broad `(^|\.)amazonaws\.com$`, which would permit *any* AWS
service — dual-review MAJOR) to `deploy/containers/tinyproxy.filter`, e.g. the specific Q / OIDC hosts
seen in the log:
```text
# examples — replace with the EXACT hosts from the proxy log:
(^|\.)q\.us-east-1\.amazonaws\.com$
(^|\.)oidc\.us-east-1\.amazonaws\.com$
(^|\.)codewhisperer\.us-east-1\.amazonaws\.com$
```
Then `docker compose -f deploy/containers/compose.egress.yaml up -d --build` and re-run until the kiro
smoke prints `SMOKE_OK`.

- [ ] **Step 4: Decision + commit**

If kiro completes → commit the pinned allowlist:
```bash
git add deploy/containers/tinyproxy.filter
git commit -m "containers: pin EXACT kiro egress hosts (Q/OIDC) from proxy-log discovery"
```
**If kiro's egress won't close to specific hosts, or auth won't hold** → drop to the claude+codex core
(kiro is used primarily at work), revert the kiro allowlist lines + the kiro agent entry, and record the
failed assumption in ADR-0016. **Do not block the increment on kiro.**

### Task 10: GATE — cwd gate + multi-repo (validation gates 4 + 5)

**Files:** none (verification only).

- [ ] **Step 1: Per-request cwd OUTSIDE the mount root is rejected**

Run (serve in one shell, request in another):
```bash
# shell 1
cargo run -q -p a2a-bridge -- serve --config examples/a2a-bridge.containerized.toml &
SERVE=$!; sleep 3
# shell 2 — A2A method is CamelCase "SendMessage", needs the `A2A-Version: 1.0` header, and the
# cwd metadata lives under message.metadata (NOT top-level params.metadata). cwd OUTSIDE the root rejects:
curl -sS -X POST http://127.0.0.1:8080/ -H 'content-type: application/json' -H 'A2A-Version: 1.0' -d '{
  "jsonrpc":"2.0","id":1,"method":"SendMessage",
  "params":{"message":{"role":"user","parts":[{"kind":"text","text":"hi"}],
                       "metadata":{"a2a-bridge.cwd":"/etc"}}}}'
```
Expected (VERIFIED): `{"error":{"code":-32600,"message":"invalid request: a2a-bridge.cwd"}}` — `/etc`
is outside `/Users/wesleyjinks/code`, rejected before any work. A cwd UNDER the root (e.g.
`/Users/wesleyjinks/code/a2a-bridge`) is **accepted** and the containerized agent reads that repo. Kill
serve: `kill $SERVE`.

- [ ] **Step 2: Multi-repo — a second repo under the mount resolves with the same serve**

Run: with `serve` up, send an **A2A `SendMessage`** (NOT `run-workflow` — only the serve path threads
`session_cwd` into the gate + the agent mint), with the cwd under `message.metadata`:
```bash
curl -sS --max-time 90 -X POST http://127.0.0.1:8080/ -H 'content-type: application/json' -H 'A2A-Version: 1.0' -d '{
  "jsonrpc":"2.0","id":2,"method":"SendMessage",
  "params":{"message":{"role":"user","parts":[{"kind":"text","text":"List the top-level files here and STOP."}],
                       "metadata":{"a2a-bridge.cwd":"/Users/wesleyjinks/code/<other-repo>"}}}}'
```
Expected (VERIFIED for this repo): the containerized agent reads the repo at that `session_cwd` (one
broad `:ro` mount, many repos). Kill serve when done.

- [ ] **Step 3: Commit the gate result**

```bash
git commit --allow-empty -m "validate: cwd gate PASS (outside-root rejected) + multi-repo under one mount"
```

---

## Phase D — Two-pass refine (separable sub-slice: design / spec-review / plan-review)

> Mechanism: keep each existing clean-room lens as the **draft** (a new node id, `inputs=[]`, reusing
> the existing lens prompt), add a **refine** node that keeps the original lens id and takes
> `inputs=[<its own draft>]` (firewall preserved — a refiner sees only its OWN draft), so the **synth
> is unchanged** (`inputs=[<lens>, <lens>]`). Prompt vars = input node ids (proven: synth already uses
> `{{correctness}}` etc.). Applied to `design`, `spec-review`, `plan-review` only — NOT `code-review`.

### Task 11: Two-pass `design`

**Files:**
- Create: `prompts/design-executability-refine.md`, `prompts/design-structure-refine.md`
- Modify: `examples/a2a-bridge.multi-agent.toml` + `examples/a2a-bridge.containerized.toml` (the `design` workflow)

- [ ] **Step 1: Write the executability refine prompt**

```markdown
<!-- prompts/design-executability-refine.md -->
You are the SAME independent senior software ARCHITECT with a PRAGMATIC / EXECUTABILITY lens. Below is
YOUR OWN first-pass design (a draft). Do a rigorous SECOND PASS.

This is still a CLEAN-ROOM design: you are NOT shown any other architect's work.

READ-ONLY + BOUNDED CONTRACT — follow exactly:
- You MAY use READ-ONLY tools (read/list/grep, `git diff`/`log`/`show`) to VERIFY your draft against
  the ACTUAL code. No edits/writes/builds/network beyond read-only git/search.
- Explore enough to verify, then STOP and write. Respond with plain text directly.

PRODUCE, in order:
1. **GAPS / UNCERTAINTIES REGISTER** — a short list of what in your draft is unverified, underspecified,
   or risky (cite path:line where you checked). Be honest; this drives the refinement.
2. **REFINED DESIGN** — your draft, deepened: close the gaps above, correct anything the code
   contradicts, tighten interfaces/flow. Keep the executability lens.

YOUR FIRST-PASS DRAFT:
{{executabilitydraft}}
```

- [ ] **Step 2: Write the structure refine prompt**

```markdown
<!-- prompts/design-structure-refine.md -->
You are the SAME independent senior software ARCHITECT with a STRUCTURE / SEAM lens. Below is YOUR OWN
first-pass design (a draft). Do a rigorous SECOND PASS.

This is still a CLEAN-ROOM design: you are NOT shown any other architect's work.

READ-ONLY + BOUNDED CONTRACT — follow exactly:
- You MAY use READ-ONLY tools (read/list/grep, `git diff`/`log`/`show`) to VERIFY your draft against
  the ACTUAL code. No edits/writes/builds/network beyond read-only git/search.
- Explore enough to verify, then STOP and write. Respond with plain text directly.

PRODUCE, in order:
1. **GAPS / UNCERTAINTIES REGISTER** — what in your draft is unverified, underspecified, or risky
   (cite path:line). Be honest; this drives the refinement.
2. **REFINED DESIGN** — your draft, deepened: close the gaps, correct anything the code contradicts,
   tighten the seams/boundaries. Keep the structure lens.

YOUR FIRST-PASS DRAFT:
{{structuredraft}}
```

- [ ] **Step 3: Restructure the `design` workflow (both example configs)**

In `examples/a2a-bridge.multi-agent.toml` AND `examples/a2a-bridge.containerized.toml`, replace the `design` workflow's nodes with:
```toml
[[workflows]]
id = "design"
# draft = clean-room (reuse existing lens prompts)
[[workflows.nodes]]
id = "executabilitydraft"
agent = "codex"
prompt_file = "../prompts/design-executability.md"
inputs = []
[[workflows.nodes]]
id = "structuredraft"
agent = "claude"
prompt_file = "../prompts/design-structure.md"
inputs = []
# refine = second pass on OWN draft (firewall preserved)
[[workflows.nodes]]
id = "executability"
agent = "codex"
prompt_file = "../prompts/design-executability-refine.md"
inputs = ["executabilitydraft"]
[[workflows.nodes]]
id = "structure"
agent = "claude"
prompt_file = "../prompts/design-structure-refine.md"
inputs = ["structuredraft"]
# synth UNCHANGED (consumes the refined lenses)
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "../prompts/design-synth.md"
inputs = ["executability", "structure"]
```
(In `multi-agent.toml` the prompt paths are `../prompts/…`; keep that. The containerized config uses the same `../prompts/…`.)

- [ ] **Step 4: Verify the two-pass design runs + the firewall holds**

Run: `cargo run -q -p a2a-bridge -- run-workflow design --input examples/sample-input.md --out /tmp/twopass.md --config examples/a2a-bridge.multi-agent.toml`
Expected: nodes run in order draft → refine → synth; `/tmp/twopass.md` contains a synthesized design; each refine output starts with a GAPS register. Confirm no refine node declares the peer's draft in `inputs` (firewall).

- [ ] **Step 5: Commit**

```bash
git add prompts/design-executability-refine.md prompts/design-structure-refine.md examples/a2a-bridge.multi-agent.toml examples/a2a-bridge.containerized.toml
git commit -m "workflow: two-pass design (clean-room draft -> grounded refine + gaps register) -> synth"
```

### Task 12: Two-pass `spec-review` and `plan-review`

**Files:**
- Create: `prompts/spec-review-rigor-refine.md`, `prompts/spec-review-soundness-refine.md`, `prompts/plan-review-exec-refine.md`, `prompts/plan-review-coverage-refine.md`
- Modify: both example configs (the `spec-review` + `plan-review` workflows)

- [ ] **Step 1: Write the four refine prompts (mirror Task 11's structure, per lens)**

Each file follows the SAME shape as `design-*-refine.md`: "you are the SAME reviewer with the <lens>;
here is YOUR draft; produce a GAPS/UNCERTAINTIES register then a REFINED review grounded in the code."
The draft var is the draft node id:
- `prompts/spec-review-rigor-refine.md` → ends with `YOUR FIRST-PASS DRAFT:\n{{rigordraft}}` (lens: completeness/ambiguity).
- `prompts/spec-review-soundness-refine.md` → `{{soundnessdraft}}` (lens: design soundness).
- `prompts/plan-review-exec-refine.md` → `{{execdraft}}` (lens: compile/ordering/ripple).
- `prompts/plan-review-coverage-refine.md` → `{{coveragedraft}}` (lens: spec coverage/decomposition).

Write the full prose for each (copy the design refine template, swap the lens sentence + the draft var).

- [ ] **Step 2: Restructure `spec-review` (both configs)**

```toml
[[workflows]]
id = "spec-review"
[[workflows.nodes]]
id = "rigordraft"
agent = "codex"
prompt_file = "../prompts/spec-review-rigor.md"
inputs = []
[[workflows.nodes]]
id = "soundnessdraft"
agent = "claude"
prompt_file = "../prompts/spec-review-soundness.md"
inputs = []
[[workflows.nodes]]
id = "rigor"
agent = "codex"
prompt_file = "../prompts/spec-review-rigor-refine.md"
inputs = ["rigordraft"]
[[workflows.nodes]]
id = "soundness"
agent = "claude"
prompt_file = "../prompts/spec-review-soundness-refine.md"
inputs = ["soundnessdraft"]
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "../prompts/spec-review-synth.md"
inputs = ["rigor", "soundness"]
```

- [ ] **Step 3: Restructure `plan-review` (both configs)** — same pattern with `execdraft`/`coveragedraft` → `exec`/`coverage` → `synth` (`inputs=["exec","coverage"]`).

- [ ] **Step 4: Verify both run**

Run: `cargo run -q -p a2a-bridge -- run-workflow spec-review --input docs/superpowers/specs/2026-06-04-containerized-agents-design.md --config examples/a2a-bridge.multi-agent.toml 2>&1 | tail -5` (and the same for `plan-review` against this plan). Expected: draft → refine → synth completes for each.

- [ ] **Step 5: Commit**

```bash
git add prompts/spec-review-*-refine.md prompts/plan-review-*-refine.md examples/a2a-bridge.multi-agent.toml examples/a2a-bridge.containerized.toml
git commit -m "workflow: two-pass spec-review + plan-review (draft -> grounded refine + gaps) -> synth"
```

---

## Phase E — Docs + ADR

### Task 13: Operator runbook

**Files:**
- Create: `docs/containerized-agents.md`

- [ ] **Step 1: Write the runbook** — covering, in order: (1) build the reader image; (2)
  `docker compose … up -d --build` the egress; (3) creds per agent — copy WRITABLE single-file creds
  into `~/.config/a2a-creds/<agent>` for claude/codex, and the **one-time `kiro-cli login --use-device-flow`
  into the `a2a-kiro-data` volume** (`~/.local/share/kiro-cli/data.sqlite3`) for kiro; (4)
  `serve --config examples/a2a-bridge.containerized.toml`; (5) the validation gates as copy-paste blocks
  (egress triad, `:ro` Binds probe, per-agent single-agent smokes, cwd gate via serve, multi-repo via
  serve); (6) the proxy-log allowlist-discovery method (pin EXACT hosts, not broad globs); (7) the
  claude-only fallback + the four unproven assumptions; (8) macOS Docker Desktop notes (file-sharing for
  `~/.config`, bind-mount latency) and the rootless-podman-on-Linux production note. **Two load-bearing
  rules to state explicitly:** (a) `run-workflow` uses the *static* cwd — **always run it from a dir
  under `allowed_cwd_root`** (else the `session/new` cwd won't exist in the container); the cwd *gate*
  is only enforced on the `serve`+A2A path. (b) **ollama**: only **local** `base_url` has no remote
  egress — an ollama-*cloud* `base_url` egresses host-direct (bypasses the proxy). Pull exact commands
  from Tasks 0–12 (DRY — reference, don't reinvent).

- [ ] **Step 2: Commit**

```bash
git add docs/containerized-agents.md
git commit -m "docs: containerized-agents operator runbook (build, egress, creds, the five gates)"
```

### Task 14: ADR-0016 + final self-review

**Files:**
- Create: `docs/adr/0016-containerized-agents-slice-a.md`

- [ ] **Step 1: Write ADR-0016** — Context (the R1 finding: `:ro` is the only hard read-only
  guarantee; agent CLIs can't be flag-restricted); Decision (Slice A = config-only containerized `:ro`
  readers + egress lock + the uncontainerized api agent; amends ADR-0013's "config-only" toward the
  Slice B enforced `[sandbox]` block); Evidence (the per-agent **single-agent smoke** outcomes — which
  agents validated, which fell back; **kiro auth VERIFIED via `~/.local/share/kiro-cli/data.sqlite3`**);
  Consequences (claude+codex+kiro containerized; **ollama local-only — validated-or-recorded-unvalidated,
  and note cloud `base_url` is host-direct egress**; the cwd gate is enforced only on the serve path;
  the four unproven assumptions retired-or-recorded). Carry the `Co-Authored-By` trailer.

- [ ] **Step 2: Commit**

```bash
git add docs/adr/0016-containerized-agents-slice-a.md
git commit -m "$(printf 'docs: ADR-0016 containerized agents (Slice A)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Self-Review (run by the plan author before handoff)

**Spec coverage:** A1 image → T1; A2 egress → T2/T3/T4; A3 config (allowed_cwd_root, allowed_cmds,
identical-path, writable creds) → T5/T6; A4/A4b creds + ollama → T5/T6; A5 two-pass → T11/T12; A6 gates
(falsifiable) → T4/T7/T8/T10; A7 DoD + risks (claude-first, four unproven assumptions, kiro) → T8/T9 +
ADR T14; runbook → T13; ADR → T14. **All dual-review must-fixes** (gate set + allowed_cwd_root +
writable creds + ERE allowlist + tools-off wording) are baked into T5/T2/T7/T8. Covered.

**Placeholder scan:** the kiro installer is concrete (`curl …/install`, T1). The only intentional
"discover during the task" steps are kiro's exact egress hosts (T9 — pinned empirically from the proxy
log) and the ollama model name (T5 — `ollama list` picks it). No silent TBDs.

**Consistency:** image tag `a2a-agent-reader:latest`, proxy `a2a-egress-proxy:8888`, networks
`a2a-egress-internal`/`a2a-egress-external`, claude/codex creds `~/.config/a2a-creds/<agent>`, kiro
auth volume `a2a-kiro-data:/root/.local/share`, node-id convention `<lens>draft` → `<lens>` → `synth`
are used identically across all tasks.

## Dual-review fold (Codex gpt-5.5 + Claude opus-4-8, against this plan + the real code)

Both reviewers **confirmed the zero-Rust premise and the two-pass refine shape** against the code;
neither found architectural rework. Folded fixes:
- **BLOCKER (Codex):** `design`/`code-review` start *both* agents → can't isolate one → **Task 8 now
  uses single-agent smoke workflows.**
- **BLOCKER (Codex) — kiro auth:** resolved **LIVE** — Linux auth is `~/.local/share/kiro-cli/data.sqlite3`
  (not `~/.aws`), via `kiro-cli login --use-device-flow` → the `a2a-kiro-data` volume (Tasks 5/9).
- **Both — `run-workflow` ≠ cwd gate:** it uses the static `current_dir`, so Task 8/9 are spawn/auth
  smokes (run from under the mount); the cwd gate stays on `serve` (Task 10, fixed multi-repo too);
  runbook states the rule.
- **Both — ollama unvalidated:** Task 8 Step 5 adds an ollama smoke; ADR records unvalidated if no
  local model. "tools-off" → "side-effect-free stub tool"; cloud-egress caveat kept.
- **Codex — kiro egress too broad:** Task 9 pins **exact** hosts from the proxy log, not `amazonaws.com`.
- **Claude — input file gap:** **Task 0** authors `examples/sample-input.md`. Task 5 Step 3 diagnostic
  corrected (`allowed_cmds` defaults to the union incl. docker). Containerfile drift reconciled
  (committed, `--force --no-confirm`).

---

## Execution Handoff

The plan's **Codex + Claude dual-review is DONE and folded** (above). Task 0/1 are already executed
ahead of time (the reader image is built + the kiro login verified) to unblock the human-gated steps.

**Two execution options:**
1. **Subagent-Driven (recommended)** — fresh subagent per task + two-stage review (spec-compliance,
   then quality). Note: the validation GATES (T4/T7/T8/T9/T10) need a human-in-the-loop with Docker +
   live creds — those tasks are operator-run, not subagent-automated.
2. **Inline Execution** — execute in this session with checkpoints.
