# a2a-bridge toolchain image (Slice B2b-2 + L3 Slice B): the reader image (ACP CLIs) + the Rust build
# toolchain, so the `impl` agent can build/test, the bridge can run a deterministic verify, AND
# rust-analyzer + lsp-mcp run in-container for live semantic nav. Used by `a2a-bridge implement`.
# NOT for the :ro reader agents (they don't compile).
#
# BUILD CONTEXT = repo ROOT (so the lspbuild stage can compile crates/lsp-mcp from the workspace):
#   docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .
# The repo-root `.dockerignore` excludes target/ (99G) etc. — without it the context upload is catastrophic.

# ── Builder: compile the Linux lsp-mcp binary from the workspace (L3 Slice B). ──
FROM a2a-agent-reader:latest AS lspbuild
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --default-toolchain 1.94.0 --profile minimal
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p lsp-mcp && cp target/release/lsp-mcp /lsp-mcp

# ADR-0040 requires `git merge-tree --write-tree --merge-base=<base>`. Debian bookworm's Git 2.39
# lacks the explicit merge-base option, which makes the implementor/verifier's merge tests fail for an
# image-only reason. Build the reviewed-capable Git release separately and copy only its installed tree.
FROM a2a-agent-reader:latest AS gitbuild
ARG GIT_VERSION=2.54.0
ARG GIT_SHA256=f689162364c10de79ef89aa8dbf48731eb057e34edbbd20aca510ce0154681a3
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential gettext libcurl4-gnutls-dev libexpat1-dev libssl-dev xz-utils zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSfL \
      "https://mirrors.edge.kernel.org/pub/software/scm/git/git-${GIT_VERSION}.tar.xz" \
      -o /tmp/git.tar.xz \
 && echo "${GIT_SHA256}  /tmp/git.tar.xz" | sha256sum -c - \
 && tar -C /tmp -xJf /tmp/git.tar.xz \
 && make -C "/tmp/git-${GIT_VERSION}" -j2 prefix=/opt/git NO_TCLTK=YesPlease all \
 && make -C "/tmp/git-${GIT_VERSION}" prefix=/opt/git NO_TCLTK=YesPlease install

# ── Final toolchain image ──
FROM a2a-agent-reader:latest

# Native build deps node:24-slim (debian bookworm) lacks: a C toolchain + linker for cargo's codegen.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust pinned to the repo's rust-toolchain.toml channel (1.94.0) + the components CI uses.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --default-toolchain 1.94.0 --profile minimal \
        --component rustfmt --component clippy --component llvm-tools-preview

# Coverage tools available so an opt-in `cargo llvm-cov` command never hits "command not found".
# Pinned for reproducibility (chosen against the 1.94.0 toolchain).
RUN cargo install --locked cargo-llvm-cov --version 0.6.21 \
 && cargo install --locked cargo-tarpaulin --version 0.32.7

# L3 Slice B: rust-analyzer (semantic nav) + rust-src (RA needs it to resolve std/core types — spike
# finding 2026-06-14; without rust-src RA logs "can't load standard library"). Added as its own layer
# so the slow apt/rustup/cargo-install layers above stay cached.
RUN rustup component add rust-analyzer rust-src

# Login shells reset PATH through Debian's /etc/profile and drop /usr/local/cargo/bin. Codex can invoke
# commands through a login shell, so expose real stable paths exactly as the Go/Python tools below do.
# Include cargo's installed subcommands: finding `cargo` alone is insufficient for `cargo clippy` or
# opt-in coverage when their companion executables are absent from the login-shell PATH.
RUN set -eux; for t in cargo rustc rustfmt rustup clippy-driver cargo-clippy rust-analyzer \
      cargo-llvm-cov cargo-tarpaulin; do \
      ln -sf "/usr/local/cargo/bin/$t" "/usr/local/bin/$t"; \
    done

# L3 Slice B: the in-container lsp-mcp shim (built in the lspbuild stage), delivered to the impl agent
# via CodexNative (`-c mcp_servers.lsp.command=/usr/local/bin/lsp-mcp`).
COPY --from=lspbuild /lsp-mcp /usr/local/bin/lsp-mcp

# C2a Step 2b: Go toolchain + gopls so the impl agent can edit/build/test Go, the bridge can run a
# deterministic Go verify, and gopls runs in-container for live nav. Pinned for reproducibility; its own
# layer so the slow Rust layers above stay cached. GOTOOLCHAIN=local prevents per-repo toolchain drift.
ENV GO_VERSION=1.23.4
RUN curl --proto '=https' --tlsv1.2 -sSfL "https://go.dev/dl/go${GO_VERSION}.linux-$(dpkg --print-architecture).tar.gz" \
      -o /tmp/go.tgz \
 && tar -C /usr/local -xzf /tmp/go.tgz && rm /tmp/go.tgz
ENV PATH=/usr/local/go/bin:/root/go/bin:$PATH GOTOOLCHAIN=local
RUN go install golang.org/x/tools/gopls@v0.17.1
# Symlink into /usr/local/bin (on EVERY shell's PATH, login + non-login) so the impl agent's go calls
# and gopls resolve even under a login shell that resets PATH via /etc/profile (the ENV PATH above only
# covers non-login execs like the bridge verify).
RUN ln -sf /usr/local/go/bin/go /usr/local/bin/go \
 && ln -sf /usr/local/go/bin/gofmt /usr/local/bin/gofmt \
 && ln -sf /root/go/bin/gopls /usr/local/bin/gopls

# Python (LSP-MCP polyglot slice): mise-provisioned python + uv + ruff. The REAL binaries are SYMLINKED
# into /usr/local/bin (already on every PATH incl. codex's stripped MCP-subprocess PATH) — NEVER mise
# shims/activation: a shim resolves the tool version from mise's env, which the stripped env drops → the
# exact #1d trap (see docs/containerized-mcp-env-trap.md). basedpyright is installed globally with npm in
# the JS/TS layer below: mise 2026.7.15 changed npm tools to location-dependent aube shims, which break when
# relocated through /usr/local/bin. node (the image base) backs basedpyright-langserver.
RUN curl -fsSL https://mise.run | sh
ENV PATH=/root/.local/bin:$PATH
RUN /root/.local/bin/mise use -g -y python@3.12.13 uv@0.11.21 ruff@0.15.17
# Symlink the RESOLVED real binaries (NOT the shims dir). Python exposes `python`+`python3`.
RUN set -eux; for t in python python3 uv ruff; do \
      ln -sf "$(/root/.local/bin/mise which "$t")" "/usr/local/bin/$t"; \
    done

# Node LSP CLIs (LSP-MCP polyglot slice): basedpyright + typescript-language-server + typescript via
# `npm install -g` — NOT mise.
# mise installs each npm package in an ISOLATED dir, so typescript-language-server cannot find `typescript`
# as a sibling (tsserver discovery fails). `npm install -g` co-locates them in /usr/local/lib/node_modules
# (siblings → tsls auto-discovers tsserver) AND puts REAL binaries on /usr/local/bin (env-trap compliant,
# no shims — validated: tsls --stdio responds to `initialize` under a fully stripped env with no
# tsserver.path needed). Pinned for reproducibility. node/npm are the image base.
RUN npm install -g basedpyright@1.39.8 typescript-language-server@5.3.0 typescript@6.0.3

COPY --from=gitbuild /opt/git /opt/git
ENV PATH=/opt/git/bin:$PATH
RUN ln -sf /opt/git/bin/git /usr/local/bin/git
