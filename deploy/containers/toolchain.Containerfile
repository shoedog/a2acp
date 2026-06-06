# a2a-bridge toolchain image (Slice B2b-2): the reader image (ACP CLIs) + the Rust build toolchain, so
# the `impl` agent can build/test AND the bridge can run a deterministic verify. Used by `a2a-bridge
# implement`. NOT for the :ro reader agents (they don't compile).
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
