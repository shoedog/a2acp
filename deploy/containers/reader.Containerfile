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
      @agentclientprotocol/claude-agent-acp@0.81.2 \
      @agentclientprotocol/codex-acp@1.13.1 \
    && npm install \
      --prefix /usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp \
      --omit=dev --no-save --package-lock=false \
      @anthropic-ai/claude-agent-sdk@0.3.280 \
    && npm install \
      --prefix /usr/local/lib/node_modules/@agentclientprotocol/codex-acp \
      --omit=dev --no-save --package-lock=false \
      @openai/codex@0.157.1

# R3b: a pinned compatibility canary must bind the package identities inside the immutable image,
# not guess from the host. Fail the build if npm resolved different transitive agent packages, then
# publish only these non-secret exact identities as image metadata for bounded `image inspect`.
RUN test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json').version")" = "1.13.1" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/node_modules/@openai/codex/package.json').version")" = "0.157.1" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/package.json').version")" = "0.81.2" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai/claude-agent-sdk/package.json').version")" = "0.3.280" \
    && test "$(node -p "require('/usr/local/lib/node_modules/@agentclientprotocol/claude-agent-acp/node_modules/@anthropic-ai/claude-agent-sdk/package.json').claudeCodeVersion")" = "2.1.280"

LABEL io.a2a-bridge.provenance.codex.adapter="@agentclientprotocol/codex-acp=1.13.1" \
      io.a2a-bridge.provenance.codex.agent-cli="@openai/codex=0.157.1" \
      io.a2a-bridge.provenance.claude.adapter="@agentclientprotocol/claude-agent-acp=0.81.2" \
      io.a2a-bridge.provenance.claude.agent-cli="@anthropic-ai/claude-agent-sdk=0.3.280" \
      io.a2a-bridge.provenance.kiro.agent-cli="kiro-cli=2.24.1"

# kiro-cli: install the LINUX build (the host's macOS binary can't run in this Linux image). Official
# zip method (https://kiro.dev/docs/cli/installation/#with-a-zip-file); arch-aware so it works whether
# Docker Desktop runs amd64 or arm64 (Apple Silicon -> arm64). Use the MUSL build: kiro-cli's current
# GNU release requires glibc 2.39, but node:24-slim is bookworm/glibc 2.36 — the GNU build now fails at
# install.sh ("built for a GNU system with glibc 2.39 or newer, try the musl version"). The musl build is
# statically linked (no glibc dep) and runs on the bookworm base. install.sh drops the binary under
# ~/.local/bin (root -> /root/.local/bin). The version and per-architecture digests come from Kiro's
# official stable manifest; do not replace the versioned URL with the mutable `latest` channel.
ARG KIRO_CLI_VERSION=2.24.1
ARG KIRO_CLI_AMD64_SHA256=0187d8f613b4ad6b63f7fe069a187c33c79664ee9874ec5848faa7dc8c001ef9
ARG KIRO_CLI_ARM64_SHA256=95e149b0b5e2be3c6f56d4e5ed1bc5313d2596ef39255529a553027fd87abff5
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
