# Podman Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the bridge's containerized backends under macOS `podman machine` with zero behavioral change on docker, plus close the `[verify].runtime` allowlist gap.

**Architecture:** The runtime is already a single configurable seam (`SandboxConfig.runtime`, threaded as data through every compose/reaper/verify/CLI site) — so podman is delivered as **config + a hand-rolled egress script + docs**, validated by live gates G1–G6 on `podman machine`. The only Rust is a pure post-snapshot `gate_verify_runtime` (reject a disallowed verify runtime into the existing `VerifyOutcome::ConfigError` path) and a warn-level runtime preflight. Docker stays the default and untouched.

**Tech Stack:** Rust 1.94.0 (workspace), TOML config, podman (rootless, macOS `podman machine`), tinyproxy egress, shell.

**Spec:** `docs/superpowers/specs/2026-06-10-podman-support-design.md` (v2 + §4 fable-consult refinement).
**Reviews folded:** `docs/superpowers/reviews/2026-06-10-podman-spec-review.md`, `…-podman-cleanroom-design.md`.

**Decisions (locked):** verify-runtime gap → reject into `ConfigError`; Linux rootless deferred (this increment = macOS `podman machine` only).

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `examples/a2a-bridge.containerized.podman.toml` | **create** | Copy of the docker containerized config; only `runtime`/`allowed_cmds` differ |
| `deploy/containers/reader.Containerfile` | modify line 3 | Qualify the registry base `FROM` for podman short-name resolution |
| `deploy/containers/proxy.Containerfile` | modify line 1 | Same `FROM` qualification |
| `deploy/containers/sync-creds.sh` | modify ~line 48 | Runtime-neutral kiro re-login hint |
| `deploy/containers/podman-egress.sh` | **create** | Idempotent `up\|status\|down` for the two-network + two-proxy egress under podman |
| `docs/containerized-agents.md` | modify | Podman runbook section |
| `docs/onboarding.md` | modify | One-line pointer to the podman config + section |
| `bin/a2a-bridge/src/config.rs` | modify | `gate_verify_runtime` pure fn + unit tests |
| `bin/a2a-bridge/src/main.rs` | modify (2 sites + preflight) | Wire the gate after `into_snapshot`; example parse/parity tests; warn-level runtime preflight |

**Slices (each independently shippable):** 1 = config + images + docs (exit: G3 + an open-egress spawn smoke). 2 = egress script (exit: G2). 3 = full-loop live validation (exit: G4+G5+G6 → **podman ships**). 4 = hardening: the verify-gate + preflight (Rust). The §6 template-dialect fork is **contingent** — built only if G4/G6 fail — and is therefore NOT a task here; if a gate fails, return to the spec §6 fork decision.

**Note on testing style:** Slices 1–3 are config/shell/docs whose acceptance is the **live gates** (they need `podman machine`); they are not red-green unit tests. Slice 4 is pure Rust and is full TDD. The live gates require the operator to free host RAM first (stop the stockTrading dev stack) and run `podman machine`.

---

## Slice 1 — Config + images + docs

### Task 1: Podman example config + parse/parity tests

**Files:**
- Create: `examples/a2a-bridge.containerized.podman.toml`
- Test: `bin/a2a-bridge/src/main.rs` (add two `#[test]` in the existing `cli_tests` module — see the existing `init_generated_config_parses_and_loads` for the pattern)

- [ ] **Step 1: Write the failing parity + parse test**

In `bin/a2a-bridge/src/main.rs`, in the `cli_tests` test module, add:

```rust
#[test]
fn podman_example_parses_validates_and_mirrors_docker() {
    // The podman example must parse, build a snapshot, and validate (S3) Docker-free.
    let podman_src = include_str!("../../../examples/a2a-bridge.containerized.podman.toml");
    let docker_src = include_str!("../../../examples/a2a-bridge.containerized.toml");

    let cfg = config::RegistryConfig::parse(podman_src).expect("podman example parses");
    let snap = cfg.into_snapshot().expect("podman example snapshots");
    // S3 runtime allowlist holds for podman (validates via the same Registry::new path used at boot).
    bridge_registry::registry::Registry::new(snap, acp_spawn_fn_for_tests())
        .expect("podman example validates (S3): runtime 'podman' is allowlisted");

    // Parity: identical except the two runtime/allowlist axes. Diffing the line-sets, every line that
    // differs must mention `runtime` or `allowed_cmds` (or `podman`/`docker`) — nothing structural drifts.
    let only_in = |a: &str, b: &str| -> Vec<String> {
        let bset: std::collections::HashSet<_> = b.lines().map(str::trim).collect();
        a.lines().map(str::trim).filter(|l| !l.is_empty() && !bset.contains(l)).map(String::from).collect()
    };
    for line in only_in(podman_src, docker_src).iter().chain(only_in(docker_src, podman_src).iter()) {
        assert!(
            line.contains("runtime") || line.contains("allowed_cmds")
                || line.contains("podman") || line.contains("docker")
                || line.starts_with('#'),
            "podman example diverges from docker example outside runtime/allowlist: {line:?}"
        );
    }
}
```

Note: if `acp_spawn_fn_for_tests()` does not already exist, reuse whatever spawn-fn helper the existing registry-validation tests in this module use (search the module for `Registry::new(` in a `#[test]`); the assertion only needs `Registry::new` to return `Ok`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p a2a-bridge podman_example_parses -- --nocapture`
Expected: FAIL — `include_str!` can't find `examples/a2a-bridge.containerized.podman.toml` (file does not exist yet) → compile error.

- [ ] **Step 3: Create the podman example config**

Create `examples/a2a-bridge.containerized.podman.toml` as a copy of `examples/a2a-bridge.containerized.toml` with **only** these edits:
- Header comment: replace the docker-specific intro with: `# Podman variant of a2a-bridge.containerized.toml. Two-line rule vs the docker config: (1) allowed_cmds lists "podman"; (2) every [agents.sandbox] block AND [verify] set runtime = "podman". Everything else is identical. Bring up egress with deploy/containers/podman-egress.sh up. See docs/containerized-agents.md (Podman).`
- In `[registry]`: `allowed_cmds = ["podman"]`.
- In **every** `[agents.sandbox]` block (claude, codex, kiro, impl) add `runtime = "podman"` as the first key under the block.
- In `[verify]` add `runtime = "podman"` as the first key.
- Keep `allowed_cwd_root`, all mounts, egress nets/proxy URLs, creds volumes, and all workflows byte-identical.

(Copy the source file first, then apply the edits, so structural parity holds for the test.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p a2a-bridge podman_example -- --nocapture`
Expected: PASS (parses, snapshots, validates S3 with `allowed_cmds=["podman"]`, parity holds).

- [ ] **Step 5: Commit**

```bash
git add examples/a2a-bridge.containerized.podman.toml bin/a2a-bridge/src/main.rs
git commit -m "feat(podman): podman containerized example + parse/parity tests"
```

### Task 2: Containerfile FROM qualification (registry bases only)

**Files:**
- Modify: `deploy/containers/reader.Containerfile:3`, `deploy/containers/proxy.Containerfile:1`

- [ ] **Step 1: Qualify the two registry bases**

In `deploy/containers/reader.Containerfile`, change the base image line to:
```dockerfile
FROM docker.io/library/node:24-slim
```
In `deploy/containers/proxy.Containerfile`, change the base image line to:
```dockerfile
FROM docker.io/library/debian:stable-slim
```
**Do NOT touch `deploy/containers/toolchain.Containerfile`** — its `FROM a2a-agent-reader:latest` is a *local* image; qualifying it would break both engines.

- [ ] **Step 2: Verify Docker still builds (no-op qualification)**

Run: `docker build -t a2a-agent-reader:latest -f deploy/containers/reader.Containerfile deploy/containers && docker build -t a2a-egress-proxy:latest -f deploy/containers/proxy.Containerfile deploy/containers`
Expected: both build successfully (the fully-qualified names resolve identically on Docker Hub). This also confirms the reader image is current.

- [ ] **Step 3: Commit**

```bash
git add deploy/containers/reader.Containerfile deploy/containers/proxy.Containerfile
git commit -m "feat(podman): qualify registry FROM bases for podman short-name resolution"
```

### Task 3: Runtime-neutral sync-creds message

**Files:**
- Modify: `deploy/containers/sync-creds.sh` (the line printing a `docker run` kiro re-login hint, ~line 48)

- [ ] **Step 1: Make the kiro re-login hint runtime-neutral**

Find the line in `deploy/containers/sync-creds.sh` that prints a hardcoded `docker run …` kiro re-login command (grep: `grep -n 'docker run' deploy/containers/sync-creds.sh`). Change it to honor an optional `CONTAINER_RUNTIME` (default `docker`), e.g. replace the literal `docker` in that printed string with `${CONTAINER_RUNTIME:-docker}`. Only that printed hint changes; the rest of the script is host-side and runtime-agnostic.

- [ ] **Step 2: Verify the script still parses**

Run: `bash -n deploy/containers/sync-creds.sh`
Expected: no output (syntax OK).

- [ ] **Step 3: Commit**

```bash
git add deploy/containers/sync-creds.sh
git commit -m "feat(podman): runtime-neutral kiro re-login hint in sync-creds.sh"
```

### Task 4: Podman runbook docs

**Files:**
- Modify: `docs/containerized-agents.md` (add a Podman section), `docs/onboarding.md` (one-line pointer)

- [ ] **Step 1: Add the Podman section to `docs/containerized-agents.md`**

Add a `## Podman (macOS)` section covering, concretely:
- **Select podman:** use `examples/a2a-bridge.containerized.podman.toml` (or, in your own config, add `"podman"` to `allowed_cmds` and `runtime = "podman"` to every `[agents.sandbox]` block and `[verify]`).
- **Machine:** `podman machine init --cpus 6 --memory 8192 --disk-size 100 && podman machine start`. Confirm `/Users` is mounted in the machine (so the identical-path `-v {m}:{m}` bind works): `podman machine inspect | grep -i mount`.
- **PATH:** the bridge resolves `podman` via `PATH`; a launchd-launched `serve` needs `podman` on its `PATH`.
- **Images (separate store — podman cannot see docker-built images):** build in order
  `podman build -t a2a-agent-reader:latest -f deploy/containers/reader.Containerfile deploy/containers`,
  then `… -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile …` (uses the reader image),
  then `… -t a2a-egress-proxy:latest -f deploy/containers/proxy.Containerfile …`.
- **Egress:** `deploy/containers/podman-egress.sh up`; **re-run it after every `podman machine start`** (`--restart` does not survive a machine restart). `… status` to check, `… down` to tear down.
- **Kiro:** the `a2a-kiro-data` volume does not carry over from docker → re-mint with `kiro-cli login --use-device-flow` into the podman volume (see the existing kiro setup steps; run them under podman).
- **Min podman version:** ≥ 4.5 (netavark ≥ 1.6) for DNS on `--internal` networks; if name resolution of `a2a-egress-proxy` from the internal net fails, see the IP-pinning fallback in `podman-egress.sh` comments.
- **Caveats:** `containers list|reap` does NOT see verify containers (they carry no `a2a.managed=1` label — true on docker too). `podman rm` is synchronous → expect 0 containers immediately after a run (unlike Docker Desktop's ~2 s async removal).

- [ ] **Step 2: Add the onboarding pointer**

In `docs/onboarding.md`, add one line under the containerized/sandbox section: `**Podman (macOS):** use examples/a2a-bridge.containerized.podman.toml and see docs/containerized-agents.md → Podman.`

- [ ] **Step 3: Commit**

```bash
git add docs/containerized-agents.md docs/onboarding.md
git commit -m "docs(podman): runbook section + onboarding pointer"
```

### Slice 1 exit gate (operator-run, live — after `podman machine` is up)

- [ ] **G3 (allowlist negative, no machine needed):** copy the podman example, set `allowed_cmds = ["docker"]`, run any containerized workflow → boot fails with `sandbox runtime not allowed: podman`. (Also covered by the Task 1 parse test inversely.)
- [ ] **G1 (spawn smoke):** with `podman machine` up + images built + `podman-egress.sh up`, run a **single-agent** workflow (e.g. `smoke-claude` from the example) under the podman config → completes; `podman events` shows the agent container start. (Diagnostic option: a temporary `egress = "open"` config isolates spawn from egress.)

---

## Slice 2 — Egress bring-up script

### Task 5: `podman-egress.sh` (up | status | down)

**Files:**
- Create: `deploy/containers/podman-egress.sh`

- [ ] **Step 1: Write the script**

Create `deploy/containers/podman-egress.sh` (chmod +x) implementing the post-condition contract from spec §3. Key requirements, all explicit:
- `#!/usr/bin/env bash`, `set -euo pipefail`, and **self-locate**: `cd "$(dirname "$0")"` (so the `-v ./tinyproxy.verify.filter` source resolves; use an absolute path via `"$PWD/tinyproxy.verify.filter"`).
- `RUNTIME="${CONTAINER_RUNTIME:-podman}"`.
- `up`:
  - Networks (idempotent — `|| true` on "already exists"): `"$RUNTIME" network create --internal a2a-egress-internal`, `"$RUNTIME" network create a2a-egress-external`, `"$RUNTIME" network create --internal a2a-verify-egress`.
  - Build the proxy image if missing: `"$RUNTIME" image exists a2a-egress-proxy:latest || "$RUNTIME" build -t a2a-egress-proxy:latest -f proxy.Containerfile .`
  - Each proxy — `rm -f` first (idempotent re-up), then create on its internal net, connect external, start:
    ```bash
    "$RUNTIME" rm -f a2a-egress-proxy 2>/dev/null || true
    "$RUNTIME" create --name a2a-egress-proxy --network a2a-egress-internal a2a-egress-proxy:latest
    "$RUNTIME" network connect a2a-egress-external a2a-egress-proxy
    "$RUNTIME" start a2a-egress-proxy

    "$RUNTIME" rm -f a2a-verify-proxy 2>/dev/null || true
    "$RUNTIME" create --name a2a-verify-proxy --network a2a-verify-egress \
      -v "$PWD/tinyproxy.verify.filter:/etc/tinyproxy/filter:ro" a2a-egress-proxy:latest
    "$RUNTIME" network connect a2a-egress-external a2a-verify-proxy
    "$RUNTIME" start a2a-verify-proxy
    ```
- `status`: list the 3 networks (`"$RUNTIME" network ls --filter name=a2a-`) and both proxies (`"$RUNTIME" ps --filter name=a2a-egress-proxy --filter name=a2a-verify-proxy`).
- `down`: tolerate-absent, proxies before networks: `"$RUNTIME" rm -f a2a-egress-proxy a2a-verify-proxy 2>/dev/null || true`; then `"$RUNTIME" network rm a2a-egress-internal a2a-egress-external a2a-verify-egress 2>/dev/null || true`.
- A header comment documenting the **IP-pinning fallback** (if internal-net DNS fails): create the internal nets with `--subnet 10.89.0.0/24` etc., `create … --ip 10.89.0.2`, and set `proxy = "http://10.89.0.2:8888"` in the podman config.
- Dispatch on `$1` (`up|status|down`), usage message otherwise.

- [ ] **Step 2: Verify it parses**

Run: `bash -n deploy/containers/podman-egress.sh && chmod +x deploy/containers/podman-egress.sh`
Expected: no syntax error.

- [ ] **Step 3: Commit**

```bash
git add deploy/containers/podman-egress.sh
git commit -m "feat(podman): podman-egress.sh up|status|down (idempotent egress contract)"
```

### Slice 2 exit gate (operator-run, live)

- [ ] **G2 (egress contract — the security gate):** `podman-egress.sh up`; `… status` shows 3 nets + 2 proxies. From a container on `a2a-egress-internal`:
  - name-resolve `a2a-egress-proxy` (if it fails: apply the IP fallback, re-run);
  - an allowlisted host (e.g. `https://api.anthropic.com`) via `http://a2a-egress-proxy:8888` → OK;
  - a non-allowlisted host via the proxy → tinyproxy refuses;
  - `curl --noproxy '*' https://example.com` → no route (internal net has no gateway).
  - Repeat on `a2a-verify-egress` via `a2a-verify-proxy`: a registry host (crates.io/github) → OK; a provider host → refused (creds-XOR-registries holds).

---

## Slice 3 — Full-loop live validation (podman ships here)

### Task 6: Full-loop gates (operator-run, live)

No code. Run, in order, on `podman machine` with the podman config:

- [ ] **G4 (reap + recovery):** run a containerized workflow; assert container **start** via `podman events` (not an echo); kill a container mid-turn → the owner-scoped boot-sweep reaps orphans on the next run; at run end `podman ps -a` shows **0** `a2a-ro-*`/`a2a-rw-*` immediately. Confirm a **lease-recovery** pass runs `managed_inspect_argv`'s `{{.Label …}}` template under podman without dropping rows (if rows are dropped → the §6 template fork; stop and escalate).
- [ ] **G5 (`:rw` + uid + creds):** `a2a-bridge implement "<trivial change>" --repo <clone-of-a-repo-under-allowed_cwd_root> --config examples/a2a-bridge.containerized.podman.toml` → the container writes the clone, the host commits the staged index (the B2b-1 round-trip), `git log`/ownership sane; a token refresh writes back through the writable creds bind.
- [ ] **G6 (`containers` CLI):** `a2a-bridge containers list` / `… reap` see and reap podman-owned `a2a.managed=1` containers (the `LIST_FORMAT` `{{.Label …}}` template under podman; same §6 escalation if rows drop).

**Podman support ships on G4+G5+G6 passing.** Record results (mirror the model-effort live-gate record) in a new ADR for this increment (number = next free under `docs/adr/`).

---

## Slice 4 — Hardening: the verify-runtime gate (the only Rust)

### Task 7: `gate_verify_runtime` (pure, TDD)

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (add `gate_verify_runtime` + its unit tests)

- [ ] **Step 1: Write the failing unit tests**

In `bin/a2a-bridge/src/config.rs` test module, add (helpers: build a minimal `VerifyConfig` literal — `egress` can be `EgressPolicy::Open`, `commands` a one-element vec; copy field shapes from an existing verify test in this file):

```rust
fn vc(runtime: Option<&str>) -> VerifyConfig {
    VerifyConfig {
        runtime: runtime.map(str::to_string),
        image: "img".into(),
        cache: "c".into(),
        egress: bridge_core::domain::EgressPolicy::Open,
        commands: vec![VerifyCommand { name: "t".into(), cmd: "true".into(), gate: true }],
    }
}

#[test]
fn gate_rejects_defaulted_runtime_when_only_podman_allowed() {
    let out = gate_verify_runtime(Some(Ok(vc(None))), &["podman".to_string()]);
    let err = out.unwrap().unwrap_err();
    assert!(format!("{err:?}").contains("docker"), "names the resolved default 'docker'");
}

#[test]
fn gate_rejects_explicit_disallowed_runtime() {
    let out = gate_verify_runtime(Some(Ok(vc(Some("docker")))), &["podman".to_string()]);
    assert!(out.unwrap().is_err());
}

#[test]
fn gate_allows_explicit_allowed_runtime() {
    let out = gate_verify_runtime(Some(Ok(vc(Some("podman")))), &["podman".to_string()]);
    assert_eq!(out.unwrap().unwrap().runtime.as_deref(), Some("podman"));
}

#[test]
fn gate_back_compat_defaulted_docker_allowed() {
    let out = gate_verify_runtime(Some(Ok(vc(None))), &["docker".to_string(), "codex-acp".to_string()]);
    assert!(out.unwrap().is_ok(), "existing docker configs unaffected");
}

#[test]
fn gate_preserves_prior_error() {
    let prior = Err(ConfigError::Registry("[verify] needs at least one command".into()));
    let out = gate_verify_runtime(Some(prior), &["podman".to_string()]);
    assert!(matches!(out, Some(Err(ConfigError::Registry(m))) if m.contains("at least one command")));
}

#[test]
fn gate_passes_through_none() {
    assert!(gate_verify_runtime(None, &["podman".to_string()]).is_none());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p a2a-bridge gate_ -- --nocapture`
Expected: FAIL — `gate_verify_runtime` not defined (compile error).

- [ ] **Step 3: Implement `gate_verify_runtime`**

Add to `bin/a2a-bridge/src/config.rs` (module level, `pub`):

```rust
/// Gate the resolved `[verify]` runtime against the snapshot's allowlist. PURE.
/// Only an `Ok` config is checked; a prior `Err` (e.g. empty commands) and `None` pass through
/// untouched. The "docker" default is applied HERE to mirror `SandboxConfig::runtime()` — keep the
/// two literals in sync (a defaulted `[verify].runtime` resolves to "docker" only later in
/// `compose_sandbox`, so the gate must apply the same default to check the value that will actually run).
pub fn gate_verify_runtime(
    verify_cfg: Option<Result<VerifyConfig, ConfigError>>,
    allowed_cmds: &[String],
) -> Option<Result<VerifyConfig, ConfigError>> {
    match verify_cfg {
        Some(Ok(vc)) => {
            let rt = vc.runtime.as_deref().unwrap_or("docker"); // pin: SandboxConfig::runtime() default
            if allowed_cmds.iter().any(|c| c == rt) {
                Some(Ok(vc))
            } else {
                Some(Err(ConfigError::Registry(format!(
                    "verify runtime not allowed: {rt:?} — add it to [registry].allowed_cmds or set [verify].runtime"
                ))))
            }
        }
        other => other,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p a2a-bridge gate_ -- --nocapture`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(verify): pure gate_verify_runtime (reject disallowed verify runtime)"
```

### Task 8: Wire the gate at both implement sites + a TOML wiring pin

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (two sites: after `into_snapshot()` in `implement` ~line 1291 and `implement --resume` ~line 1554)
- Test: `bin/a2a-bridge/src/main.rs` (one TOML-level wiring test in `cli_tests`)

- [ ] **Step 1: Write the failing wiring-pin test**

In `bin/a2a-bridge/src/main.rs` `cli_tests`, add (runtime-free — proves snapshot→gate→`ConfigError`):

```rust
#[test]
fn verify_runtime_gate_rejects_via_snapshot_allowlist() {
    // A [registry]-less all-podman config: into_snapshot's default union contains "podman" (sandbox
    // runtime), so a defaulted-docker [verify] must be rejected by the gate.
    let toml = r#"
default = "a"
allowed_cwd_root = "/tmp"
[[agents]]
id = "a"
cmd = "codex-acp"
[agents.sandbox]
runtime = "podman"
image = "img"
mount = "/tmp"
access = "ro"
egress = "open"
[verify]
image = "img"
cache = "c"
egress = "open"
[[verify.commands]]
name = "t"
cmd = "true"
"#;
    let cfg = config::RegistryConfig::parse(toml).expect("parses");
    let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());
    let snap = cfg.into_snapshot().expect("snapshots");
    let gated = config::gate_verify_runtime(verify_cfg, &snap.allowed_cmds);
    let outcome = run_verify_step(&gated, &bridge_core::SessionCwd::parse("/tmp").unwrap(), std::path::Path::new("/tmp"));
    assert!(matches!(outcome, verify::VerifyOutcome::ConfigError),
        "defaulted-docker verify under an all-podman allowlist → ConfigError (no spawn)");
}
```

Adjust the sandbox/verify TOML fields to whatever the parser requires (mirror an existing sandbox+verify fixture in the repo if fields are missing); the assertion is the point.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p a2a-bridge verify_runtime_gate_rejects -- --nocapture`
Expected: FAIL — without the gate wired, the test calls `gate_verify_runtime` (already exists from Task 7) so it actually passes at the helper level; the **wiring** is proven by Steps 3–4. (If you want a true red, first assert against the *un-gated* `verify_cfg` and watch it return `Ok`, then switch to the gated value.)

- [ ] **Step 3: Wire the gate at both implement sites**

In `bin/a2a-bridge/src/main.rs`, in the `implement` command, immediately AFTER:
```rust
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("implement: snapshot: {e}"))?;
```
add:
```rust
    // Gate the [verify] runtime against the resolved allowlist (reject a disallowed runtime into
    // VerifyOutcome::ConfigError — verify never runs on a non-allowlisted engine).
    let verify_cfg = config::gate_verify_runtime(verify_cfg, &snapshot.allowed_cmds);
```
Do the **identical** addition in the `implement --resume` command after its `into_snapshot()` (the `… --resume: snapshot: {e}` one). `verify_cfg` is owned, so this shadows it for the later loop wiring.

- [ ] **Step 4: Run the test + the wider suite to verify**

Run: `cargo test -p a2a-bridge verify_runtime_gate -- --nocapture && cargo test -p a2a-bridge`
Expected: PASS; no other test regresses.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(verify): wire gate_verify_runtime after into_snapshot at both implement sites"
```

### Task 9: Warn-level runtime preflight

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (a pure helper + its test + a call at `implement` and `serve`)

- [ ] **Step 1: Write the failing test**

In `bin/a2a-bridge/src/main.rs` `cli_tests`:

```rust
#[test]
fn preflight_warns_on_missing_runtime_and_skips_empty() {
    use std::collections::BTreeSet;
    // Empty runtime set (host-only config) → no warning, returns nothing to warn about.
    assert!(missing_runtimes(&BTreeSet::new(), &|_| true).is_empty());
    // A runtime whose probe fails is reported (caller warns).
    let mut s = BTreeSet::new();
    s.insert("podman".to_string());
    assert_eq!(missing_runtimes(&s, &|_| false), vec!["podman".to_string()]);
    // A runtime whose probe succeeds is not reported.
    assert!(missing_runtimes(&s, &|_| true).is_empty());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p a2a-bridge preflight_warns -- --nocapture`
Expected: FAIL — `missing_runtimes` not defined.

- [ ] **Step 3: Implement the pure helper + the production probe + the calls**

Add to `bin/a2a-bridge/src/main.rs`:

```rust
/// Pure: return the runtimes whose `probe` says they are unavailable. Injectable for tests.
fn missing_runtimes(runtimes: &std::collections::BTreeSet<String>, probe: &dyn Fn(&str) -> bool) -> Vec<String> {
    runtimes.iter().filter(|rt| !probe(rt)).cloned().collect()
}

/// Production probe: `<runtime> info` returns exit 0 within a short timeout.
fn runtime_responds(runtime: &str) -> bool {
    std::process::Command::new(runtime)
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Warn-level preflight: collect the distinct runtimes a snapshot's sandboxes + an optional verify use,
/// probe each, and warn (do not fail) on any that don't respond.
fn preflight_runtimes(snapshot: &bridge_core::domain::RegistrySnapshot, verify_runtime: Option<&str>) {
    let mut runtimes: std::collections::BTreeSet<String> = snapshot
        .entries
        .iter()
        .filter_map(|e| e.sandbox.as_ref().map(|sb| sb.runtime().to_string()))
        .collect();
    if let Some(rt) = verify_runtime { runtimes.insert(rt.to_string()); }
    for rt in missing_runtimes(&runtimes, &runtime_responds) {
        tracing::warn!(runtime = %rt,
            "configured container runtime '{rt}' did not respond to `{rt} info` — is it installed and (for podman) is `podman machine` started?");
    }
}
```

Then call `preflight_runtimes(&snapshot, verify_cfg.as_ref().and_then(|r| r.as_ref().ok()).and_then(|v| v.runtime.as_deref()))` once after the gate in `implement` (and `--resume`), and `preflight_runtimes(&snapshot, None)` once at `serve` boot after its `into_snapshot()` (the `serve` site near `main.rs:2423`/the serve `into_snapshot`). Use the actual field accessor for `AgentEntry.sandbox` and `SandboxConfig::runtime()` as defined in `bridge-core::domain`.

- [ ] **Step 4: Run the test + build to verify**

Run: `cargo test -p a2a-bridge preflight_warns && cargo build -p a2a-bridge`
Expected: PASS + builds.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(runtime): warn-level preflight for configured container runtimes"
```

---

## Pre-merge verification (after Slice 4)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` (docker default paths untouched; new gate/preflight/parse tests green)
- [ ] ci.yml coverage floors: `cargo llvm-cov --workspace --fail-under-lines 85` then per-package (`bridge-core`, `bridge-acp`, `bridge-api`, `bridge-workflow` ≥ 90 — run the workspace pass first so per-package reuses it, per the model-effort finding).

## Live gate appendix (G1–G6 — operator-run on `podman machine`)

Prereq: stop the stockTrading dev stack (free host RAM for the machine VM); `podman machine init --cpus 6 --memory 8192 --disk-size 100 && podman machine start`; build images (Task 4 Step 1 order); `podman-egress.sh up`. Gates: **G1/G3** end Slice 1; **G2** ends Slice 2; **G4/G5/G6** end Slice 3 (**podman ships**). On any `{{.Label …}}` template row-drop in G4/G6, STOP and apply the spec §6 fork. Tear down: `podman-egress.sh down && podman machine stop`; restart the stockTrading stack.

---

## Self-Review

- **Spec coverage:** §1 config+parity → Task 1; §2 FROM → Task 2; §8 sync-creds → Task 3; §7 docs → Task 4; §3 egress script+contract → Task 5 (+G2); §10 gates → Slice exits + Task 6 + the appendix; §4 verify-gate → Tasks 7–8; §5 preflight → Task 9; §6 template fork → explicitly contingent (escalation noted in G4/G6, not a task); §14 follow-ups (Linux, memory slices, disk) → out of scope. ✓
- **Type consistency:** `gate_verify_runtime(Option<Result<VerifyConfig, ConfigError>>, &[String]) -> Option<Result<VerifyConfig, ConfigError>>`, `ConfigError::Registry(String)`, `VerifyConfig { runtime, image, cache, egress, commands }`, `VerifyCommand { name, cmd, gate }`, `EgressPolicy::Open`, `run_verify_step(&Option<…>, &SessionCwd, &Path) -> VerifyOutcome::{NotConfigured,ConfigError,…}`, `snapshot.allowed_cmds`, `missing_runtimes`/`runtime_responds`/`preflight_runtimes`, `SandboxConfig::runtime()`/`AgentEntry.sandbox` — used consistently. ✓
- **Placeholders:** the live gates are intentionally operator-run (need `podman machine`), with exact commands; the §6 template fork is intentionally contingent (not a stub). No TBDs in code steps. ✓
