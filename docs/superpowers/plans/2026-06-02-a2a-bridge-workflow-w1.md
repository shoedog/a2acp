# Workflow-DAG Orchestration (W1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a greenfield workflow-DAG orchestration capability (`crates/bridge-workflow`) — run a named DAG of agent-task nodes (fan-out/pipeline/fan-in) over the existing registry — plus the `code-review` instance (fan-out [codex, claude] → `synth` rollup), invoked by an A2A streaming skill or the `run-workflow` CLI.

**Architecture:** A new `bridge-workflow` crate holds the graph types + a `WorkflowExecutor` that runs each node via the existing `AgentRegistry`/`AgentBackend::prompt` (concatenating `Update::Text`; `configure_session` per node; cancel via a `CancellationToken`). Workflows are parsed **once at boot** from `[[workflows]]` TOML (load-once; no hot-reload). An A2A `skill="<wf-id>"` routes to a new `RouteTarget::Workflow` (streaming-only; unary rejects); a thin `run-workflow` CLI calls the same executor.

**Tech Stack:** Rust. `bridge-core` ports (`AgentRegistry`, `AgentBackend`, `Update`, `effective_config`), `tokio`/`futures`, `tokio_util::sync::CancellationToken`, `serde`/`toml`. New dev-dep in tests: none beyond workspace. Spec: `docs/superpowers/specs/2026-06-02-a2a-bridge-workflow-orchestration-design.md` (rev2).

**Conventions (project standing rules):** subagent task commits do **NOT** add a `Co-Authored-By` trailer (only the ADR commit does, Task 11). Coverage measured **after** `cargo llvm-cov clean --workspace`. `~/code/a2a-local-bridge` is firewall-black-box. Every task ends green: `cargo build`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test` (touched crate).

**Plan status — folds the spec's rev2 corrections:** the executor does NOT reuse `Translator::run` (its artifact is `last_text` → would drop content); cancellation is explicit (`backend.cancel` per in-flight node, not stream-drop); workflows load-once; triggers streaming-only.

---

## Ordering & green-per-task (the API-backend lesson)

**Phase A (Tasks 0-6) — the `bridge-workflow` crate + the two new ids.** This builds against the *current* `bridge-core` and never references `RouteTarget`, so it is immune to the Phase-B ripple. Each task leaves the touched crates green.

**Phase B (Tasks 7-11) — wiring + the atomic ripple.** Adding `RouteTarget::Workflow` re-expands the enum and breaks **every exhaustive match on `RouteTarget`** at once (`server.rs:454`, `:1041`, plus `local_agent_id:271`, `if-let:1035`, `cancel_task`). There is **no compiling intermediate**, so the variant + all its arms + the producer + the `InboundServer` fields it needs land in **ONE atomic commit (Task 9)**. Config parsing (Task 7) and the crate (Phase A) are additive and precede it.

## File Structure

**New crate `crates/bridge-workflow/`:** `Cargo.toml`; `src/lib.rs` (`pub mod graph; pub mod template; pub mod executor;` + re-exports); `src/graph.rs` (types + validation); `src/template.rs` (single-pass `{{var}}`); `src/executor.rs` (`WorkflowExecutor` + events + node-turn runner); `tests/executor_e2e.rs`.

**Modified `bridge-core`:** `src/ids.rs` (+ `WorkflowId`, `NodeId`, strict); `src/domain.rs` (+ `RouteTarget::Workflow`).

**Modified `bin/a2a-bridge`:** `src/config.rs` (`[[workflows]]` parse + boot-load); `src/route.rs` (`SkillRoute` workflow set); `src/main.rs` (wire executor + map + CLI); a new `run-workflow` subcommand.

**Modified `crates/bridge-a2a-inbound`:** `src/server.rs` (`spawn_workflow_producer`, the `RouteTarget::Workflow` arms, `cancel_task` arm, `InboundServer` fields); `src/card.rs` (workflow skills).

**New:** `prompts/review-codex.md`, `prompts/review-claude.md`, `prompts/review-synth.md`; `docs/adr/0009-workflow-orchestration.md`.

---

## Task 0: Branch + the two ids + crate skeleton

**Files:** Create `crates/bridge-workflow/Cargo.toml`, `src/lib.rs`, `src/{graph,template,executor}.rs` (placeholders); Modify `crates/bridge-core/src/ids.rs`.

- [ ] **Step 1: Branch**
```bash
cd /Users/wesleyjinks/code/a2a-bridge && git checkout main && git checkout -b feat/workflow-w1
```

- [ ] **Step 2: Write the failing id test.** Add to `crates/bridge-core/src/ids.rs` `mod tests`:
```rust
#[test]
fn strict_ids_reject_non_charset() {
    assert!(WorkflowId::parse("code-review").is_ok());
    assert!(NodeId::parse("synth_1").is_ok());
    assert!(WorkflowId::parse("").is_err());
    assert!(NodeId::parse("has space").is_err());
    assert!(NodeId::parse("br{{ace").is_err());
    assert!(WorkflowId::parse("UPPER").is_err()); // lowercase only
}
```

- [ ] **Step 3: Run → fail:** `cargo test -p bridge-core ids::` → FAIL (`WorkflowId` not found).

- [ ] **Step 4: Implement.** In `ids.rs`, add a strict macro + the two ids (after the `id_newtype!` macro):
```rust
macro_rules! id_newtype_strict {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name(String);
        impl $name {
            /// Validated id: non-empty and `[a-z0-9_-]+` only. Stricter than the plain
            /// id_newtype because these ids are interpolated into `{{<id>}}` template tokens.
            pub fn parse(s: impl Into<String>) -> Result<Self, BridgeError> {
                let s = s.into();
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') {
                    return Err(BridgeError::InvalidRequest { field: stringify!($name) });
                }
                Ok(Self(s))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
    };
}
id_newtype_strict!(WorkflowId);
id_newtype_strict!(NodeId);
```

- [ ] **Step 5: Run → pass:** `cargo test -p bridge-core ids::` → PASS.

- [ ] **Step 6: Create the crate.** `crates/bridge-workflow/Cargo.toml`:
```toml
[package]
name = "bridge-workflow"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
bridge-core = { path = "../bridge-core" }
tokio = { workspace = true }
tokio-util = { workspace = true }
futures.workspace = true
async-stream.workspace = true
tokio-stream.workspace = true
serde.workspace = true
tracing.workspace = true
async-trait.workspace = true

[dev-dependencies]
tokio = { workspace = true }
tokio-test = { workspace = true }
```
`src/lib.rs`:
```rust
//! bridge-workflow — workflow-DAG agent orchestration (W1). See
//! docs/superpowers/specs/2026-06-02-a2a-bridge-workflow-orchestration-design.md
pub mod executor;
pub mod graph;
pub mod template;
```
Create `src/graph.rs`, `src/template.rs`, `src/executor.rs` each with a single line `// placeholder`.

- [ ] **Step 7: Verify + commit:** `cargo build -p bridge-workflow && cargo build -p bridge-core`
```bash
git add crates/bridge-workflow crates/bridge-core/src/ids.rs Cargo.lock
git commit -m "feat(workflow): WorkflowId/NodeId strict ids + bridge-workflow skeleton"
```

---

## Task 1: Graph types + validation (`graph.rs`)

**Files:** Modify `crates/bridge-workflow/src/graph.rs`.

- [ ] **Step 1: Write the failing tests.** Append to `graph.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::ids::{AgentId, NodeId, WorkflowId};

    fn node(id: &str, agent: &str, inputs: &[&str]) -> WorkflowNode {
        WorkflowNode {
            id: NodeId::parse(id).unwrap(),
            agent: AgentId::parse(agent).unwrap(),
            prompt_template: format!("{{{{input}}}} {}", id),
            inputs: inputs.iter().map(|i| NodeId::parse(*i).unwrap()).collect(),
        }
    }

    #[test]
    fn valid_review_graph_has_single_terminal() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("code-review").unwrap(),
            nodes: vec![node("codex","codex",&[]), node("claude","claude",&[]), node("synth","claude",&["codex","claude"])],
        };
        g.validate().unwrap();
        assert_eq!(g.terminal().unwrap().id.as_str(), "synth");
    }
    #[test]
    fn rejects_cycle() {
        let g = WorkflowGraph { id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a","x",&["b"]), node("b","x",&["a"])] };
        assert!(matches!(g.validate(), Err(WorkflowError::Cyclic)));
    }
    #[test]
    fn rejects_multi_terminal() {
        let g = WorkflowGraph { id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a","x",&[]), node("b","x",&[])] };
        assert!(matches!(g.validate(), Err(WorkflowError::NotSingleTerminal(_))));
    }
    #[test]
    fn rejects_unknown_input_ref() {
        let g = WorkflowGraph { id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a","x",&["ghost"])] };
        assert!(matches!(g.validate(), Err(WorkflowError::UnknownInput { .. })));
    }
    #[test]
    fn rejects_duplicate_node_id() {
        let g = WorkflowGraph { id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a","x",&[]), node("a","x",&[])] };
        assert!(matches!(g.validate(), Err(WorkflowError::DuplicateNode(_))));
    }
}
```

- [ ] **Step 2: Run → fail:** `cargo test -p bridge-workflow graph::` → FAIL (types missing).

- [ ] **Step 3: Implement.** Replace `// placeholder` in `graph.rs`:
```rust
//! Workflow DAG types + validation. Edges are implicit from each node's `inputs`.
use bridge_core::ids::{AgentId, NodeId, WorkflowId};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct WorkflowGraph {
    pub id: WorkflowId,
    pub nodes: Vec<WorkflowNode>,
}

#[derive(Debug, Clone)]
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,
    pub prompt_template: String,
    pub inputs: Vec<NodeId>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkflowError {
    Empty,
    DuplicateNode(String),
    UnknownInput { node: String, input: String },
    Cyclic,
    NotSingleTerminal(usize),
}

impl WorkflowGraph {
    /// Validate: non-empty, unique node ids, all `inputs` reference real nodes,
    /// acyclic, exactly one terminal (no other node lists it in `inputs`).
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.nodes.is_empty() { return Err(WorkflowError::Empty); }
        let mut seen = HashSet::new();
        for n in &self.nodes {
            if !seen.insert(n.id.as_str()) { return Err(WorkflowError::DuplicateNode(n.id.as_str().into())); }
        }
        let ids: HashSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        for n in &self.nodes {
            for inp in &n.inputs {
                if !ids.contains(inp.as_str()) {
                    return Err(WorkflowError::UnknownInput { node: n.id.as_str().into(), input: inp.as_str().into() });
                }
            }
        }
        self.assert_acyclic()?;
        let referenced: HashSet<&str> = self.nodes.iter().flat_map(|n| n.inputs.iter().map(|i| i.as_str())).collect();
        let terminals = self.nodes.iter().filter(|n| !referenced.contains(n.id.as_str())).count();
        if terminals != 1 { return Err(WorkflowError::NotSingleTerminal(terminals)); }
        Ok(())
    }

    /// The single terminal node (call only after `validate`).
    pub fn terminal(&self) -> Option<&WorkflowNode> {
        let referenced: HashSet<&str> = self.nodes.iter().flat_map(|n| n.inputs.iter().map(|i| i.as_str())).collect();
        self.nodes.iter().find(|n| !referenced.contains(n.id.as_str()))
    }

    fn assert_acyclic(&self) -> Result<(), WorkflowError> {
        // Kahn's algorithm: repeatedly remove nodes whose inputs are all already removed.
        let mut remaining: Vec<&WorkflowNode> = self.nodes.iter().collect();
        let mut done: HashSet<&str> = HashSet::new();
        while !remaining.is_empty() {
            let ready: Vec<&str> = remaining.iter()
                .filter(|n| n.inputs.iter().all(|i| done.contains(i.as_str())))
                .map(|n| n.id.as_str()).collect();
            if ready.is_empty() { return Err(WorkflowError::Cyclic); }
            for r in &ready { done.insert(r); }
            remaining.retain(|n| !ready.contains(&n.id.as_str()));
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run → pass:** `cargo test -p bridge-workflow graph::` → PASS (5 tests); `cargo clippy -p bridge-workflow --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit:**
```bash
git add crates/bridge-workflow/src/graph.rs
git commit -m "feat(workflow): graph types + DAG validation (acyclic, single-terminal)"
```

---

## Task 2: Single-pass template (`template.rs`)

**Files:** Modify `crates/bridge-workflow/src/template.rs`.

- [ ] **Step 1: Write the failing tests.** Append:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    fn vars<'a>(p: &[(&'a str, &'a str)]) -> HashMap<&'a str, &'a str> { p.iter().cloned().collect() }

    #[test]
    fn substitutes_known_tokens() {
        let out = render("review {{input}} via {{codex}}", &vars(&[("input","DIFF"),("codex","OK")]));
        assert_eq!(out, "review DIFF via OK");
    }
    #[test]
    fn unknown_token_left_verbatim() {
        assert_eq!(render("a {{ghost}} b", &vars(&[("input","x")])), "a {{ghost}} b");
    }
    #[test]
    fn single_pass_no_reexpansion() {
        // codex's output literally contains "{{claude}}". A naive sequential replace would
        // expand it when {{claude}} is substituted next. Single-pass must NOT.
        let out = render("{{codex}}|{{claude}}", &vars(&[("codex","see {{claude}}"),("claude","REAL")]));
        assert_eq!(out, "see {{claude}}|REAL");
    }
}
```

- [ ] **Step 2: Run → fail:** `cargo test -p bridge-workflow template::` → FAIL.

- [ ] **Step 3: Implement.** Replace `// placeholder`:
```rust
//! Single-pass `{{var}}` template rendering. One left-to-right scan: each `{{token}}`
//! is replaced by `vars[token]` (or left verbatim if unknown). A substituted VALUE is
//! never re-scanned, so an upstream output containing `{{x}}` cannot be re-expanded.
use std::collections::HashMap;

pub fn render(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(close) = template[i + 2..].find("}}") {
                let token = &template[i + 2..i + 2 + close];
                match vars.get(token) {
                    Some(v) => out.push_str(v),          // value is NOT re-scanned
                    None => out.push_str(&template[i..i + 2 + close + 2]), // unknown: verbatim
                }
                i += 2 + close + 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
```
(Note: `bytes[i] as char` is safe for the ASCII branch bytes; the multibyte content passes through via `push_str` of `&str` slices. If clippy flags the `as char`, use `out.push(template[i..].chars().next().unwrap()); i += ch.len_utf8();` — but the `{{`/`}}` scan is ASCII-anchored so the simple form is correct for valid UTF-8 templates.)

- [ ] **Step 4: Run → pass:** `cargo test -p bridge-workflow template::` → PASS (3 tests); clippy clean.

- [ ] **Step 5: Commit:**
```bash
git add crates/bridge-workflow/src/template.rs
git commit -m "feat(workflow): single-pass {{var}} template rendering"
```

---

## Task 3: Executor + node-turn runner (single node)

**Files:** Modify `crates/bridge-workflow/src/executor.rs`.

The node-turn runner: `configure_session(effective_config)` → `backend.prompt` → concatenate `Update::Text` → `forget_session`. Cancel via a `CancellationToken`.

- [ ] **Step 1: Write the failing test.** Append to `executor.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, Part, RegistrySnapshot};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, NodeId, SessionId, WorkflowId};
    use bridge_core::ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update};
    use crate::graph::{WorkflowGraph, WorkflowNode};
    use futures::StreamExt;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    // A fake backend that records configure_session + the prompt it received, and replies with text.
    #[derive(Default)]
    struct Rec { configured: Mutex<bool>, prompts: Mutex<Vec<String>>, cancels: Mutex<u32> }
    struct FakeBackend { reply: String, rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(&self, _s: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
            self.rec.prompts.lock().unwrap().push(parts.iter().map(|p| p.text.clone()).collect());
            let updates = vec![Ok(Update::Text(self.reply.clone())),
                               Ok(Update::Done { stop_reason: "end_turn".into() })];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { *self.rec.cancels.lock().unwrap() += 1; Ok(()) }
        async fn configure_session(&self, _s: &SessionId, _c: &EffectiveConfig) -> Result<(), BridgeError> {
            *self.rec.configured.lock().unwrap() = true; Ok(())
        }
    }
    struct NoopLease; impl Lease for NoopLease {}
    struct FakeRegistry { backends: std::collections::HashMap<String, (String, Arc<Rec>)> }
    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let (reply, rec) = self.backends.get(id.as_str()).cloned()
                .ok_or(BridgeError::UnknownAgent { id: id.as_str().into() })?;
            Ok(Resolved {
                entry: Arc::new(bridge_core::domain::AgentEntry {
                    id: id.clone(), cmd: Some("x".into()), base_url: None, api_key_env: None, args: vec![],
                    kind: bridge_core::domain::AgentKind::Acp, model_provider: None, model: None, effort: None,
                    mode: None, cwd: None, auth_method: None, name: None, description: None, tags: vec![],
                    version: None, extensions: Default::default() }),
                backend: Arc::new(FakeBackend { reply, rec }),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId { AgentId::parse("codex").unwrap() }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
        fn list(&self) -> Vec<AgentId> { vec![] }
    }
    fn one_node_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph { id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode { id: NodeId::parse("only").unwrap(), agent: AgentId::parse("codex").unwrap(),
                prompt_template: "echo {{input}}".into(), inputs: vec![] }] })
    }

    #[tokio::test]
    async fn single_node_configures_renders_concatenates() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry { backends: [("codex".to_string(), ("HELLO".to_string(), rec.clone()))].into() });
        let ex = WorkflowExecutor::new(reg);
        let mut events: Vec<WorkflowEvent> = ex.run(one_node_graph(), "DIFF".into(), "run1".into(), CancellationToken::new())
            .collect::<Vec<_>>().await.into_iter().map(|r| r.unwrap()).collect();
        let term = events.pop().unwrap();
        assert!(matches!(term, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "HELLO"));
        assert!(*rec.configured.lock().unwrap(), "configure_session called");
        assert_eq!(rec.prompts.lock().unwrap()[0], "echo DIFF", "template rendered with {{input}}");
    }
}
```

- [ ] **Step 2: Run → fail:** `cargo test -p bridge-workflow executor::` → FAIL.

- [ ] **Step 3: Implement.** Replace `// placeholder` in `executor.rs`:
```rust
//! WorkflowExecutor — runs a validated DAG over the registry. Each node: configure_session
//! → prompt → concatenate Update::Text (NOT the translator's last_text). Cancel via token.
use crate::graph::{WorkflowGraph, WorkflowNode};
use crate::template::render;
use bridge_core::domain::{effective_config, Part};
use bridge_core::error::BridgeError;
use bridge_core::ids::{NodeId, SessionId};
use bridge_core::ports::{AgentRegistry, Update, STOP_REASON_CANCELLED};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct WorkflowExecutor { registry: Arc<dyn AgentRegistry> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutcome { Completed, Failed, Canceled }

#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    NodeStarted { node: NodeId },
    NodeFinished { node: NodeId, ok: bool },
    Terminal { outcome: WorkflowOutcome, output: String },
}

pub type WorkflowStream = std::pin::Pin<Box<dyn futures::Stream<Item = Result<WorkflowEvent, BridgeError>> + Send>>;

impl WorkflowExecutor {
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self { Self { registry } }

    /// Run one node: render its prompt from `vars`, resolve+configure+prompt+drain, forget.
    /// Returns (text, ok). On any failure returns the error marker + ok=false (caller decides
    /// terminal vs degradation). Cancellation → Err(BridgeError) mapped by the caller.
    async fn run_node(&self, node: &WorkflowNode, vars: &HashMap<&str, &str>, run_id: &str,
                      cancel: &CancellationToken) -> (String, bool) {
        let rendered = render(&node.prompt_template, vars);
        let session = match SessionId::parse(format!("workflow-{}-{}", node.id.as_str(), run_id)) {
            Ok(s) => s, Err(_) => return (format!("[node {} failed: bad session id]", node.id.as_str()), false),
        };
        let resolved = match self.registry.resolve(&node.agent).await {
            Ok(r) => r, Err(e) => return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false),
        };
        let eff = effective_config(&resolved.entry, None);
        let _ = resolved.backend.configure_session(&session, &eff).await;
        let mut stream = match resolved.backend.prompt(&session, vec![Part { text: rendered }]).await {
            Ok(s) => s, Err(e) => { resolved.backend.forget_session(&session).await;
                return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false); }
        };
        let mut text = String::new();
        let mut ok = true;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let _ = resolved.backend.cancel(&session).await;
                    ok = false; text = format!("[node {} canceled]", node.id.as_str()); break;
                }
                item = stream.next() => match item {
                    Some(Ok(Update::Text(t))) => text.push_str(&t),
                    Some(Ok(Update::Permission(_))) => {} // safe: backends resolve permission internally
                    Some(Ok(Update::Done { stop_reason })) => {
                        if stop_reason == STOP_REASON_CANCELLED { ok = false; }
                        break;
                    }
                    Some(Err(e)) => { ok = false; text = format!("[node {} failed: {:?}]", node.id.as_str(), e); break; }
                    None => break,
                }
            }
        }
        resolved.backend.forget_session(&session).await;
        (text, ok)
    }

    pub fn run(&self, graph: Arc<WorkflowGraph>, input: String, run_id: String, cancel: CancellationToken) -> WorkflowStream {
        let this = WorkflowExecutor { registry: self.registry.clone() };
        Box::pin(async_stream::stream! {
            // Task 4 fills topo scheduling. This single-node milestone runs nodes in order,
            // each consuming `{{input}}` only.
            let mut outputs: HashMap<String, (String, bool)> = HashMap::new();
            let mut terminal_output = String::new();
            let mut terminal_ok = true;
            for node in &graph.nodes {
                yield Ok(WorkflowEvent::NodeStarted { node: node.id.clone() });
                let mut vars: HashMap<&str, &str> = HashMap::new();
                vars.insert("input", input.as_str());
                for inp in &node.inputs {
                    if let Some((t, _)) = outputs.get(inp.as_str()) { vars.insert(inp.as_str(), t.as_str()); }
                }
                let (text, ok) = this.run_node(node, &vars, &run_id, &cancel).await;
                yield Ok(WorkflowEvent::NodeFinished { node: node.id.clone(), ok });
                terminal_output = text.clone(); terminal_ok = ok;
                outputs.insert(node.id.as_str().to_string(), (text, ok));
            }
            let outcome = if cancel.is_cancelled() { WorkflowOutcome::Canceled }
                else if terminal_ok { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed };
            yield Ok(WorkflowEvent::Terminal { outcome, output: terminal_output });
        })
    }
}
```
> Note: the borrow of `inp.as_str()` into `vars` requires `outputs` values to outlive the `vars` map within the loop iteration — they do (both live to the end of the iteration). If the borrow checker objects, clone into owned `String`s in a per-iteration `Vec` and borrow from that.

- [ ] **Step 4: Run → pass:** `cargo test -p bridge-workflow executor::` → PASS; clippy clean.

- [ ] **Step 5: Commit:**
```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "feat(workflow): executor + node-turn runner (configure/prompt/concat/forget)"
```

---

## Task 4: Topological scheduling (fan-out parallel / pipeline / fan-in)

**Files:** Modify `crates/bridge-workflow/src/executor.rs`.

- [ ] **Step 1: Write the failing tests.** Append to the `tests` mod:
```rust
fn review_graph() -> Arc<WorkflowGraph> {
    let n = |id: &str, ag: &str, ins: &[&str], tpl: &str| WorkflowNode {
        id: NodeId::parse(id).unwrap(), agent: AgentId::parse(ag).unwrap(),
        prompt_template: tpl.into(), inputs: ins.iter().map(|i| NodeId::parse(*i).unwrap()).collect() };
    Arc::new(WorkflowGraph { id: WorkflowId::parse("code-review").unwrap(), nodes: vec![
        n("codex","codex",&[], "review {{input}}"),
        n("claude","claude",&[], "review {{input}}"),
        n("synth","synth",&["codex","claude"], "merge {{codex}} + {{claude}}"),
    ]})
}
#[tokio::test]
async fn fan_in_synth_receives_both_reviews() {
    let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
    let reg = Arc::new(FakeRegistry { backends: [
        ("codex".to_string(), mk("CODEX_REVIEW")),
        ("claude".to_string(), mk("CLAUDE_REVIEW")),
        ("synth".to_string(), { let (_, rec) = mk("FINAL"); ("FINAL".to_string(), rec) }),
    ].into() });
    let synth_rec = reg.backends.get("synth").unwrap().1.clone();
    let ex = WorkflowExecutor::new(reg);
    let evs: Vec<_> = ex.run(review_graph(), "DIFF".into(), "r".into(), CancellationToken::new())
        .collect::<Vec<_>>().await;
    let last = evs.last().unwrap().as_ref().unwrap();
    assert!(matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "FINAL"));
    let synth_prompt = &synth_rec.prompts.lock().unwrap()[0];
    assert!(synth_prompt.contains("CODEX_REVIEW") && synth_prompt.contains("CLAUDE_REVIEW"),
        "synth got both reviews: {synth_prompt}");
}
```

- [ ] **Step 2: Run → fail** (the Task-3 sequential loop processes nodes in declaration order — for `review_graph` that *happens* to work, so this test may PASS already; if so, ADD a test that asserts the two fan-out legs run **concurrently**: give codex a backend whose `prompt` blocks on a shared barrier until claude's `prompt` is also entered — assert no deadlock/timeout. That proves parallelism, which the sequential loop fails).

```rust
#[tokio::test]
async fn fan_out_runs_concurrently() {
    use tokio::sync::Barrier;
    // Both legs must enter prompt() before either proceeds → only possible if run in parallel.
    let barrier = Arc::new(Barrier::new(2));
    // (implement a BarrierBackend variant that .wait()s in prompt() before replying;
    //  wrap the whole run in tokio::time::timeout(2s) and assert it completes.)
    let _ = barrier; // see Step 3 for the BarrierBackend
}
```

- [ ] **Step 3: Implement topo scheduling.** Replace the `run` body's `for node in &graph.nodes` loop with a ready-set scheduler that runs independent ready nodes via `futures::future::join_all` and unblocks downstream as outputs complete:
```rust
            let mut outputs: HashMap<String, (String, bool)> = HashMap::new();
            let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
            let terminal_id = graph.terminal().map(|n| n.id.as_str().to_string()).unwrap_or_default();
            while done.len() < graph.nodes.len() {
                let ready: Vec<&WorkflowNode> = graph.nodes.iter()
                    .filter(|n| !done.contains(n.id.as_str())
                        && n.inputs.iter().all(|i| done.contains(i.as_str())))
                    .collect();
                if ready.is_empty() { break; } // validated acyclic, so this can't happen
                for n in &ready { yield Ok(WorkflowEvent::NodeStarted { node: n.id.clone() }); }
                // Build each ready node's vars from completed outputs, then run all concurrently.
                let futs = ready.iter().map(|n| {
                    let mut owned: Vec<(String, String)> = vec![("input".into(), input.clone())];
                    for inp in &n.inputs {
                        if let Some((t, _)) = outputs.get(inp.as_str()) { owned.push((inp.as_str().into(), t.clone())); }
                    }
                    let node = (*n).clone(); let run_id = run_id.clone(); let cancel = cancel.clone(); let this = &this;
                    async move {
                        let vars: HashMap<&str, &str> = owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                        let (text, ok) = this.run_node(&node, &vars, &run_id, &cancel).await;
                        (node.id.as_str().to_string(), text, ok)
                    }
                });
                for (id, text, ok) in futures::future::join_all(futs).await {
                    yield Ok(WorkflowEvent::NodeFinished { node: NodeId::parse(&id).unwrap(), ok });
                    done.insert(id.clone());
                    outputs.insert(id, (text, ok));
                }
            }
            let (term_text, term_ok) = outputs.get(&terminal_id).cloned().unwrap_or_default();
            let outcome = if cancel.is_cancelled() { WorkflowOutcome::Canceled }
                else if term_ok { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed };
            yield Ok(WorkflowEvent::Terminal { outcome, output: term_text });
```
Add the `BarrierBackend` to the test module (a backend whose `prompt` `.wait()`s on a shared `Arc<Barrier>` before replying) to drive the concurrency test.

- [ ] **Step 4: Run → pass:** `cargo test -p bridge-workflow` (fan_in + fan_out_concurrently + earlier) → PASS; clippy clean.

- [ ] **Step 5: Commit:**
```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "feat(workflow): topological scheduling — fan-out parallel, pipeline, fan-in"
```

---

## Task 5: Node-failure degradation + terminal outcome

**Files:** Modify `crates/bridge-workflow/tests` (the executor test mod).

The logic exists (Task 3/4: failed node → marker into `outputs`; terminal-ok drives outcome). This task proves it.

- [ ] **Step 1: Write the failing test.** Append:
```rust
#[tokio::test]
async fn failed_fan_out_leg_marker_reaches_synth_and_run_completes() {
    // codex resolves to a MISSING agent id → run_node returns an error marker; synth still runs.
    let reg = Arc::new(FakeRegistry { backends: [
        ("claude".to_string(), ("CLAUDE_REVIEW".to_string(), Arc::new(Rec::default()))),
        ("synth".to_string(),  ("FINAL".to_string(),         Arc::new(Rec::default()))),
        // NOTE: no "codex" → resolve fails for the codex node
    ].into() });
    let synth_rec = reg.backends.get("synth").unwrap().1.clone();
    let ex = WorkflowExecutor::new(reg);
    let evs: Vec<_> = ex.run(review_graph(), "DIFF".into(), "r".into(), CancellationToken::new()).collect::<Vec<_>>().await;
    // run completes (terminal synth ok) — graceful degradation
    assert!(matches!(evs.last().unwrap().as_ref().unwrap(),
        WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, .. }));
    // a NodeFinished{ok:false} was emitted for codex
    assert!(evs.iter().any(|e| matches!(e.as_ref().unwrap(), WorkflowEvent::NodeFinished { node, ok:false } if node.as_str()=="codex")));
    // the exact failure marker reached synth's prompt
    let synth_prompt = &synth_rec.prompts.lock().unwrap()[0];
    assert!(synth_prompt.contains("[node codex failed:"), "marker reached synth: {synth_prompt}");
}
```

- [ ] **Step 2: Run → pass** (logic already implemented). If the marker isn't in synth's prompt, fix `run_node`/scheduling so a failed node's marker is stored in `outputs` and substituted downstream.

- [ ] **Step 3: Commit:**
```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "test(workflow): node-failure degradation — marker reaches synth, run completes"
```

---

## Task 6: Cancellation (token → backend.cancel per in-flight node)

**Files:** Modify the executor test mod.

The `run_node` already `select!`s on `cancel.cancelled()` → `backend.cancel`. This task proves the real cancel.

- [ ] **Step 1: Write the failing test.** Append:
```rust
#[tokio::test]
async fn cancel_calls_backend_cancel_and_ends_canceled() {
    // A backend whose prompt() stream never yields Done until cancelled (a pending stream).
    struct Pending { rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl AgentBackend for Pending {
        async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(futures::stream::pending())) // never yields
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { *self.rec.cancels.lock().unwrap()+=1; Ok(()) }
    }
    let rec = Arc::new(Rec::default());
    fn minimal_entry(id: &AgentId) -> bridge_core::domain::AgentEntry {
        bridge_core::domain::AgentEntry {
            id: id.clone(), cmd: Some("x".into()), base_url: None, api_key_env: None, args: vec![],
            kind: bridge_core::domain::AgentKind::Acp, model_provider: None, model: None, effort: None,
            mode: None, cwd: None, auth_method: None, name: None, description: None, tags: vec![],
            version: None, extensions: Default::default() }
    }
    struct PReg { rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl AgentRegistry for PReg {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved { entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(Pending { rec: self.rec.clone() }), lease: Box::new(NoopLease) })
        }
        fn default_id(&self) -> AgentId { AgentId::parse("a").unwrap() }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
        fn list(&self) -> Vec<AgentId> { vec![] }
    }
    let token = CancellationToken::new();
    let reg = Arc::new(PReg { rec: rec.clone() });
    let ex = WorkflowExecutor::new(reg);
    let t2 = token.clone();
    tokio::spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(20)).await; t2.cancel(); });
    let evs: Vec<_> = tokio::time::timeout(std::time::Duration::from_secs(2),
        ex.run(one_node_graph(), "x".into(), "r".into(), token).collect::<Vec<_>>()).await.unwrap();
    assert!(matches!(evs.last().unwrap().as_ref().unwrap(), WorkflowEvent::Terminal { outcome: WorkflowOutcome::Canceled, .. }));
    assert_eq!(*rec.cancels.lock().unwrap(), 1, "backend.cancel was called for the in-flight node");
}
```
(Fill the `PReg` `AgentEntry` from the `FakeRegistry` literal; the point is the `Pending` backend + the cancel assertion.)

- [ ] **Step 2: Run → pass** (the select! cancel path exists). Fix if `backend.cancel` isn't called.

- [ ] **Step 3: Commit:**
```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "test(workflow): cancellation calls backend.cancel per in-flight node, ends Canceled"
```

---

## Task 7: `[[workflows]]` config parse + boot-load

**Files:** Modify `bin/a2a-bridge/src/config.rs`.

Parse `[[workflows]]` (load each `prompt_file`'s contents) into validated `WorkflowGraph`s. Additive — does NOT touch `RouteTarget`.

- [ ] **Step 1: Add `bridge-workflow` dep** to `bin/a2a-bridge/Cargo.toml` `[dependencies]`: `bridge-workflow = { path = "../../crates/bridge-workflow" }`.

- [ ] **Step 2: Write the failing test.** Add to `config.rs` tests (note `[server]` is required; prompt files via a `tempfile`):
```rust
#[test]
fn parses_workflows_and_loads_prompts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.md"), "review {{input}}").unwrap();
    let toml = format!(r#"
default = "codex"
[[agents]]
id = "codex"
cmd = "codex-acp"
[[workflows]]
id = "wf1"
  [[workflows.nodes]]
  id = "only"; agent = "codex"; prompt_file = "p.md"; inputs = []
[server]
addr = "127.0.0.1:8080"
"#);
    let cfg = RegistryConfig::parse(&toml).unwrap();
    let wfs = cfg.load_workflows(dir.path()).unwrap();   // base dir for relative prompt_file
    let g = wfs.get(&bridge_core::ids::WorkflowId::parse("wf1").unwrap()).unwrap();
    assert_eq!(g.nodes[0].prompt_template, "review {{input}}");
    g.validate().unwrap();
}
```
(Add `tempfile` to `bin/a2a-bridge` `[dev-dependencies]` if absent.)

- [ ] **Step 3: Implement.** In `config.rs`: a raw `WorkflowToml`/`WorkflowNodeToml` (serde) on `RegistryConfig`, and `load_workflows(&self, base: &Path) -> Result<HashMap<WorkflowId, Arc<WorkflowGraph>>, ConfigError>` that, for each `[[workflows]]`: parse the `WorkflowId`/`NodeId`/`AgentId`s (propagate the strict-id errors), read each `prompt_file` (joined onto `base`; **missing/unreadable → `ConfigError`**), build the `WorkflowGraph`, and `graph.validate()` (**bad DAG → `ConfigError`**). Full serde structs + the loader:
```rust
#[derive(Debug, serde::Deserialize)]
pub struct WorkflowToml { pub id: String, #[serde(default)] pub nodes: Vec<WorkflowNodeToml> }
#[derive(Debug, serde::Deserialize)]
pub struct WorkflowNodeToml { pub id: String, pub agent: String, pub prompt_file: String, #[serde(default)] pub inputs: Vec<String> }
// add to RegistryConfig: #[serde(default)] pub workflows: Vec<WorkflowToml>

impl RegistryConfig {
    pub fn load_workflows(&self, base: &std::path::Path)
        -> Result<std::collections::HashMap<bridge_core::ids::WorkflowId, std::sync::Arc<bridge_workflow::graph::WorkflowGraph>>, ConfigError>
    {
        use bridge_core::ids::{AgentId, NodeId, WorkflowId};
        use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
        let mut map = std::collections::HashMap::new();
        for w in &self.workflows {
            let id = WorkflowId::parse(w.id.clone()).map_err(|e| ConfigError::Registry(format!("workflow id {:?}: {e:?}", w.id)))?;
            let mut nodes = Vec::with_capacity(w.nodes.len());
            for n in &w.nodes {
                let tpl = std::fs::read_to_string(base.join(&n.prompt_file))
                    .map_err(|e| ConfigError::Registry(format!("workflow {} node {} prompt_file {:?}: {e}", w.id, n.id, n.prompt_file)))?;
                nodes.push(WorkflowNode {
                    id: NodeId::parse(n.id.clone()).map_err(|e| ConfigError::Registry(format!("node id {:?}: {e:?}", n.id)))?,
                    agent: AgentId::parse(n.agent.clone()).map_err(|e| ConfigError::Registry(format!("node agent {:?}: {e:?}", n.agent)))?,
                    prompt_template: tpl,
                    inputs: n.inputs.iter().map(|i| NodeId::parse(i.clone())).collect::<Result<_,_>>()
                        .map_err(|e| ConfigError::Registry(format!("workflow {} input id: {e:?}", w.id)))?,
                });
            }
            let g = WorkflowGraph { id: id.clone(), nodes };
            g.validate().map_err(|e| ConfigError::Registry(format!("workflow {} invalid: {e:?}", w.id)))?;
            map.insert(id, std::sync::Arc::new(g));
        }
        Ok(map)
    }
}
```

- [ ] **Step 4: Run → pass:** `cargo test -p a2a-bridge config::parses_workflows`. (Workspace still green — additive.) clippy clean.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/src/config.rs bin/a2a-bridge/Cargo.toml Cargo.lock
git commit -m "feat(config): parse [[workflows]] + boot-load prompt files into validated graphs"
```

---

## Task 8: SkillRoute workflow set (additive)

**Files:** Modify `bin/a2a-bridge/src/route.rs`.

Give `SkillRoute` the boot workflow-id set so a workflow-id skill can route. (The `RouteTarget::Workflow` *variant* is added atomically in Task 9; here `SkillRoute` just gains the set + a method that the Task-9 arm uses — to keep this additive, `SkillRoute` stores the set now and Task 9 adds the routing arm.)

- [ ] **Step 1:** Add a field `workflows: std::collections::HashSet<String>` to `SkillRoute` + a constructor `SkillRoute::with_workflows(registry, ids)`; keep `new` delegating with an empty set. Add a test that `with_workflows` stores the ids (and a `knows_workflow(&str) -> bool` helper).

- [ ] **Step 2-4:** TDD the helper; `cargo test -p a2a-bridge route::`; clippy clean.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/src/route.rs
git commit -m "feat(route): SkillRoute carries the boot workflow-id set"
```

---

## Task 9: ATOMIC — `RouteTarget::Workflow` ripple + producer + wiring + card

**Files (ONE commit):** `crates/bridge-core/src/domain.rs`, `crates/bridge-a2a-inbound/src/server.rs`, `crates/bridge-a2a-inbound/src/card.rs`, `bin/a2a-bridge/src/route.rs`, `bin/a2a-bridge/src/main.rs`, `crates/bridge-a2a-inbound/Cargo.toml` (+ `bridge-workflow` dep).

**Why atomic:** adding `RouteTarget::Workflow` breaks the exhaustive matches at `server.rs:454` and `:1041` simultaneously; `spawn_workflow_producer` needs `InboundServer` to hold the executor + workflow map; so the variant, all arms, the producer, and the field-wiring land together. Use the compiler as the checklist (`cargo build --workspace` → fix every error).

- [ ] **Step 1: Write the failing test** (the A2A streaming e2e — it won't compile until the ripple lands; that's the red). Outline in `crates/bridge-a2a-inbound/tests/` or `bin/a2a-bridge/tests/`:
```rust
// A streaming skill="code-review" task over a FAKE registry (codex/claude/synth fakes) →
// assert: node Status events; final Artifact == synth output; terminal Completed;
// AND a unary skill="code-review" send → InvalidRequest.
```
(Use the existing inbound test harness pattern — mirror an existing fanout e2e in `server.rs` tests / `bin/a2a-bridge/tests`.)

- [ ] **Step 2: `cargo build --workspace` → compile errors** (non-exhaustive matches). This is the checklist.

- [ ] **Step 3: Implement the ripple (all sites):**
  1. **`bridge-core/src/domain.rs`** — `RouteTarget`: add `Workflow(crate::ids::WorkflowId)`.
  2. **`bridge-a2a-inbound/Cargo.toml`** — add `bridge-workflow = { path = "../bridge-workflow" }`.
  3. **`InboundServer`** — add fields `executor: Arc<bridge_workflow::executor::WorkflowExecutor>` and `workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>`; thread them through the constructor.
  4. **`server.rs:454` `stream_message` match** — add `RouteTarget::Workflow(id) => spawn_workflow_producer(&srv, routed, id, tx),`.
  5. **`server.rs:1041` `unary_message` match** — add `RouteTarget::Workflow(_) => return bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest { field: "skill" }),` (streaming-only).
  6. **`server.rs:271` `local_agent_id`** — handle `Workflow` explicitly (it has no single agent id; this helper is only called on `Local` paths, so `Workflow => unreachable!("workflow not a local target")` or restructure — verify the call sites).
  7. **`server.rs:1035` `if let RouteTarget::Fanout`** — confirm the workflow case isn't mis-handled (the unary path rejects workflow in 5, so this pre-dispatch is fine; add a guard if needed).
  8. **`cancel_task` (`:1294`)** — add handling so a workflow task latches `request_cancel` (already always called at `:1292`) and returns without the local-backend no-op (mark workflow tasks, or detect via a workflow-task set; the producer does the node cancels via `poll_cancel_requested`).
  9. **`spawn_workflow_producer`** (new, mirror `spawn_fanout_producer`): build `input` from `routed`'s parts; `run_id = task.as_str()`; create a `CancellationToken`; spawn a task that `poll_cancel_requested(&store, &task).await` → `token.cancel()`; consume `executor.run(graph, input, run_id, token)` mapping `NodeStarted/NodeFinished` → `Event` Status (labeled by node id), `Terminal{Completed,output}` → Artifact + `Event::terminal(TaskOutcome::Completed)`, `Terminal{Failed,..}` → `Event::terminal(TaskOutcome::Failed)`, `Terminal{Canceled,..}` → `Event::terminal(TaskOutcome::Canceled)`; send each `Event` to `tx`. (`spawn_workflow_producer` looks up the graph by `id` from `srv.workflows`; an unknown id → terminal `Failed`.)
  10. **`route.rs` `SkillRoute::route`** — add the precedence arm: `else if self.workflows.contains(skill) => RouteTarget::Workflow(WorkflowId::parse(skill)?)`.
  11. **`card.rs:84`** — push one `AgentSkill` per workflow id into `skills` (the builder takes the workflow-id set).
  12. **`main.rs`** — `cfg.load_workflows(config_dir)` → the map; build `WorkflowExecutor::new(registry.clone())`; pass both into `InboundServer::new(...)` and `SkillRoute::with_workflows(...)` and the card builder.

- [ ] **Step 4: Make the WHOLE workspace green:** `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Fix every error the compiler names.

- [ ] **Step 5: ONE atomic commit:**
```bash
git add crates/bridge-core/src/domain.rs crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/src/card.rs \
        crates/bridge-a2a-inbound/Cargo.toml bin/a2a-bridge/src/route.rs bin/a2a-bridge/src/main.rs Cargo.lock
git commit -m "feat(workflow): RouteTarget::Workflow ripple + spawn_workflow_producer + card skills (atomic)"
```

---

## Task 10: The `run-workflow` CLI + the `code-review` instance + e2e

**Files:** `bin/a2a-bridge/src/main.rs` (subcommand), `prompts/review-*.md`, the project config example, tests.

- [ ] **Step 1: `run-workflow` subcommand.** In `main.rs`, add `a2a-bridge run-workflow <id> --input <file> [--out <file>]`: load config + `load_workflows`, build the executor, generate a fresh `run_id` (`uuid`-style — use `std::process::id()` + a monotonic counter or a timestamp if no uuid dep; a unique string suffices), read `--input`, run `executor.run`, print `NodeStarted/NodeFinished` to stderr + terminal output to stdout/`--out`. Test: a `run-workflow` integration test over fake agents asserting the synth output is printed.

- [ ] **Step 2: The prompt files.** Create `prompts/review-codex.md`, `prompts/review-claude.md`, `prompts/review-synth.md`. Codex = blockers/correctness/regressions/test-gaps lens (`{{input}}`); Claude = architecture/seams/design lens (`{{input}}`); synth = merge `{{codex}}` + `{{claude}}` into one de-duplicated review weighted by the complementary roles (may reference `{{input}}`). Keep them focused.

- [ ] **Step 3: The config entry.** Add the `code-review` `[[workflows]]` block (§2 of the spec) to the example/dev config the bridge loads.

- [ ] **Step 4: The A2A streaming e2e + unary reject + terminal Failed** (the Task-9 Step-1 test, now compiling): assert node Status events, synth Artifact, terminal Completed, **synth prompt recorded BOTH reviews**; a unary `code-review` send → `InvalidRequest`; a terminal-node failure → `Failed`.

- [ ] **Step 5: Verify + commit:** `cargo test --workspace` green; clippy clean.
```bash
git add bin/a2a-bridge/src/main.rs prompts/ <config-example> bin/a2a-bridge/tests/
git commit -m "feat(workflow): run-workflow CLI + code-review instance + A2A streaming e2e (DoD-9/10/11)"
```

---

## Task 11: CI floor + ADR-0009 + final verification

- [ ] **Step 1: CI floor.** In `.github/workflows/ci.yml`, after the `bridge-api` coverage step:
```yaml
      - name: Coverage — bridge-workflow (≥90% line coverage)
        run: cargo llvm-cov --package bridge-workflow --fail-under-lines 90
```

- [ ] **Step 2: Verify coverage** (after clean):
```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --package bridge-workflow --fail-under-lines 90
cargo llvm-cov --workspace --fail-under-lines 85
```
If `bridge-workflow` < 90, add deterministic executor tests for the uncovered arms (the gated live test adds no coverage).

- [ ] **Step 3: ADR-0009** — create `docs/adr/0009-workflow-orchestration.md`: the workflow-DAG capability (chain-of-brains, ADR-0008 re-trigger #5 first build), the rev2 design corrections (Translator NOT reused — `last_text` would drop content; cancellation explicit; load-once; streaming-only), and that this is W1 of the self-hosting program (W2-4 deferred). **Controller doc — trailer REQUIRED.**

- [ ] **Step 4: Full sweep:** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

- [ ] **Step 5: Commits:**
```bash
git add .github/workflows/ci.yml && git commit -m "ci: bridge-workflow 90% line-coverage floor"
git add docs/adr/0009-workflow-orchestration.md
git commit -m "$(cat <<'EOF'
docs(adr): 0009 — workflow-DAG orchestration (W1)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: DoD checklist** (spec §9): DoD-1..12 each map to a passing test (DoD-12 gated `#[ignore]`). Hand back for holistic review + `finishing-a-development-branch`.

---

## Self-Review notes (controller)

- **Spec coverage:** §2 model → Tasks 0/1; §3 executor/template → Tasks 2/3/4; §3 node-failure → Task 5; §3/§6 cancel → Task 6; §4.1 ripple+producer+card → Task 9; §4.2 CLI → Task 10; §4.3 SkillRoute → Tasks 8/9; §5 instance → Task 10; §6 load-once config → Task 7; §6 terminal outcome → Tasks 4/9; §9 DoD-1..12 → Tasks 3-6/9/10/11; coverage → Task 11; ADR → Task 11. No gap.
- **Green-per-task:** Phase A (0-6) builds against current `bridge-core` (never references `RouteTarget`). Task 7/8 are additive. The `RouteTarget::Workflow` ripple is the single atomic Task 9 (no compiling intermediate) — the API-backend lesson applied.
- **TDD; one logical change per commit; no `Co-Authored-By` on subagent commits (Task 11 ADR excepted).**
