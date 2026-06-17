# Python in the containerized polyglot implementor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Python parity (in-container basedpyright nav + warm venv + verify) in the polyglot implementor, reusing the `[[languages]]` + `apply_warm_lsp` machinery **unchanged**. Net code = one optional lsp-mcp warn; the rest is the toolchain-image Python layer + a python profile + a live gate.

**Architecture:** mise-provisioned tooling (python/uv/ruff/basedpyright) with **real binaries symlinked into `/usr/local/bin`** (NO shims — per `docs/containerized-mcp-env-trap.md`); a `python` `[[languages]]` profile whose warm `fetch` builds a uv venv at `/pyvenv` (always-create) and whose `lsp_env` points basedpyright at it via `LSP_MCP_PYTHON_PATH`; verify syncs its own `/cache/venv` for `pytest`.

**Tech Stack:** Docker/OrbStack + `a2a-toolchain` image; mise; uv; basedpyright; ruff; Rust (lsp-mcp, the bin). **Spec:** `docs/superpowers/specs/2026-06-17-lsp-mcp-python-polyglot.md`. **MUST-READ:** `docs/containerized-mcp-env-trap.md`.

---

## Task 1: Toolchain image — Python layer (mise-provisioned, real-binary symlinks)

**Files:** Modify `deploy/containers/toolchain.Containerfile` (append after the go symlink block, line ~64, so existing rust/go/node layers stay cached).

- [ ] **Step 1: Add the Python layer**

Append:

```dockerfile
# Python (LSP-MCP polyglot slice): mise-provisioned python + uv + ruff + basedpyright. Real binaries are
# SYMLINKED into /usr/local/bin (on every PATH incl. codex's stripped MCP-subprocess PATH) — NEVER mise
# shims/activation (a shim resolves the version from mise's env, which the stripped env drops → the #1d
# trap; see docs/containerized-mcp-env-trap.md). mise installs to ~/.local/share/mise/installs/.../bin
# (real, absolute-path executables). node is already present (image base) for basedpyright-langserver.
RUN curl -fsSL https://mise.run | sh
ENV PATH=/root/.local/bin:$PATH
# Pin explicit versions for reproducibility (discover current with `mise ls-remote <tool>`; example pins):
RUN /root/.local/bin/mise use -g -y python@3.12 uv@0.5 ruff@0.8 "npm:basedpyright@1.22" \
 && /root/.local/bin/mise reshim
# Symlink the REAL resolved binaries onto /usr/local/bin (NOT the shims dir). basedpyright ships BOTH a
# `basedpyright` CLI (answers --version) and `basedpyright-langserver` (stdio). python install exposes
# `python`+`python3`.
RUN set -eux; for t in python python3 uv ruff basedpyright basedpyright-langserver; do \
      real="$(/root/.local/bin/mise which "$t")"; \
      ln -sf "$real" "/usr/local/bin/$t"; \
    done
```

- [ ] **Step 2: Build the image**

Run (from repo root, per the Containerfile header): `docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .`
Expected: builds clean; only the new layers compile (rust/go/lspbuild cached).

- [ ] **Step 3: Stripped-env shim guard (the #1d-trap regression test)**

Run each in a **stripped** env to prove the symlinks resolve with no mise activation:
```bash
docker run --rm --entrypoint env a2a-toolchain:latest -i PATH=/usr/local/bin:/usr/bin:/bin python3 --version
docker run --rm --entrypoint env a2a-toolchain:latest -i PATH=/usr/local/bin:/usr/bin:/bin uv --version
docker run --rm --entrypoint env a2a-toolchain:latest -i PATH=/usr/local/bin:/usr/bin:/bin ruff --version
docker run --rm --entrypoint env a2a-toolchain:latest -i PATH=/usr/local/bin:/usr/bin:/bin basedpyright --version
docker run --rm --entrypoint env a2a-toolchain:latest -i PATH=/usr/local/bin:/usr/bin:/bin sh -c 'command -v basedpyright-langserver'
```
Expected: each prints a version / path (NO "not found", NO mise/activation error). `basedpyright-langserver` is stdio → use `command -v`, NOT `--version` (codex MAJOR-5).

- [ ] **Step 4: Commit**

```bash
git add deploy/containers/toolchain.Containerfile
git commit -m "feat(toolchain): mise-provisioned python+uv+ruff+basedpyright, real binaries on /usr/local/bin (no shims)"
```

---

## Task 2: The `python` `[[languages]]` profile (both configs, mirrored)

**Files:** Modify `examples/a2a-bridge.containerized.toml` AND `examples/a2a-bridge.containerized.podman.toml` (the `podman_example_parses_validates_and_mirrors_docker` test enforces they match).

- [ ] **Step 1: Add the python profile to `containerized.toml`** (after the `go` profile block, ~line 116)

```toml
[[languages]]
id = "python"
# Always create the venv at /pyvenv (so the interpreter exists even with no installable deps — the hard
# LSP_MCP_PYTHON_PATH override would otherwise crash basedpyright). UV_PROJECT_ENVIRONMENT pins the target
# so uv never writes /work/.venv on the :ro real repo. Deps are best-effort (|| true) → no-lock repos get a
# bare venv (stdlib resolves; 3rd-party shows unresolved, not a crash). See docs/containerized-mcp-env-trap.md.
fetch = "uv venv /pyvenv && { UV_PROJECT_ENVIRONMENT=/pyvenv uv sync --frozen || UV_PROJECT_ENVIRONMENT=/pyvenv uv pip install -r requirements.txt || true; }"
warm_cache = "a2a-impl-lsp-cache-py"
dep_cache_path = "/pyvenv"
verify_cache_path = "/cache"
fetch_env = { UV_CACHE_DIR = "/uvcache" }
verify_env = { UV_CACHE_DIR = "/cache/uv" }
# basedpyright reads site-packages via this interpreter. PATH not needed (binaries symlinked to /usr/local/bin,
# already on codex's stripped PATH). PYTHONDONTWRITEBYTECODE guards a __pycache__ write under the :ro venv.
lsp_env = { LSP_MCP_PYTHON_PATH = "/pyvenv/bin/python", PYTHONDONTWRITEBYTECODE = "1" }
[[languages.verify]]
name = "format"
cmd  = "ruff format --check ."
[[languages.verify]]
name = "lint"
cmd  = "ruff check ."
[[languages.verify]]
name = "test"
cmd  = "UV_PROJECT_ENVIRONMENT=/cache/venv uv sync --frozen && /cache/venv/bin/pytest -q"
```
(NO `image =` line — inherit `[verify].image` like rust/go.)

- [ ] **Step 2: Mirror the identical block into `.podman.toml`** (after its `go` profile). Byte-identical to Step 1 (only the file's runtime/allowed_cmds differ elsewhere; the `[[languages]]` block must match).

- [ ] **Step 3: Verify parse + mirror**

Run: `cargo test -p a2a-bridge --bin a2a-bridge podman_example`
Expected: PASS (`podman_example_parses_validates_and_mirrors_docker` — proves both files parse + the python block matches across docker/podman). If it fails on a mismatch, align the two blocks.

- [ ] **Step 4: Commit**

```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml
git commit -m "feat(containerized): python [[languages]] profile (uv venv at /pyvenv + basedpyright lsp_env + verify venv)"
```

---

## Task 3: lsp-mcp — warn when a repo pins its own pyright config (codex MAJOR-1)

**Why:** basedpyright honors a repo `pyrightconfig.json` / `[tool.pyright]` / `[tool.basedpyright]` over the pushed `pythonPath`, silently ignoring the warmed `/pyvenv`. A one-line warning makes that observable.

**Files:** Modify `crates/lsp-mcp/src/lang.rs` (a pure detector + a warn in `pyright_config`). Test: same file.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn detects_repo_pyright_config() {
    let d = std::env::temp_dir().join(format!("lspmcp-pyrcfg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    assert!(!repo_has_pyright_config(&d));                       // none
    std::fs::write(d.join("pyrightconfig.json"), "{}").unwrap();
    assert!(repo_has_pyright_config(&d));                        // explicit file
    let _ = std::fs::remove_file(d.join("pyrightconfig.json"));
    std::fs::write(d.join("pyproject.toml"), "[tool.basedpyright]\n").unwrap();
    assert!(repo_has_pyright_config(&d));                        // pyproject section
    let _ = std::fs::remove_dir_all(&d);
}
```

- [ ] **Step 2: Run → fail** (`cargo test -p lsp-mcp detects_repo_pyright_config` → `repo_has_pyright_config` not found).

- [ ] **Step 3: Implement the pure detector + wire the warn**

```rust
/// True if the repo pins its OWN basedpyright/pyright config, which OVERRIDES the pushed `pythonPath`
/// (so a warmed venv may be ignored). Checks `pyrightconfig.json` or a `[tool.pyright]`/`[tool.basedpyright]`
/// section in `pyproject.toml`. Cheap, best-effort (a read failure → false).
pub fn repo_has_pyright_config(repo: &Path) -> bool {
    if repo.join("pyrightconfig.json").is_file() {
        return true;
    }
    std::fs::read_to_string(repo.join("pyproject.toml"))
        .map(|s| s.contains("[tool.pyright]") || s.contains("[tool.basedpyright]"))
        .unwrap_or(false)
}
```
In `pyright_config` (right after the `python_path` is resolved/logged), add:
```rust
    if repo_has_pyright_config(repo) {
        eprintln!(
            "[lsp-mcp] WARNING: {repo:?} has a pyrightconfig.json / [tool.(based)pyright] section — \
             basedpyright may honor it OVER the pushed pythonPath {python_path:?}, so a warmed venv \
             could be ignored (third-party resolution may differ). See docs/containerized-mcp-env-trap.md."
        );
    }
```

- [ ] **Step 4: Run → pass + all-four**

Run: `cargo test -p lsp-mcp detects_repo_pyright_config && cargo test -p lsp-mcp && cargo clippy -p lsp-mcp --all-targets -- -D warnings && cargo fmt -p lsp-mcp -- --check`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp/src/lang.rs
git commit -m "feat(lsp-mcp): warn when a repo pins its own pyright config (may override pushed pythonPath)"
```

---

## Task 4: Live gate (DoD)

**Build the release binaries first:** `cargo build --release --bin a2a-bridge --bin lsp-mcp` (the gate runs the release binary; a stale one omits new wiring).

- [ ] **Create a deps-bearing python fixture** under `/Users/wesleyjinks/code` (so it's under `allowed_cwd_root`), e.g. `a2a-1d-py`: a package importing a 3rd-party lib whose return type is non-trivial (e.g. a function returning a `requests.Response`), with a committed `pyproject.toml` + `uv.lock` (`uv lock`), and NO `pyrightconfig.json`. Add a `.py` with a typed function, `git init && commit`.

- [ ] **Gate 1 — 3rd-party nav via the warmed venv.**
  ```bash
  echo "<fn-name>" > /tmp/py-fn.txt
  ./target/release/a2a-bridge run-workflow c2b-nav --input /tmp/py-fn.txt \
    --session-cwd /Users/wesleyjinks/code/a2a-1d-py \
    --config examples/a2a-bridge.containerized.toml --out /tmp/py-nav.md
  ```
  Expected: basedpyright returns a **type-resolved** signature whose type involves the 3rd-party dep (proves the warm venv resolved it). The lsp call log (`<repo>/.git/a2a-bridge/lsp-mcp-calls.log`) shows the call; the agent_stderr / lsp-spawn shows `[lsp-mcp] python interpreter: /pyvenv/bin/python` (NOT the `python3` fallback — Opus M6). **If "no lsp tool":** apply the env-dump wrapper diagnostic from `docs/containerized-mcp-env-trap.md`.

- [ ] **Gate 2 — `:ro` safety.** `git -C /Users/wesleyjinks/code/a2a-1d-py status --porcelain` is empty.

- [ ] **Gate 3 — cache reuse.** `docker volume ls | grep a2a-impl-lsp-cache-py` shows ONE venv vol for the fixture; a 2nd run reuses it.

- [ ] **Gate 4 — verify sees deps.** Run an `implement`/verify path (or a direct `run_verify` exercise) so `ruff format`/`ruff check` + the `/cache/venv` `pytest` run green in-container.

- [ ] **Gate 5 — no regression.** Re-run the rust + go gates (tiny fixtures) → still type-resolved; a host-only workflow pays no warm cost.

- [ ] **Gate 6 — degrade (no-lock).** A python repo with NO `uv.lock`/`requirements.txt` → warm creates a BARE venv; nav resolves stdlib (no crash), 3rd-party shows unresolved. (Validates the always-create-venv MAJOR-4 fix.)

---

## Self-Review notes (author)

- **Spec coverage:** Task 1 = Layer 1 (image, codex MAJOR-5 binary, Opus shim-guard); Task 2 = Layer 2 (profile, MAJOR-2 UV_PROJECT_ENVIRONMENT, MAJOR-3 verify venv, MAJOR-4 always-create, Opus drop image=/PATH/UV_CACHE_DIR + PYTHONDONTWRITEBYTECODE); Task 3 = MAJOR-1 warn; Task 4 = DoD incl. M6 interpreter assertion + degrade.
- **Reuse unchanged:** no edits to `apply_warm_lsp`/`select_profile`/`warm_lsp_deps_step`/`compose_warm_fetch` — confirmed by both reviews. Task 3 is the only Rust change.
- **Risk:** Task 1 (image + the #1d trap surface) is the high-risk task → codex reviews Task 1 + the final; Opus per-task on 2/3. The env-dump diagnostic is the fallback.
- **Open confirmations for the implementer:** (1) current pinned versions via `mise ls-remote <tool>` (the example pins may have moved); (2) `mise which <tool>` resolves with the `-g` config set in the build (HOME=/root) — if not, fall back to symlinking from `~/.local/share/mise/installs/<tool>/<ver>/bin`; (3) basedpyright's npm package bin names (`basedpyright`, `basedpyright-langserver`) — confirm post-install. Adjust to reality; do not invent.
