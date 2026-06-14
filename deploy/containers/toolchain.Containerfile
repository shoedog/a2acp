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
RUN cargo build --release -p lsp-mcp && cp target/release/lsp-mcp /lsp-mcp

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

# L3 Slice B: the in-container lsp-mcp shim (built in the lspbuild stage), delivered to the impl agent
# via CodexNative (`-c mcp_servers.lsp.command=/usr/local/bin/lsp-mcp`).
COPY --from=lspbuild /lsp-mcp /usr/local/bin/lsp-mcp
