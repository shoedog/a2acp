# a2a-bridge reader image: portable ACP agent CLIs + read-only exploration tools. NO build toolchain
# (readers verify via read/grep/git diff; they don't compile — that's the Slice B implement image).
FROM docker.io/library/node:24-slim

# Read tools the review/design lenses use, + curl for the egress gate + the kiro installer,
# + unzip/ca-certificates for installers, + git/ripgrep for read/grep.
RUN apt-get update && apt-get install -y --no-install-recommends \
      git ripgrep ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

# Pin the ACP agent CLIs (portable Node packages; provider-free ACP compatibility verified 2026-09-03).
# claude-agent-acp pulls @anthropic-ai/claude-agent-sdk, whose optional dep is the platform `claude`
# binary — the LINUX build resolves here, not the host's macOS one.
RUN npm install -g \
      @agentclientprotocol/claude-agent-acp@0.73.0 \
      @agentclientprotocol/codex-acp@1.8.0 \
    && npm install \
      --prefix /usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp \
      --omit=dev --no-save --package-lock=false \
      @anthropic-ai/claude-agent-sdk@0.3.257 \
    && npm install \
      --prefix /usr/local/lib/node_modules/@agentclientprotocol/codex-acp \
      --omit=dev --no-save --package-lock=false \
      @openai/codex@0.153.0

# R3b: a pinned compatibility canary must bind the package identities inside the immutable image,
# not guess from the host. Fail the build if npm resolved different transitive agent packages, then
# publish only these non-secret exact identities as image metadata for bounded `image inspect`.
RUN test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json').version")" = "1.8.0" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/node_modules/@openai/codex/package.json').version")" = "0.153.0" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/package.json').version")" = "0.73.0" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai/claude-agent-sdk/package.json').version")" = "0.3.257" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai/claude-agent-sdk/package.json').claudeCodeVersion")" = "2.1.257"

LABEL io.a2a-bridge.provenance.codex.adapter="@agentclientprotocol/codex-acp=1.8.0" \
      io.a2a-bridge.provenance.codex.agent-cli="@openai/codex=0.153.0" \
      io.a2a-bridge.provenance.claude.adapter="@agentclientprotocol/claude-agent-acp=0.73.0" \
      io.a2a-bridge.provenance.claude.agent-cli="@anthropic-ai/claude-agent-sdk=0.3.257" \
      io.a2a-bridge.provenance.kiro.agent-cli="kiro-cli=2.21.0"

# kiro-cli: install the LINUX build (the host's macOS binary can't run in this Linux image). Official
# zip method (https://kiro.dev/docs/cli/installation/#with-a-zip-file); arch-aware so it works whether
# Docker Desktop runs amd64 or arm64 (Apple Silicon -> arm64). Use the MUSL build: kiro-cli's current
# GNU release requires glibc 2.39, but node:24-slim is bookworm/glibc 2.36 — the GNU build now fails at
# install.sh ("built for a GNU system with glibc 2.39 or newer, try the musl version"). The musl build is
# statically linked (no glibc dep) and runs on the bookworm base. install.sh drops the binary under
# ~/.local/bin (root -> /root/.local/bin). The version and per-architecture digests come from Kiro's
# official stable manifest; do not replace the versioned URL with the mutable `latest` channel.
ARG KIRO_CLI_VERSION=2.21.0
ARG KIRO_CLI_AMD64_SHA256=9dade2b24424e5740b55c7b71a0d8f6b57193277bd03383042a2334421f77267
ARG KIRO_CLI_ARM64_SHA256=f4dd3b1ee1f0cc790bbc9449b2fa43871d3130956a2afa5bdeb7b19b2cc88e6c
RUN set -eux; \
    case "$(dpkg --print-architecture)" in \
      amd64) archive="kirocli-x86_64-linux-musl.zip"; sha256="${KIRO_CLI_AMD64_SHA256}" ;; \
      arm64) archive="kirocli-aarch64-linux-musl.zip"; sha256="${KIRO_CLI_ARM64_SHA256}" ;; \
      *) echo "unsupported arch" >&2; exit 1 ;; \
    esac; \
    url="https://prod.download.cli.kiro.dev/stable/${KIRO_CLI_VERSION}/${archive}"; \
    curl --proto '=https' --tlsv1.2 -sSf "$url" -o /tmp/kirocli.zip; \
    echo "${sha256}  /tmp/kirocli.zip" | sha256sum -c -; \
    unzip -q /tmp/kirocli.zip -d /tmp; \
    /tmp/kirocli/install.sh --force --no-confirm; \
    test "$(/root/.local/bin/kiro-cli --version)" = "kiro-cli ${KIRO_CLI_VERSION}"; \
    rm -rf /tmp/kirocli /tmp/kirocli.zip
ENV PATH="/root/.local/bin:${PATH}"

# Workdir is cosmetic: the ACP session cwd arrives over the protocol (session/new); the repo is
# bind-mounted at its identical host path at run time.
WORKDIR /work
