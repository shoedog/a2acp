# a2a-bridge reader image: portable ACP agent CLIs + read-only exploration tools. NO build toolchain
# (readers verify via read/grep/git diff; they don't compile — that's the Slice B implement image).
FROM docker.io/library/node:24-slim

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
# Docker Desktop runs amd64 or arm64 (Apple Silicon -> arm64). node:24-slim is bookworm/glibc 2.36
# (>= 2.34 required). install.sh drops the binary under ~/.local/bin (root -> /root/.local/bin).
# </dev/null keeps install.sh non-interactive during the build (no hang on a prompt).
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
ENV PATH="/root/.local/bin:${PATH}"

# Workdir is cosmetic: the ACP session cwd arrives over the protocol (session/new); the repo is
# bind-mounted at its identical host path at run time.
WORKDIR /work
