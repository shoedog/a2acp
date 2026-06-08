# Concurrency-safe containerized runs — Increment A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make concurrent containerized `a2a-bridge` runs (same/different repo, one shared config) safe — no
name clash, no cross-reap — with `flock`-lease crash-orphan recovery and a `containers list|reap` operator
surface; all DB-free.

**Architecture:** Every managed `:rw`/`:ro` container carries docker labels (`a2a.run/owner/host/lease/…`)
and a `run_id` in its name; a per-process `flock` **lease** is the liveness signal (lock free ⇒ owner dead).
Reaping splits into: run-scoped END-sweep (by `a2a.run`, unconditional), owner-scoped before-first-use
crash-recovery sweep (classify by lease, Dead-only), and the unchanged specific-name per-turn reapers.

**Tech Stack:** Rust (workspace). New `crates/bridge-core/src/{run_identity,liveness}.rs`; modify
`bridge-core/src/{sandbox,reaper,lib}.rs`, `bridge-container/src/lib.rs`, `bridge-acp/src/acp_backend.rs`,
`bin/a2a-bridge/src/main.rs`. `fs2` (already in `bridge-store`) for the lock. Docker for the live gate only.

**Spec:** `docs/superpowers/specs/2026-06-07-concurrent-runs-slice-a-design.md` (rev3; clean-room +
dual-spec-reviewed; flock-primary liveness, serve-request isolation deferred to B).

**Conventions:** TDD green-per-task (red first); task/code commits do NOT carry the `Co-Authored-By` trailer
(doc commits do). Branch `feat/concurrent-runs-a` (already holds the spec). Coverage after `cargo llvm-cov
clean --workspace`; floors per **ci.yml** (workspace 85; bridge-core/acp/api/workflow 90). Serialize heavy
Docker jobs at the live gate. The HARD ORDERING CONSTRAINT (spec D2) is realized as **Slice S3 is one
commit** — the `:rw` `run_id`-in-name change and the boot-sweep flip land together.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/bridge-core/src/run_identity.rs` | create | `RunHandle` (process identity); `ContainerLabels` set; `Verdict {Alive,Dead,Unknown}`; pure `classify(labels, my_host, probe) -> Verdict`. |
| `crates/bridge-core/src/liveness.rs` | create | `host_id()`; `LeaseProbe` trait + `FsLeaseProbe` (`fs2` try-lock); `acquire_lease(run_id) -> LeaseGuard` (held for process life; removed on drop). |
| `crates/bridge-core/src/sandbox.rs` | modify | `a2a_name(role, owner, run_id, tail)`; `a2a_label_args(&ContainerLabels) -> Vec<String>`; splice `--label`s into `compose_sandbox` (so BOTH roles inherit); `by_owner_filter_argv`/`by_run_filter_argv`/`managed_list_argv`. |
| `crates/bridge-core/src/reaper.rs` | modify | `run_scoped_reap(runtime, run_id)` (reap `label=a2a.run=`); `classify_sweep(runtime, owner, my_host, probe)` (list-by-owner → classify → reap Dead); `last_output_age(runtime, name)`. |
| `crates/bridge-core/src/lib.rs` | modify | `pub mod run_identity; pub mod liveness;`. |
| `crates/bridge-container/src/lib.rs` | modify | `:rw` name gains `run_id`; accept `ContainerLabels` at construction → labels; **drop** the construction boot-sweep + the now-dead `sweep_fn` param; `retire`/END unchanged (specific name). |
| `crates/bridge-acp/src/acp_backend.rs` | modify | `:ro` spawn name gains `run_id` + the label set. |
| `bin/a2a-bridge/src/main.rs` | modify | build ONE `RunHandle` + acquire its lease per process; thread labels into both spawn paths; per-owner before-first-use `classify_sweep`; `Ro/RwSweepGuard` → `run_scoped_reap` + lease drop; `containers list|reap` subcommand + dispatch + `TOP_USAGE`. |
| `examples/a2a-bridge.containerized.toml` / docs | modify | none required for behavior; `containers` usage doc in AGENTS.md. |

Helper used throughout: `container_owner(config_path, mount, agent_id)` (`main.rs`) stays the owner hash.

---

## SLICE S1 — inert identity (labels only; NO name change; no behavior change)

### Task 1: `ContainerLabels` + `a2a_label_args` (pure)

**Files:** Create `crates/bridge-core/src/run_identity.rs`; modify `crates/bridge-core/src/lib.rs`,
`crates/bridge-core/src/sandbox.rs`.

- [ ] **Step 1: declare the module.** In `crates/bridge-core/src/lib.rs` add after `pub mod reaper;`:
```rust
pub mod run_identity;
```

- [ ] **Step 2: write the failing test** in `run_identity.rs` (`#[cfg(test)] mod tests`):
```rust
#[test]
fn container_labels_emit_managed_label_set() {
    let l = ContainerLabels {
        role: "rw".into(), kind: "warm".into(), agent: "impl".into(),
        owner: "abc".into(), run_id: "r1".into(), host: "h1".into(), lease: "/l/r1.lock".into(),
        repo: Some("/Users/w/code/proj".into()), cwd: Some("/Users/w/code/proj".into()),
        start: "2026-06-07T00:00:00Z".into(),
    };
    let args = l.to_arg_pairs();
    assert!(args.contains(&("a2a.managed".into(), "1".into())));
    assert!(args.contains(&("a2a.role".into(), "rw".into())));
    assert!(args.contains(&("a2a.run".into(), "r1".into())));
    assert!(args.contains(&("a2a.host".into(), "h1".into())));
    assert!(args.contains(&("a2a.lease".into(), "/l/r1.lock".into())));
    // display-only fields present when Some
    assert!(args.iter().any(|(k, v)| k == "a2a.repo" && v == "/Users/w/code/proj"));
}

#[test]
fn container_labels_omit_absent_display_fields() {
    let l = ContainerLabels { repo: None, cwd: None, ..sample() };
    let args = l.to_arg_pairs();
    assert!(!args.iter().any(|(k, _)| k == "a2a.repo" || k == "a2a.cwd"));
}
```
(Add a `fn sample() -> ContainerLabels` test helper mirroring the first literal.)

- [ ] **Step 3: run → RED** (`cargo test -p bridge-core run_identity 2>&1 | tail`): fails to compile.

- [ ] **Step 4: implement** the struct + `to_arg_pairs` in `run_identity.rs`:
```rust
/// The label set stamped on every managed container. Identity values are hashes/UUIDs/paths
/// (docker-label-safe); `repo`/`cwd` are display-only (sanitize at the call site; `None` ⇒ omitted).
#[derive(Clone, Debug)]
pub struct ContainerLabels {
    pub role: String,   // "rw" | "ro"
    pub kind: String,   // "warm" | "perturn" | "oneshot"
    pub agent: String,
    pub owner: String,
    pub run_id: String,
    pub host: String,
    pub lease: String,  // absolute lease-file path
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub start: String,  // rfc3339
}
impl ContainerLabels {
    /// `(key, value)` pairs; `a2a.managed=1` always, display-only fields only when `Some`.
    pub fn to_arg_pairs(&self) -> Vec<(String, String)> {
        let mut v = vec![
            ("a2a.managed".into(), "1".into()),
            ("a2a.role".into(), self.role.clone()),
            ("a2a.kind".into(), self.kind.clone()),
            ("a2a.agent".into(), self.agent.clone()),
            ("a2a.owner".into(), self.owner.clone()),
            ("a2a.run".into(), self.run_id.clone()),
            ("a2a.host".into(), self.host.clone()),
            ("a2a.lease".into(), self.lease.clone()),
            ("a2a.start".into(), self.start.clone()),
        ];
        if let Some(r) = &self.repo { v.push(("a2a.repo".into(), r.clone())); }
        if let Some(c) = &self.cwd { v.push(("a2a.cwd".into(), c.clone())); }
        v
    }
}
```

- [ ] **Step 5: add the argv splicer** in `sandbox.rs` (pure; used by the compose fns):
```rust
/// PURE. `--label k=v` argv for a managed container's label set.
pub fn a2a_label_args(pairs: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::with_capacity(pairs.len() * 2);
    for (k, v) in pairs {
        out.push("--label".into());
        out.push(format!("{k}={v}"));
    }
    out
}
```
plus a `sandbox.rs` test:
```rust
#[test]
fn a2a_label_args_pairs_each_as_two_tokens() {
    let a = a2a_label_args(&[("a2a.run".into(), "r1".into()), ("a2a.managed".into(), "1".into())]);
    assert_eq!(a, vec!["--label", "a2a.run=r1", "--label", "a2a.managed=1"]);
}
```

- [ ] **Step 6: run → GREEN** (`cargo test -p bridge-core run_identity a2a_label 2>&1 | tail`).

- [ ] **Step 7: commit**
```bash
git add crates/bridge-core/src/{run_identity.rs,lib.rs,sandbox.rs}
git commit -m "core: ContainerLabels + a2a_label_args (inert) (cr-a)"
```

### Task 2: splice labels into `compose_sandbox` (both roles inherit)

**Files:** Modify `crates/bridge-core/src/sandbox.rs`.

- [ ] **Step 1: write the failing test** (`sandbox.rs` tests) — labels reach the argv via the existing
compose path, AFTER the `run -i --rm` (and `--name` for named variants) prefix:
```rust
#[test]
fn compose_sandbox_includes_label_args() {
    let mut sb = ro_locked(); // existing test helper
    sb.access = MountAccess::Ro;
    let labels = vec![("a2a.managed".into(), "1".into()), ("a2a.run".into(), "r1".into())];
    let (_p, argv) = compose_sandbox_labeled(&sb, "claude-agent-acp", &[], &labels);
    assert!(argv.windows(2).any(|w| w[0] == "--label" && w[1] == "a2a.run=r1"));
    assert_eq!(&argv[0..3], &["run", "-i", "--rm"]); // prefix preserved
}
```

- [ ] **Step 2: RED.**

- [ ] **Step 3: implement** a `compose_sandbox_labeled` that threads labels, and refactor
`compose_sandbox` to delegate (keeps ONE source of truth; existing callers pass `&[]`):
```rust
pub fn compose_sandbox(sb: &SandboxConfig, agent_cmd: &str, agent_args: &[String]) -> (String, Vec<String>) {
    compose_sandbox_labeled(sb, agent_cmd, agent_args, &[])
}

/// As `compose_sandbox`, plus `--label`s spliced right after the `run -i --rm` prefix.
pub fn compose_sandbox_labeled(
    sb: &SandboxConfig, agent_cmd: &str, agent_args: &[String], labels: &[(String, String)],
) -> (String, Vec<String>) {
    // ... existing body building `argv` ...
    // after `let mut argv = vec!["run".into(), "-i".into(), "--rm".into()];`, splice labels:
    let label_argv = a2a_label_args(labels);
    argv.splice(3..3, label_argv);
    // ... rest unchanged ...
}
```
Add `labels` params to `compose_container_rw` + `compose_sandbox_named` (thread to `compose_sandbox_labeled`;
their `--name` splice stays AFTER the labels — i.e. splice name at `3..3` first, then labels at `3..3`, or
build name then labels so order is `run -i --rm --name N --label …`). Update the existing callers in
`bridge-container`/`bridge-acp` to pass `&[]` for now (S3/S1-wiring fills them).

- [ ] **Step 4: GREEN** (`cargo test -p bridge-core sandbox 2>&1 | tail`) + `cargo build --workspace` (callers compile with `&[]`).

- [ ] **Step 5: commit**
```bash
git add crates/bridge-core/src/sandbox.rs crates/bridge-container/src/lib.rs crates/bridge-acp/src/acp_backend.rs
git commit -m "core: compose_sandbox_labeled (label splice; callers pass empty) (cr-a)"
```

### Task 3: `RunHandle` + host id + `a2a_name` (identity scaffolding, no wiring)

**Files:** Modify `crates/bridge-core/src/run_identity.rs`, `crates/bridge-core/src/sandbox.rs`.

- [ ] **Step 1: failing tests:**
```rust
// run_identity.rs
#[test]
fn run_handle_builds_label_for_owner() {
    let h = RunHandle { run_id: "r1".into(), host: "h1".into(), lease: "/l/r1.lock".into(),
        start: "T".into() };
    let l = h.labels("rw", "warm", "impl", "owner9", Some("/repo"), Some("/cwd"));
    assert_eq!(l.run_id, "r1"); assert_eq!(l.owner, "owner9"); assert_eq!(l.role, "rw");
}
// sandbox.rs
#[test]
fn a2a_name_carries_owner_and_run() {
    assert_eq!(a2a_name("rw", "own", "r1", "0"), "a2a-rw-own-r1-0");
    assert_eq!(a2a_name("ro", "own", "r1", "abcd"), "a2a-ro-own-r1-abcd");
}
```

- [ ] **Step 2: RED.**

- [ ] **Step 3: implement:**
```rust
// run_identity.rs
#[derive(Clone, Debug)]
pub struct RunHandle { pub run_id: String, pub host: String, pub lease: String, pub start: String }
impl RunHandle {
    pub fn labels(&self, role: &str, kind: &str, agent: &str, owner: &str,
        repo: Option<&str>, cwd: Option<&str>) -> ContainerLabels {
        ContainerLabels {
            role: role.into(), kind: kind.into(), agent: agent.into(), owner: owner.into(),
            run_id: self.run_id.clone(), host: self.host.clone(), lease: self.lease.clone(),
            repo: repo.map(sanitize_display), cwd: cwd.map(sanitize_display), start: self.start.clone(),
        }
    }
}
/// Display-label hygiene: keep printable ASCII, cap length, never break label syntax.
fn sanitize_display(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_graphic() || *c == ' ' || *c == '/').take(200).collect()
}
```
```rust
// sandbox.rs
pub fn a2a_name(role: &str, owner: &str, run_id: &str, tail: &str) -> String {
    format!("a2a-{role}-{owner}-{run_id}-{tail}")
}
```

- [ ] **Step 4: GREEN.** **Step 5: commit** (`core: RunHandle + a2a_name (cr-a)`).

---

## SLICE S2 — flock liveness module (no wiring)

### Task 4: `liveness.rs` — host id, lease acquire/probe (`fs2`)

**Files:** Create `crates/bridge-core/src/liveness.rs`; modify `crates/bridge-core/src/lib.rs`,
`crates/bridge-core/Cargo.toml`.

- [ ] **Step 1: add the dep.** In `crates/bridge-core/Cargo.toml` `[dependencies]` add `fs2 = { workspace
= true }` (already used by `bridge-store`; if not a workspace dep, copy the version from
`crates/bridge-store/Cargo.toml`).

- [ ] **Step 2: declare the module** in `lib.rs`: `pub mod liveness;`.

- [ ] **Step 3: failing tests** (`liveness.rs` tests) — the real `flock` semantics via a tempfile:
```rust
#[tokio::test]
async fn lease_held_blocks_second_acquire_then_frees_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("A2A_LEASE_DIR", dir.path()); // test override (see impl)
    let probe = FsLeaseProbe;
    let guard = acquire_lease("r1").unwrap();          // run holds it
    let path = guard.path().to_string_lossy().to_string();
    assert_eq!(probe.is_free(&path), false, "held lease is not free");
    drop(guard);                                       // process exits
    assert_eq!(probe.is_free(&path), true, "freed after drop");
}
#[test]
fn is_free_unknown_paths_report_not_free_is_false_via_absent() {
    // absent file → caller treats as Unknown (spec): probe returns None
    assert!(FsLeaseProbe.try_state("/no/such/lease.lock").is_none());
}
```

- [ ] **Step 4: RED.**

- [ ] **Step 5: implement:**
```rust
use fs2::FileExt;
use std::path::{Path, PathBuf};

/// Stable per-host id (best-effort): hostname; falls back to "localhost". Used so a sweep never reaps
/// containers a DIFFERENT host owns (label `a2a.host`).
pub fn host_id() -> String {
    std::process::Command::new("hostname").output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

fn lease_dir() -> PathBuf {
    if let Ok(d) = std::env::var("A2A_LEASE_DIR") { return PathBuf::from(d); }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".a2a-bridge").join("leases")
}

/// Held for the owning process's life; the OS releases the flock when the File drops (clean OR crash).
pub struct LeaseGuard { path: PathBuf, _file: std::fs::File }
impl LeaseGuard {
    pub fn path(&self) -> &Path { &self.path }
}
impl Drop for LeaseGuard {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.path); } // best-effort; OS already freed the lock
}

/// Create + exclusively flock `<lease_dir>/<run_id>.lock`. Returned guard must outlive the run.
pub fn acquire_lease(run_id: &str) -> std::io::Result<LeaseGuard> {
    let dir = lease_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{run_id}.lock"));
    let file = std::fs::OpenOptions::new().create(true).read(true).write(true).open(&path)?;
    file.try_lock_exclusive()?; // we are the owner; held until `file` drops
    Ok(LeaseGuard { path, _file: file })
}

/// Probe a lease path WITHOUT holding it. `Some(true)` = free (acquired+released → owner dead);
/// `Some(false)` = held (owner alive); `None` = absent/unreadable (caller ⇒ Unknown ⇒ spare).
pub trait LeaseProbe: Send + Sync {
    fn try_state(&self, lease_path: &str) -> Option<bool>;
    fn is_free(&self, lease_path: &str) -> bool { matches!(self.try_state(lease_path), Some(true)) }
}
pub struct FsLeaseProbe;
impl LeaseProbe for FsLeaseProbe {
    fn try_state(&self, lease_path: &str) -> Option<bool> {
        let f = std::fs::OpenOptions::new().read(true).write(true).open(lease_path).ok()?;
        match f.try_lock_exclusive() {
            Ok(()) => { let _ = f.unlock(); Some(true) }   // acquired ⇒ free ⇒ dead
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Some(false), // held ⇒ alive
            Err(_) => None,                                 // unknown
        }
    }
}
```

- [ ] **Step 6: GREEN** (`cargo test -p bridge-core liveness 2>&1 | tail`). **Step 7: commit**
(`core: flock lease + probe + host_id (cr-a)`).

### Task 5: pure `classify` (Verdict over labels × probe)

**Files:** Modify `crates/bridge-core/src/run_identity.rs`.

- [ ] **Step 1: failing tests** (inject a fake probe so it's Docker-free):
```rust
struct FakeProbe(std::collections::HashMap<String, Option<bool>>);
impl crate::liveness::LeaseProbe for FakeProbe {
    fn try_state(&self, p: &str) -> Option<bool> { self.0.get(p).copied().flatten() }
}
fn labels_for(host: &str, lease: &str) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([("a2a.host".into(), host.into()), ("a2a.lease".into(), lease.into())])
}
#[test]
fn classify_covers_all_verdicts() {
    use Verdict::*;
    let me = "h1";
    let mk = |state| { let mut m = std::collections::HashMap::new(); m.insert("/l".into(), state); FakeProbe(m) };
    // other host → Unknown
    assert_eq!(classify(&labels_for("h2", "/l"), me, &mk(Some(true))), Unknown);
    // same host, lease free → Dead
    assert_eq!(classify(&labels_for("h1", "/l"), me, &mk(Some(true))), Dead);
    // same host, lease held → Alive
    assert_eq!(classify(&labels_for("h1", "/l"), me, &mk(Some(false))), Alive);
    // same host, lease absent/unknown → Unknown
    assert_eq!(classify(&labels_for("h1", "/l"), me, &mk(None)), Unknown);
}
```

- [ ] **Step 2: RED.**

- [ ] **Step 3: implement:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict { Alive, Dead, Unknown }

/// Fail-safe toward SPARING: only `Dead` (same host + lease lock free) permits a reap.
pub fn classify(
    labels: &std::collections::HashMap<String, String>,
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) -> Verdict {
    match labels.get("a2a.host") {
        Some(h) if h != my_host => return Verdict::Unknown, // another machine
        None => return Verdict::Unknown,
        _ => {}
    }
    let Some(lease) = labels.get("a2a.lease") else { return Verdict::Unknown };
    match probe.try_state(lease) {
        Some(true) => Verdict::Dead,   // lock free ⇒ owner gone
        Some(false) => Verdict::Alive, // lock held ⇒ owner alive
        None => Verdict::Unknown,      // absent/unreadable ⇒ spare
    }
}
```

- [ ] **Step 4: GREEN.** **Step 5: commit** (`core: classify (Verdict via lease + host) (cr-a)`).

### Task 6: reaper helpers — `run_scoped_reap`, `classify_sweep`, `last_output_age`

**Files:** Modify `crates/bridge-core/src/reaper.rs`, `crates/bridge-core/src/sandbox.rs`.

- [ ] **Step 1: pure filter-argv + a failing sandbox test:**
```rust
// sandbox.rs
#[test]
fn label_filter_argvs() {
    assert_eq!(by_run_filter_argv("docker", "r1").1, vec!["ps","-aq","--filter","label=a2a.run=r1"]);
    assert_eq!(by_owner_filter_argv("docker", "own").1, vec!["ps","-aq","--filter","label=a2a.owner=own"]);
}
```
Implement in `sandbox.rs`:
```rust
pub fn by_run_filter_argv(runtime: &str, run_id: &str) -> (String, Vec<String>) {
    (runtime.into(), vec!["ps".into(),"-aq".into(),"--filter".into(), format!("label=a2a.run={run_id}")])
}
pub fn by_owner_filter_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (runtime.into(), vec!["ps".into(),"-aq".into(),"--filter".into(), format!("label=a2a.owner={owner}")])
}
/// `(program, argv)` to read each owner-labeled container's id + a2a.host + a2a.lease, tab-separated.
pub fn managed_inspect_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (runtime.into(), vec!["ps".into(),"-a".into(),"--filter".into(), format!("label=a2a.owner={owner}"),
        "--format".into(), "{{.ID}}\t{{.Label \"a2a.host\"}}\t{{.Label \"a2a.lease\"}}".into()])
}
```

- [ ] **Step 2: GREEN** the sandbox test; then add the reaper helpers (these shell out, so unit-test the
PURE decision; the docker calls are exercised by the live gate). In `reaper.rs`:
```rust
use crate::run_identity::{classify, Verdict};
use crate::liveness::LeaseProbe;

/// Reap THIS run's containers (END-sweep): `ps -aq --filter label=a2a.run=<id>` → `rm -f` each. Best-effort.
pub fn run_scoped_reap(runtime: &str, run_id: &str) {
    let (p, argv) = crate::sandbox::by_run_filter_argv(runtime, run_id);
    if let Ok(out) = std::process::Command::new(&p).args(&argv).output() {
        for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            let _ = std::process::Command::new(runtime).args(["rm", "-f", id]).output();
        }
    }
}

/// Owner-scoped crash-recovery: inspect each container in `owner`, `classify` it, reap ONLY `Dead`.
pub fn classify_sweep(runtime: &str, owner: &str, my_host: &str, probe: &dyn LeaseProbe) {
    let (p, argv) = crate::sandbox::managed_inspect_argv(runtime, owner);
    let Ok(out) = std::process::Command::new(&p).args(&argv).output() else { return };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.split('\t');
        let (Some(id), Some(host), Some(lease)) = (it.next(), it.next(), it.next()) else { continue };
        let labels = std::collections::HashMap::from([
            ("a2a.host".to_string(), host.to_string()), ("a2a.lease".to_string(), lease.to_string())]);
        if classify(&labels, my_host, probe) == Verdict::Dead {
            let _ = std::process::Command::new(runtime).args(["rm", "-f", id]).output();
        }
    }
}

/// Container's last-output age in seconds (`docker logs --tail 1 -t`), or None if no output/parse fail.
pub fn last_output_age_secs(runtime: &str, name: &str) -> Option<u64> { /* parse the rfc3339 ts prefix */ }
```
(Implement `last_output_age_secs` by running `docker logs --tail 1 -t <name>`, taking the leading rfc3339
token, and returning `now - ts` in seconds; on any failure return `None`. Unit-test the PURE parse of a
sample `2026-06-07T00:00:00.000000000Z <line>` against a fixed "now" by extracting a pure
`fn parse_log_ts(line, now_epoch) -> Option<u64>` and testing it.)

- [ ] **Step 3: GREEN** (`cargo test -p bridge-core reaper sandbox 2>&1 | tail`). **Step 4: commit**
(`reaper: run_scoped_reap + classify_sweep + last_output_age (cr-a)`).

---

## SLICE S3 — atomic behavioral flip (ONE commit; the HARD ORDERING constraint)

> Tasks 7–10 land as a **single commit** — splitting them reintroduces the name clash (spec D2). Implement
> all four, run the full suite, then commit once.

### Task 7: `:rw` name + labels in `ContainerRwBackend`

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] Add a `labels: ContainerLabels` (or the fields needed to build it: `run_id`, `host`, `lease`, `repo`,
`cwd`, `start`) to `ContainerRwConfig`/the constructor, plumbed from `main.rs`. At the mint site
(`open_inner`, currently `let name = format!("a2a-rw-{}-{}", self.owner, n);`) change to:
```rust
let name = bridge_core::sandbox::a2a_name("rw", &self.owner, &self.run_id, &n.to_string());
let labels = self.labels.to_arg_pairs(); // role/kind=warm|perturn set by caller
let (program, argv) = compose_container_rw(&self.cfg.sandbox, &rw_canon, &name, &self.cfg.cmd, &self.cfg.args, &labels);
```
- [ ] **Drop the construction boot-sweep** (`sweep_fn(format!("a2a-rw-{owner}-"))` in `new_with_hooks`) and
the now-dead `sweep_fn` parameter — recovery moves to `main.rs`'s before-first-use `classify_sweep`. Update
`new`/`new_warm`/`new_with_hooks`/`new_warm_with_hooks` signatures + all call sites + the boot-sweep test
(`boot_sweep_runs_at_construction_with_owner_filter`) → delete it (replaced by the main-level sweep tested
at the live gate).

### Task 8: `:ro` name + labels in the AcpBackend container spawn

**Files:** Modify `crates/bridge-acp/src/acp_backend.rs`, `bin/a2a-bridge/src/main.rs` (`acp_spawn_inputs`).

- [ ] Where the `:ro` container name is built (`ro_container_name(&owner, &nonce)` in `main.rs`'s
`acp_spawn_inputs`), switch to `a2a_name("ro", &owner, &run_id, &nonce)` and pass the label set through to
`compose_sandbox_named` → `compose_sandbox_labeled`. Thread `run_id`/labels via `AcpConfig`/the spawn inputs.

### Task 9: flip the sweeps in `main.rs`

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

- [ ] Build ONE `RunHandle` at the top of each entry path (`implement_cmd`, `run_workflow_cmd`, the serve
setup): `let host = liveness::host_id(); let run_id = uuid(); let _lease = liveness::acquire_lease(&run_id)?;`
(`_lease` must live for the whole command — bind it in the function scope, not a temporary). `start` =
rfc3339 now (pass a timestamp in; the workspace forbids `Date::now()` in some crates — `main` may use
`std::time::SystemTime`). Thread `RunHandle` into both spawn paths.
- [ ] Replace `ro_sweep_targets`+`ro_sweep`+`RoSweepGuard` and `rw_sweep_targets`+`rw_sweep`+`RwSweepGuard`:
  - **Before-first-use recovery:** for each owner in the snapshot, call
    `reaper::classify_sweep(runtime, &owner, &host, &FsLeaseProbe)` before the registry/spawn (for one-shots
    this is at startup; keep it where the old `*_sweep` boot calls were).
  - **END guards:** `RoSweepGuard`/`RwSweepGuard` now hold the `run_id` and on `Drop` call
    `reaper::run_scoped_reap(runtime, &run_id)` (the `_lease` guard drops alongside, removing the lease).

### Task 10: build + full suite + commit S3

- [ ] `cargo build --workspace 2>&1 | tail`; fix any call-site fallout (dropped `sweep_fn`, new params).
- [ ] `cargo test --workspace 2>&1 | grep -E "test result: FAILED|FAILED|error" ` → none. Adapt/replace the
container boot-sweep test + any sweep-guard tests to the new label-scoped behavior.
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -3` → clean.
- [ ] **Commit ONCE** (the atomic flip):
```bash
git add -A
git commit -m "concurrent-runs: run_id-in-name + flock-classify sweeps + run-scoped end-guards (atomic) (cr-a)"
```

---

## SLICE S4 — `containers list`

### Task 11: `containers list` (read-only classify + stale + legacy)

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1:** add a pure formatter `fn format_containers_row(...) -> String` + a unit test (given a
parsed record → expected columns incl. `Alive/Dead/Unknown` + `stale` flag at >threshold).
- [ ] **Step 2:** implement `containers_cmd(args)`: `docker ps -a --filter label=a2a.managed=1 --format
'<tsv of run/role/kind/agent/host/lease/repo/cwd/start/name>'` → for each, `classify` + `last_output_age_secs`
→ print the table; then a second pass listing **legacy** unlabeled `a2a-{rw,ro}-*` names (no `a2a.managed`)
as `legacy (list-only)`. Dispatch `Some("containers") => return containers_cmd(&raw_args[2..])` in the
top-level `match`; add `containers` to the unknown-subcommand list + `TOP_USAGE`; handle `list` (default) and
`--help`.
- [ ] **Step 3:** `cargo test -p a2a-bridge --bin a2a-bridge containers 2>&1 | tail`; manual smoke
`a2a-bridge containers list`.
- [ ] **Step 4: commit** (`containers: list (classify + stale + legacy, read-only) (cr-a)`).

---

## SLICE S5 — `containers reap`

### Task 12: `containers reap` (Dead-only default; --stale/--force overrides)

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1:** pure `fn reap_plan(records, flags) -> Vec<String /*ids/names to reap*/>` + tests: default =
this-owner(s) Dead-only; `--all-dead` = every owner Dead-only; `--run <id>`/`--owner <hash>` Dead-only unless
`--force`; `--stale [--older-than d]` = Alive containers past the age; `--force <name>` = exactly that name
regardless of verdict. Assert the plan never includes an `Alive` container unless `--stale`/`--force`.
- [ ] **Step 2:** wire `reap` into `containers_cmd` using `reaper::run_scoped_reap`/`classify_sweep`/direct
`rm -f` per the plan. `--force <name>` calls `docker rm -f <name>` directly (the only Alive/legacy override).
- [ ] **Step 3:** `cargo test … reap_plan`; manual smoke (`containers reap --all-dead`).
- [ ] **Step 4: commit** (`containers: reap (--all-dead/--run/--owner/--stale/--force) (cr-a)`).

---

## Task 13: workspace gate

- [ ] Serialized: `cargo fmt --all`; `cargo build --workspace`; `cargo test --workspace` (+ `cargo test -p
bridge-container` separately — the workspace test excludes it for the hermetic verify, but here run BOTH);
`cargo clippy --workspace --all-targets --all-features -- -D warnings`. All green.
- [ ] Coverage: `cargo llvm-cov clean --workspace` then `cargo llvm-cov -p bridge-core -p a2a-bridge`; keep
`run_identity`/`liveness`/the pure reaper decisions ≥90. Floors per ci.yml.
- [ ] Commit any fmt-only changes (scoped `git add` of the touched files).

## Task 14: live gate (operator-run; Docker)

Refresh containerized creds first (the launchd refresher should keep them fresh; verify). Use distinct
`run_id`s naturally (two processes). Serialize heavy jobs.

- [ ] **Concurrent same-repo:** two `a2a-bridge implement` runs against the SAME repo + SAME config at once →
both complete; assert no `docker run --name` failure and that each run's containers (by `a2a.run`) survive
while the other runs (`docker ps --filter label=a2a.run=<each>`).
- [ ] **Crash recovery:** start a run, `kill -9` it mid-flight; confirm its lease lock is now free; start a
second same-owner run → its before-first-use `classify_sweep` reaps the orphan (Dead) while its own
containers survive.
- [ ] **Visibility:** `a2a-bridge containers list` shows both runs (Alive/Dead/stale + any legacy);
`containers reap --all-dead` reaps only Dead; `containers reap --force <name>` reaps a named live one.
- [ ] Record evidence for the ADR.

## Task 15: ADR-0025 + finish

- [ ] Write `docs/adr/0025-concurrent-runs.md` (the flock-lease model, the atomic-flip constraint, the
deferred B items) — carries the `Co-Authored-By` trailer.
- [ ] finishing-a-development-branch: verify tests, merge `feat/concurrent-runs-a` → main, push, memory.

---

## After the build

Plan dual-review (containerized `plan-review` + a2a-local codex backstop) BEFORE building; fold a rev2 if
needed. Then inline/subagent TDD build (S1→S5), the gate (T13), live gate (T14), merge, memory, ADR-0025.

## Self-review (writing-plans)

- **Spec coverage:** D1 labels (T1–T2) + RunHandle/name (T3); D2 run_id-in-name + atomic constraint (S3
  note + T7/T10); D3 flock liveness + classify (T4–T5); D4 run_scoped_reap / classify_sweep / before-first-
  use + END guards (T6, T9); D5 staleness (last_output_age T6, list T11, reap --stale T12); D6 containers
  list|reap (T11–T12); D7 both :rw/:ro (T7–T8) + serve-as-unit (one RunHandle/lease per process, T9). All
  covered. Legacy orphans (T11 list / T12 --force). Host label (T1, classify T5).
- **Type consistency:** `ContainerLabels`/`RunHandle`/`Verdict`/`classify`/`LeaseProbe`/`acquire_lease`/
  `a2a_name`/`a2a_label_args`/`by_run_filter_argv`/`classify_sweep`/`run_scoped_reap` names are used
  consistently across tasks.
- **No placeholders:** the pure cores carry complete code; the S3 wiring tasks give exact mint sites +
  signatures (the one intentional prose stub is `last_output_age_secs`'s docker shell-out, with its PURE
  parse split out + tested — T6).
