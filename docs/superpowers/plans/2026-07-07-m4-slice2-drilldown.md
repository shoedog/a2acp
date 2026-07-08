# M4 Slice 2 — Drill-down HTTP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use `- [ ]` checkbox syntax.

**Goal:** Add owner-approved, authenticated drill-down HTTP routes for turn rows, task journals, and node artifacts, plus trace refs and uncapped turn-log usage in status DTOs.
**Architecture:** `bridge-core` owns Prometheus-free store types and bounded read ports; `bridge-store` implements the bounded SQLite reads under one connection guard. `bridge-coordinator` builds usage and relative trace refs from `turn_log` and checkpoint metadata; `bridge-a2a-inbound` owns auth, content headers, audit logging, route parsing, and response status matrices.
**Tech Stack:** Rust 1.94, axum 0.7, rusqlite (bundled), tokio, serde.

## Global Constraints

- Toolchain `1.94.0`.
- CI gates fmt (`-D warnings`) + clippy + full `--workspace` test + `cargo deny`.
- Local triad = fmt+clippy+test (`-j 1` to avoid `--all-targets` OOM).
- Metrics/trace surfaces opt-in, default OFF.
- No new unauthenticated HTTP (existing loopback bearer auth).
- `prometheus` types never leak into `bridge-core` ports/DTOs — confined to `bridge-observ`.
- High-cardinality ids (`task_id`/`context_id`/`turn_id`/`prompt_id`) are never Prometheus labels — they live in the turn-log / trace surfaces.

---

## File Structure

- `bin/a2a-bridge/src/config.rs` — parse and validate `[traces]`; expose `TracesConfig`.
- `bin/a2a-bridge/src/main.rs` — install `TurnLogObserver` when traces need durable rows; pass trace config into `Coordinator` and `InboundServer`.
- `crates/bridge-core/src/ids.rs` — make strict ids orderable for `BTreeSet<NodeId>`.
- `crates/bridge-core/src/task_store.rs` — add bounded drill-down read DTOs and `TaskStore` methods; implement them for `MemoryTaskStore`.
- `crates/bridge-store/src/sqlite.rs` — implement bounded SQLite reads for turn rows, usage rollups, journals, and node artifacts.
- `crates/bridge-coordinator/src/detached.rs` — add `workflow_spec_node_ids` beside `WorkflowSpecEnvelope`.
- `crates/bridge-coordinator/src/coordinator.rs` — add `TraceRefs`, optional usage/trace DTO fields, percent-encoded refs, and async status builders.
- `crates/bridge-a2a-inbound/Cargo.toml` — add `tracing-subscriber` dev-dependency for audit-log tests.
- `crates/bridge-a2a-inbound/src/server.rs` — add `TraceHttpConfig`, mount routes, implement handlers, headers, gates, and integration tests.

Implementation choice: keep `Coordinator::new` source-compatible and add `Coordinator::with_trace_refs_config(enabled, max_task_turns)`; the seam is still stored as `trace_refs_enabled: bool` plus `max_task_turns` fields on `Coordinator`.

---

### Task 1: `[traces]` Config, `TraceHttpConfig`, and Turn-Log Install Predicate

**Files:** Modify `bin/a2a-bridge/src/config.rs:91`, `bin/a2a-bridge/src/config.rs:146`, `bin/a2a-bridge/src/config.rs:1116`, `bin/a2a-bridge/src/config.rs:1684`; modify `bin/a2a-bridge/src/main.rs:674`, `bin/a2a-bridge/src/main.rs:6085`, `bin/a2a-bridge/src/main.rs:6123`, `bin/a2a-bridge/src/main.rs:6232`; modify `crates/bridge-a2a-inbound/src/server.rs:85`, `crates/bridge-a2a-inbound/src/server.rs:135`, `crates/bridge-a2a-inbound/src/server.rs:185`.
**Interfaces:** Consumes existing `RegistryConfig::parse(&str) -> Result<RegistryConfig, ConfigError>`, `MetricsConfig`, `InboundServer::with_metrics_endpoint`. Produces `TracesToml`, `TracesConfig`, `RegistryConfig::traces_config(&self) -> Result<TracesConfig, ConfigError>`, `bridge_a2a_inbound::server::TraceHttpConfig`, `TraceHttpConfig::default()`, `InboundServer::with_trace_http_config(mut self, config: TraceHttpConfig) -> Self`, `turn_log_observer_enabled(metrics_cfg: &config::MetricsConfig, traces_cfg: &config::TracesConfig) -> bool`.

- [ ] Step: write the failing tests.
```rust
// bin/a2a-bridge/src/config.rs, append near metrics_config_tests
#[cfg(test)]
mod traces_config_tests {
    use super::*;

    const BASE: &str = r#"
default = "codex"
[server]
addr = "127.0.0.1:0"
[[agents]]
id = "codex"
cmd = "codex"
"#;

    #[test]
    fn traces_config_defaults_disabled() {
        let cfg = RegistryConfig::parse(BASE).unwrap();
        let traces = cfg.traces_config().unwrap();

        assert!(!traces.enabled);
        assert_eq!(traces.journal_max_bytes, 16_777_216);
        assert_eq!(traces.journal_max_events, 100_000);
        assert_eq!(traces.artifact_max_bytes, 4_194_304);
        assert_eq!(traces.max_task_turns, 512);
    }

    #[test]
    fn traces_config_rejects_zero_limits() {
        for key in [
            "journal_max_bytes",
            "journal_max_events",
            "artifact_max_bytes",
            "max_task_turns",
        ] {
            let raw = format!(
                "{BASE}\n[traces]\nenabled = true\n{key} = 0\n"
            );
            let err = RegistryConfig::parse(&raw)
                .unwrap()
                .traces_config()
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("[traces]") && err.contains(key) && err.contains("> 0"),
                "unexpected error for {key}: {err}"
            );
        }
    }

    #[test]
    fn traces_config_independent_from_metrics() {
        let raw = format!(
            "{BASE}\n[metrics]\nenabled = false\nturn_log = false\n[traces]\nenabled = true\n"
        );
        let cfg = RegistryConfig::parse(&raw).unwrap();
        let metrics = cfg.metrics_config().unwrap();
        let traces = cfg.traces_config().unwrap();

        assert!(!metrics.enabled);
        assert!(!metrics.turn_log);
        assert!(traces.enabled);
    }
}
```

```rust
// bin/a2a-bridge/src/main.rs, append inside the existing #[cfg(test)] tests module
#[test]
fn turn_log_observer_enabled_for_traces_even_without_metrics() {
    let metrics = config::MetricsConfig {
        enabled: false,
        prometheus: false,
        turn_log: false,
    };
    let traces = config::TracesConfig {
        enabled: true,
        journal_max_bytes: 16,
        journal_max_events: 16,
        artifact_max_bytes: 16,
        max_task_turns: 2,
    };

    assert!(turn_log_observer_enabled(&metrics, &traces));
}

#[test]
fn turn_log_observer_enabled_for_metrics_turn_log_without_traces() {
    let metrics = config::MetricsConfig {
        enabled: true,
        prometheus: true,
        turn_log: true,
    };
    let traces = config::TracesConfig {
        enabled: false,
        journal_max_bytes: 16,
        journal_max_events: 16,
        artifact_max_bytes: 16,
        max_task_turns: 2,
    };

    assert!(turn_log_observer_enabled(&metrics, &traces));
}

#[test]
fn turn_log_observer_disabled_when_neither_surface_needs_rows() {
    let metrics = config::MetricsConfig {
        enabled: true,
        prometheus: true,
        turn_log: false,
    };
    let traces = config::TracesConfig {
        enabled: false,
        journal_max_bytes: 16,
        journal_max_events: 16,
        artifact_max_bytes: 16,
        max_task_turns: 2,
    };

    assert!(!turn_log_observer_enabled(&metrics, &traces));
}
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, append inside tests
#[test]
fn trace_http_config_defaults_disabled() {
    let cfg = TraceHttpConfig::default();

    assert!(!cfg.enabled);
    assert_eq!(cfg.journal_max_bytes, 16_777_216);
    assert_eq!(cfg.journal_max_events, 100_000);
    assert_eq!(cfg.artifact_max_bytes, 4_194_304);
    assert_eq!(cfg.max_task_turns, 512);
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p a2a-bridge traces_config_tests turn_log_observer_ -- --nocapture
cargo test -p bridge-a2a-inbound trace_http_config_defaults_disabled -- --nocapture
```
Expected FAIL: unresolved `TracesConfig`, `traces_config`, `turn_log_observer_enabled`, and `TraceHttpConfig`.

- [ ] Step: add `[traces]` config and validation.
```rust
// bin/a2a-bridge/src/config.rs, near default_metrics_exporters/default_true
fn default_journal_max_bytes() -> usize {
    16_777_216
}

fn default_journal_max_events() -> usize {
    100_000
}

fn default_artifact_max_bytes() -> usize {
    4_194_304
}

fn default_max_task_turns() -> usize {
    512
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TracesToml {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_journal_max_bytes")]
    pub journal_max_bytes: usize,
    #[serde(default = "default_journal_max_events")]
    pub journal_max_events: usize,
    #[serde(default = "default_artifact_max_bytes")]
    pub artifact_max_bytes: usize,
    #[serde(default = "default_max_task_turns")]
    pub max_task_turns: usize,
}

impl Default for TracesToml {
    fn default() -> Self {
        Self {
            enabled: false,
            journal_max_bytes: default_journal_max_bytes(),
            journal_max_events: default_journal_max_events(),
            artifact_max_bytes: default_artifact_max_bytes(),
            max_task_turns: default_max_task_turns(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracesConfig {
    pub enabled: bool,
    pub journal_max_bytes: usize,
    pub journal_max_events: usize,
    pub artifact_max_bytes: usize,
    pub max_task_turns: usize,
}
```

```rust
// bin/a2a-bridge/src/config.rs, add to RegistryConfig
#[serde(default)]
pub traces: TracesToml,
```

```rust
// bin/a2a-bridge/src/config.rs, inside impl RegistryConfig near metrics_config()
pub fn traces_config(&self) -> Result<TracesConfig, ConfigError> {
    let limits = [
        ("journal_max_bytes", self.traces.journal_max_bytes),
        ("journal_max_events", self.traces.journal_max_events),
        ("artifact_max_bytes", self.traces.artifact_max_bytes),
        ("max_task_turns", self.traces.max_task_turns),
    ];
    for (name, value) in limits {
        if value == 0 {
            return Err(ConfigError::Registry(format!(
                "[traces].{name} must be > 0"
            )));
        }
    }
    Ok(TracesConfig {
        enabled: self.traces.enabled,
        journal_max_bytes: self.traces.journal_max_bytes,
        journal_max_events: self.traces.journal_max_events,
        artifact_max_bytes: self.traces.artifact_max_bytes,
        max_task_turns: self.traces.max_task_turns,
    })
}
```

- [ ] Step: add inbound trace HTTP config.
```rust
// crates/bridge-a2a-inbound/src/server.rs, above InboundServer
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceHttpConfig {
    pub enabled: bool,
    pub journal_max_bytes: usize,
    pub journal_max_events: usize,
    pub artifact_max_bytes: usize,
    pub max_task_turns: usize,
}

impl Default for TraceHttpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            journal_max_bytes: 16_777_216,
            journal_max_events: 100_000,
            artifact_max_bytes: 4_194_304,
            max_task_turns: 512,
        }
    }
}
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, add field to InboundServer
trace_config: TraceHttpConfig,
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, initialize in InboundServer::from_coordinator
trace_config: TraceHttpConfig::default(),
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, add builder near with_metrics_endpoint
#[must_use]
pub fn with_trace_http_config(mut self, config: TraceHttpConfig) -> Self {
    self.trace_config = config;
    self
}
```

- [ ] Step: wire serve-side config and turn-log predicate.
```rust
// bin/a2a-bridge/src/main.rs, near batch_runtime()
fn turn_log_observer_enabled(
    metrics_cfg: &config::MetricsConfig,
    traces_cfg: &config::TracesConfig,
) -> bool {
    traces_cfg.enabled || (metrics_cfg.enabled && metrics_cfg.turn_log)
}
```

```rust
// bin/a2a-bridge/src/main.rs, after metrics_cfg is built in serve startup
let traces_cfg = cfg.traces_config()?;
```

```rust
// bin/a2a-bridge/src/main.rs, replace the observer construction condition
let install_turn_log = turn_log_observer_enabled(&metrics_cfg, &traces_cfg);
let observer: Arc<dyn bridge_core::ports::Observer> =
    if !metrics_cfg.enabled && !install_turn_log {
        Arc::new(bridge_observ::NoopObserver)
    } else {
        let mut sinks: Vec<Arc<dyn bridge_core::ports::Observer>> = Vec::new();
        if let Some(prom) = &prometheus_observer {
            sinks.push(prom.clone());
        }
        if install_turn_log {
            let dropped = prometheus_observer
                .as_ref()
                .map(|p| p.drop_counter())
                .unwrap_or_else(bridge_observ::DropCounter::disabled);
            sinks.push(Arc::new(bridge_observ::TurnLogObserver::new(
                task_store.clone(),
                dropped,
                1024,
                Arc::new(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64
                }),
            )));
        }

        Arc::new(bridge_observ::DedupObserver::new_with_dedupe(
            Arc::new(bridge_observ::FanoutObserver::new(sinks)),
            dedupe,
        ))
    };
```

```rust
// bin/a2a-bridge/src/main.rs, add to server builder chain
.with_trace_http_config(bridge_a2a_inbound::server::TraceHttpConfig {
    enabled: traces_cfg.enabled,
    journal_max_bytes: traces_cfg.journal_max_bytes,
    journal_max_events: traces_cfg.journal_max_events,
    artifact_max_bytes: traces_cfg.artifact_max_bytes,
    max_task_turns: traces_cfg.max_task_turns,
})
```

- [ ] Step: run to green.
```bash
cargo test -p a2a-bridge traces_config_tests turn_log_observer_ -- --nocapture
cargo test -p bridge-a2a-inbound trace_http_config_defaults_disabled -- --nocapture
```
Expected PASS: defaults, zero-limit validation, metrics/traces independence, trace HTTP defaults.

- [ ] Step: commit.
```bash
git add bin/a2a-bridge/src/config.rs bin/a2a-bridge/src/main.rs crates/bridge-a2a-inbound/src/server.rs && git commit -m "feat: add trace drilldown config"
```

---

### Task 2: `TaskStore` Drill-Down Types and In-Memory Reads

**Files:** Modify `crates/bridge-core/src/task_store.rs:170`, `crates/bridge-core/src/task_store.rs:208`, `crates/bridge-core/src/task_store.rs:502`, `crates/bridge-core/src/task_store.rs:685`, `crates/bridge-core/src/task_store.rs:745`; test `crates/bridge-core/src/task_store.rs:1182`.
**Interfaces:** Consumes existing `TurnLogRow`, `TurnLogFinished`, `TurnLogUsage`, `TaskRecordStatus`, `MemoryTaskStore`. Produces `NodeArtifactMeta`, `NodeCheckpointOutput`, `TaskUsageAgg`, `JournalRead`, `TaskStore::turn_log_row`, `TaskStore::turn_log_rows_for_task`, `TaskStore::turn_log_usage_for_task`, `TaskStore::latest_turn_log_row_for_session`, `TaskStore::journal_jsonl_bounded`, `TaskStore::node_checkpoint_nodes`, `TaskStore::node_checkpoint_output`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-core/src/task_store.rs, inside #[cfg(test)] mod tests
use crate::ids::{ContextId, TurnId};
use crate::orch::{TerminalUsage, UsageCost};
use crate::ports::{TraceParent, TurnContext, TurnOutcome};
use std::time::Duration;

fn turn_ctx(turn: &str, session: &str, task: Option<&str>, attempt: u32) -> TurnContext {
    TurnContext {
        turn_id: TurnId::parse(turn).unwrap(),
        session_id: ContextId::parse(session).unwrap(),
        task_id: task.map(|t| TaskId::parse(t).unwrap()),
        workflow: Some("code-review".to_string()),
        node: Some("reviewer".to_string()),
        attempt,
        agent: "codex".to_string(),
        model: Some("gpt-5.5".to_string()),
        effort: Some("high".to_string()),
        mode: Some("default".to_string()),
        prompt_id: Some("prompt/eval".to_string()),
        traceparent: TraceParent::parse_header_value(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ),
    }
}

async fn write_finished_turn(
    store: &MemoryTaskStore,
    turn: &str,
    session: &str,
    task: Option<&str>,
    completed_ms: i64,
    input: u64,
    output: u64,
    cost: Option<(&str, f64)>,
) {
    let ctx = turn_ctx(turn, session, task, 0);
    store
        .upsert_turn_finished(&TurnLogFinished {
            ctx: ctx.clone(),
            started_ms: completed_ms - 10,
            completed_ms,
            latency: Duration::from_millis(10),
            ttft: None,
            outcome: TurnOutcome::Success,
        })
        .await
        .unwrap();
    store
        .update_turn_usage(&TurnLogUsage {
            ctx,
            usage: UsageSnapshot {
                used: None,
                size: None,
                cost: cost.map(|(currency, amount)| UsageCost {
                    amount,
                    currency: currency.to_string(),
                }),
                terminal: Some(TerminalUsage {
                    total_tokens: input + output + 999,
                    input_tokens: input,
                    output_tokens: output,
                    thought_tokens: Some(3),
                    cached_read_tokens: None,
                    cached_write_tokens: Some(5),
                }),
                at_ms: completed_ms,
            },
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn memory_turn_log_row_lookup() {
    let store = MemoryTaskStore::new();
    write_finished_turn(&store, "turn-a", "ctx-a", Some("task-a"), 20, 2, 4, Some(("USD", 0.25))).await;

    let found = store
        .turn_log_row(&TurnId::parse("turn-a").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.turn_id.as_str(), "turn-a");
    assert_eq!(found.task_id.as_ref().unwrap().as_str(), "task-a");
    assert_eq!(found.traceparent.unwrap().to_header_value(), "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01");

    assert!(store
        .turn_log_row(&TurnId::parse("missing").unwrap())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn memory_turn_log_rows_for_task_orders_and_limits() {
    let store = MemoryTaskStore::new();
    write_finished_turn(&store, "turn-c", "ctx-a", Some("task-a"), 30, 1, 1, None).await;
    write_finished_turn(&store, "turn-a", "ctx-a", Some("task-a"), 10, 1, 1, None).await;
    write_finished_turn(&store, "turn-b", "ctx-a", Some("task-a"), 20, 1, 1, None).await;
    write_finished_turn(&store, "turn-x", "ctx-a", Some("task-x"), 5, 1, 1, None).await;

    let rows = store
        .turn_log_rows_for_task(&TaskId::parse("task-a").unwrap(), 2)
        .await
        .unwrap();

    assert_eq!(
        rows.iter().map(|r| r.turn_id.as_str()).collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );
}

#[tokio::test]
async fn memory_turn_log_usage_for_task_is_unbounded_and_single_currency() {
    let store = MemoryTaskStore::new();
    write_finished_turn(&store, "turn-1", "ctx-a", Some("task-a"), 10, 2, 3, Some(("USD", 0.10))).await;
    write_finished_turn(&store, "turn-2", "ctx-a", Some("task-a"), 20, 5, 7, Some(("USD", 0.20))).await;
    write_finished_turn(&store, "turn-3", "ctx-a", Some("task-a"), 30, 11, 13, Some(("USD", 0.30))).await;

    let agg = store
        .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(agg.rows, 3);
    assert_eq!(agg.input_tokens, 18);
    assert_eq!(agg.output_tokens, 23);
    assert_eq!(agg.thought_tokens, Some(9));
    assert_eq!(agg.cached_read_tokens, None);
    assert_eq!(agg.cached_write_tokens, Some(15));
    assert_eq!(agg.cost.unwrap().currency, "USD");
    assert!((agg.cost.unwrap_or(UsageCost { amount: 0.0, currency: String::new() }).amount - 0.60).abs() < 0.000_001);
    assert_eq!(agg.at_ms, 30);
}

#[tokio::test]
async fn memory_turn_log_usage_for_task_omits_mixed_currency_cost() {
    let store = MemoryTaskStore::new();
    write_finished_turn(&store, "turn-1", "ctx-a", Some("task-a"), 10, 2, 3, Some(("USD", 0.10))).await;
    write_finished_turn(&store, "turn-2", "ctx-a", Some("task-a"), 20, 5, 7, Some(("EUR", 0.20))).await;

    let agg = store
        .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(agg.input_tokens, 7);
    assert_eq!(agg.output_tokens, 10);
    assert!(agg.cost.is_none());
}

#[tokio::test]
async fn memory_latest_turn_log_row_for_session_returns_latest() {
    let store = MemoryTaskStore::new();
    write_finished_turn(&store, "turn-old", "ctx-a", None, 10, 1, 1, None).await;
    write_finished_turn(&store, "turn-new", "ctx-a", None, 20, 1, 1, None).await;
    write_finished_turn(&store, "turn-other", "ctx-b", None, 30, 1, 1, None).await;

    let row = store
        .latest_turn_log_row_for_session(&ContextId::parse("ctx-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(row.turn_id.as_str(), "turn-new");
}

#[tokio::test]
async fn memory_journal_jsonl_bounded_body_counts_and_limits() {
    let store = MemoryTaskStore::new();
    let task = TaskId::parse("task-a").unwrap();
    let op = OperationId::parse("op-a").unwrap();
    store.create(&rec("task-a", 1)).await.unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            10,
            OrchEventKind::Progress {
                message: "one".into(),
            },
        )
        .await
        .unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            11,
            OrchEventKind::Progress {
                message: "two".into(),
            },
        )
        .await
        .unwrap();

    let body = store
        .journal_jsonl_bounded(&task, 10, 10_000)
        .await
        .unwrap();

    match body {
        JournalRead::Body { jsonl, events, bytes } => {
            assert_eq!(events, 2);
            assert_eq!(bytes as usize, jsonl.len());
            assert_eq!(jsonl.lines().count(), 2);
            assert!(jsonl.ends_with('\n'));
        }
        JournalRead::TooLarge { .. } => panic!("journal should fit"),
    }

    assert!(matches!(
        store.journal_jsonl_bounded(&task, 1, 10_000).await.unwrap(),
        JournalRead::TooLarge { events: 2, .. }
    ));
    assert!(matches!(
        store.journal_jsonl_bounded(&task, 10, 1).await.unwrap(),
        JournalRead::TooLarge { events: 2, .. }
    ));
}

#[tokio::test]
async fn memory_node_checkpoint_nodes_and_output_are_bounded() {
    let store = MemoryTaskStore::new();
    let task = TaskId::parse("task-a").unwrap();
    store.create(&rec("task-a", 1)).await.unwrap();
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("node-b").unwrap(),
            "large-output",
            true,
            10,
        )
        .await
        .unwrap();
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("node-a").unwrap(),
            "small",
            false,
            11,
        )
        .await
        .unwrap();

    let nodes = store.node_checkpoint_nodes(&task).await.unwrap();
    assert_eq!(
        nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["node-b", "node-a"]
    );

    let output = store
        .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output,
        NodeCheckpointOutput::Found {
            output: "small".into(),
            ok: false,
            usage: None,
            bytes: 5
        }
    );

    assert_eq!(
        store
            .node_checkpoint_output(&task, &NodeId::parse("node-b").unwrap(), 3)
            .await
            .unwrap(),
        Some(NodeCheckpointOutput::TooLarge { bytes: 12 })
    );
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-core memory_turn_log_row_lookup memory_turn_log_rows_for_task_orders_and_limits memory_turn_log_usage_for_task_is_unbounded_and_single_currency memory_turn_log_usage_for_task_omits_mixed_currency_cost memory_latest_turn_log_row_for_session_returns_latest memory_journal_jsonl_bounded_body_counts_and_limits memory_node_checkpoint_nodes_and_output_are_bounded -- --nocapture
```
Expected FAIL: unresolved `JournalRead`, `NodeCheckpointOutput`, and new `TaskStore` methods.

- [ ] Step: add drill-down DTOs and trait methods.
```rust
// crates/bridge-core/src/task_store.rs, after TurnLogRow
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeArtifactMeta {
    pub node: NodeId,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeCheckpointOutput {
    Found {
        output: String,
        ok: bool,
        usage: Option<crate::orch::UsageSnapshot>,
        bytes: u64,
    },
    TooLarge {
        bytes: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskUsageAgg {
    pub rows: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub cost: Option<crate::orch::UsageCost>,
    pub at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalRead {
    Body {
        jsonl: String,
        events: u64,
        bytes: u64,
    },
    TooLarge {
        events: u64,
        bytes: u64,
    },
}
```

```rust
// crates/bridge-core/src/task_store.rs, inside trait TaskStore after turn_log_rows()
async fn turn_log_row(&self, _turn_id: &TurnId) -> Result<Option<TurnLogRow>, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn turn_log_rows_for_task(
    &self,
    _task: &TaskId,
    _limit: usize,
) -> Result<Vec<TurnLogRow>, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn turn_log_usage_for_task(
    &self,
    _task: &TaskId,
) -> Result<Option<TaskUsageAgg>, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn latest_turn_log_row_for_session(
    &self,
    _session: &ContextId,
) -> Result<Option<TurnLogRow>, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn journal_jsonl_bounded(
    &self,
    _task: &TaskId,
    _max_events: usize,
    _max_bytes: usize,
) -> Result<JournalRead, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn node_checkpoint_nodes(&self, _task: &TaskId) -> Result<Vec<NodeId>, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn node_checkpoint_output(
    &self,
    _task: &TaskId,
    _node: &NodeId,
    _max_bytes: usize,
) -> Result<Option<NodeCheckpointOutput>, BridgeError> {
    Err(BridgeError::StoreFailure)
}
```

- [ ] Step: implement in-memory turn-log methods.
```rust
// crates/bridge-core/src/task_store.rs, inside impl TaskStore for MemoryTaskStore
async fn turn_log_row(&self, turn_id: &TurnId) -> Result<Option<TurnLogRow>, BridgeError> {
    Ok(self.turn_log.lock().unwrap().get(turn_id.as_str()).cloned())
}

async fn turn_log_rows_for_task(
    &self,
    task: &TaskId,
    limit: usize,
) -> Result<Vec<TurnLogRow>, BridgeError> {
    let mut rows: Vec<_> = self
        .turn_log
        .lock()
        .unwrap()
        .values()
        .filter(|row| row.task_id.as_ref().map(|t| t.as_str()) == Some(task.as_str()))
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        a.completed_ms
            .unwrap_or(i64::MAX)
            .cmp(&b.completed_ms.unwrap_or(i64::MAX))
            .then_with(|| a.turn_id.as_str().cmp(b.turn_id.as_str()))
    });
    rows.truncate(limit);
    Ok(rows)
}

async fn turn_log_usage_for_task(
    &self,
    task: &TaskId,
) -> Result<Option<TaskUsageAgg>, BridgeError> {
    let rows: Vec<_> = self
        .turn_log
        .lock()
        .unwrap()
        .values()
        .filter(|row| row.task_id.as_ref().map(|t| t.as_str()) == Some(task.as_str()))
        .cloned()
        .collect();

    if rows.is_empty() {
        return Ok(None);
    }

    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut thought_tokens = None::<u64>;
    let mut cached_read_tokens = None::<u64>;
    let mut cached_write_tokens = None::<u64>;
    let mut cost_amount = None::<f64>;
    let mut currencies = std::collections::HashSet::new();
    let mut at_ms = 0_i64;

    for row in &rows {
        input_tokens += row.input_tokens.unwrap_or(0);
        output_tokens += row.output_tokens.unwrap_or(0);
        if let Some(v) = row.thought_tokens {
            thought_tokens = Some(thought_tokens.unwrap_or(0) + v);
        }
        if let Some(v) = row.cached_read_tokens {
            cached_read_tokens = Some(cached_read_tokens.unwrap_or(0) + v);
        }
        if let Some(v) = row.cached_write_tokens {
            cached_write_tokens = Some(cached_write_tokens.unwrap_or(0) + v);
        }
        if let (Some(amount), Some(currency)) = (row.cost_amount, row.cost_currency.as_ref()) {
            cost_amount = Some(cost_amount.unwrap_or(0.0) + amount);
            currencies.insert(currency.clone());
        }
        if let Some(ms) = row.completed_ms {
            at_ms = at_ms.max(ms);
        }
    }

    let cost = if currencies.len() == 1 {
        cost_amount.map(|amount| crate::orch::UsageCost {
            amount,
            currency: currencies.into_iter().next().unwrap(),
        })
    } else {
        None
    };

    Ok(Some(TaskUsageAgg {
        rows: rows.len() as u64,
        input_tokens,
        output_tokens,
        thought_tokens,
        cached_read_tokens,
        cached_write_tokens,
        cost,
        at_ms,
    }))
}

async fn latest_turn_log_row_for_session(
    &self,
    session: &ContextId,
) -> Result<Option<TurnLogRow>, BridgeError> {
    let rows = self.turn_log.lock().unwrap();
    Ok(rows
        .values()
        .filter(|row| row.session_id.as_str() == session.as_str())
        .max_by(|a, b| {
            a.completed_ms
                .unwrap_or(i64::MIN)
                .cmp(&b.completed_ms.unwrap_or(i64::MIN))
                .then_with(|| a.turn_id.as_str().cmp(b.turn_id.as_str()))
        })
        .cloned())
}
```

- [ ] Step: implement in-memory journal and checkpoint reads.
```rust
// crates/bridge-core/src/task_store.rs, inside impl TaskStore for MemoryTaskStore
async fn journal_jsonl_bounded(
    &self,
    task: &TaskId,
    max_events: usize,
    max_bytes: usize,
) -> Result<JournalRead, BridgeError> {
    let events = self
        .journals
        .lock()
        .unwrap()
        .get(task.as_str())
        .cloned()
        .unwrap_or_default();

    let mut lines = Vec::with_capacity(events.len());
    let mut bytes = 0_u64;
    for (_seq, event) in &events {
        let line = serde_json::to_string(event).map_err(|_| BridgeError::StoreFailure)?;
        bytes += line.as_bytes().len() as u64 + 1;
        lines.push(line);
    }

    if events.len() > max_events || bytes as usize > max_bytes {
        return Ok(JournalRead::TooLarge {
            events: events.len() as u64,
            bytes,
        });
    }

    let jsonl = if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    Ok(JournalRead::Body {
        jsonl,
        events: events.len() as u64,
        bytes,
    })
}

async fn node_checkpoint_nodes(&self, task: &TaskId) -> Result<Vec<NodeId>, BridgeError> {
    let g = self.checkpoints.lock().unwrap();
    let mut rows = Vec::new();
    for ((tid, nid), (_output, _ok, _ts, seq, _usage)) in g.iter() {
        if tid == task.as_str() {
            rows.push((
                *seq,
                NodeId::parse(nid).map_err(|_| BridgeError::StoreFailure)?,
            ));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
    Ok(rows.into_iter().map(|(_seq, node)| node).collect())
}

async fn node_checkpoint_output(
    &self,
    task: &TaskId,
    node: &NodeId,
    max_bytes: usize,
) -> Result<Option<NodeCheckpointOutput>, BridgeError> {
    let g = self.checkpoints.lock().unwrap();
    let Some((output, ok, _ts, _seq, usage)) = g.get(&(
        task.as_str().to_string(),
        node.as_str().to_string(),
    )) else {
        return Ok(None);
    };
    let bytes = output.as_bytes().len() as u64;
    if bytes as usize > max_bytes {
        return Ok(Some(NodeCheckpointOutput::TooLarge { bytes }));
    }
    Ok(Some(NodeCheckpointOutput::Found {
        output: output.clone(),
        ok: *ok,
        usage: usage.clone(),
        bytes,
    }))
}
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-core memory_turn_log_row_lookup memory_turn_log_rows_for_task_orders_and_limits memory_turn_log_usage_for_task_is_unbounded_and_single_currency memory_turn_log_usage_for_task_omits_mixed_currency_cost memory_latest_turn_log_row_for_session_returns_latest memory_journal_jsonl_bounded_body_counts_and_limits memory_node_checkpoint_nodes_and_output_are_bounded -- --nocapture
```
Expected PASS: all in-memory bounded reads work and default trait stubs remain source-compatible.

- [ ] Step: commit.
```bash
git add crates/bridge-core/src/task_store.rs && git commit -m "feat: add trace task-store read ports"
```

---

### Task 3: SQLite Turn Row, Task Row List, Usage Rollup, and Latest Warm Turn

**Files:** Modify `crates/bridge-store/src/sqlite.rs:262`, `crates/bridge-store/src/sqlite.rs:803`; test `crates/bridge-store/src/sqlite.rs:1560`.
**Interfaces:** Consumes Task 2 `TaskStore` methods and `TaskUsageAgg`. Produces SQLite implementations of `turn_log_row`, `turn_log_rows_for_task`, `turn_log_usage_for_task`, `latest_turn_log_row_for_session`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-store/src/sqlite.rs, inside #[cfg(test)] mod tests
fn ctx_for(turn: &str, session: &str, task: &str, completed_attempt: u32) -> TurnContext {
    TurnContext {
        turn_id: TurnId::parse(turn).unwrap(),
        session_id: ContextId::parse(session).unwrap(),
        task_id: Some(TaskId::parse(task).unwrap()),
        workflow: Some("code-review".to_string()),
        node: Some("reviewer".to_string()),
        attempt: completed_attempt,
        agent: "codex".to_string(),
        model: Some("gpt-5.5".to_string()),
        effort: Some("high".to_string()),
        mode: Some("default".to_string()),
        prompt_id: Some("prompt/eval".to_string()),
        traceparent: TraceParent::parse_header_value(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ),
    }
}

async fn write_sqlite_turn(
    store: &SqliteStore,
    ctx: TurnContext,
    completed_ms: i64,
    input: u64,
    output: u64,
    cost: Option<(&str, f64)>,
) {
    store
        .upsert_turn_finished(&TurnLogFinished {
            ctx: ctx.clone(),
            started_ms: completed_ms - 10,
            completed_ms,
            latency: std::time::Duration::from_millis(10),
            ttft: Some(std::time::Duration::from_millis(2)),
            outcome: TurnOutcome::Success,
        })
        .await
        .unwrap();
    store
        .update_turn_usage(&TurnLogUsage {
            ctx,
            usage: UsageSnapshot {
                used: None,
                size: None,
                cost: cost.map(|(currency, amount)| UsageCost {
                    amount,
                    currency: currency.to_string(),
                }),
                terminal: Some(TerminalUsage {
                    total_tokens: 999,
                    input_tokens: input,
                    output_tokens: output,
                    thought_tokens: Some(1),
                    cached_read_tokens: Some(2),
                    cached_write_tokens: None,
                }),
                at_ms: completed_ms,
            },
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn sqlite_turn_log_row_lookup() {
    let store = SqliteStore::open_in_memory().unwrap();
    write_sqlite_turn(
        &store,
        ctx_for("turn-a", "ctx-a", "task-a", 0),
        20,
        2,
        4,
        Some(("USD", 0.25)),
    )
    .await;

    let row = store
        .turn_log_row(&TurnId::parse("turn-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(row.turn_id.as_str(), "turn-a");
    assert_eq!(row.session_id.as_str(), "ctx-a");
    assert_eq!(row.task_id.as_ref().unwrap().as_str(), "task-a");
    assert_eq!(row.input_tokens, Some(2));
    assert_eq!(row.output_tokens, Some(4));
    assert_eq!(row.cost_currency.as_deref(), Some("USD"));
    assert_eq!(row.traceparent.unwrap().to_header_value(), "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01");

    assert!(store
        .turn_log_row(&TurnId::parse("missing").unwrap())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn sqlite_turn_log_rows_for_task_orders_and_limits() {
    let store = SqliteStore::open_in_memory().unwrap();
    write_sqlite_turn(&store, ctx_for("turn-c", "ctx-a", "task-a", 0), 30, 1, 1, None).await;
    write_sqlite_turn(&store, ctx_for("turn-a", "ctx-a", "task-a", 0), 10, 1, 1, None).await;
    write_sqlite_turn(&store, ctx_for("turn-b", "ctx-a", "task-a", 0), 20, 1, 1, None).await;
    write_sqlite_turn(&store, ctx_for("turn-x", "ctx-a", "task-x", 0), 5, 1, 1, None).await;

    let rows = store
        .turn_log_rows_for_task(&TaskId::parse("task-a").unwrap(), 2)
        .await
        .unwrap();

    assert_eq!(
        rows.iter().map(|r| r.turn_id.as_str()).collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );
}

#[tokio::test]
async fn sqlite_turn_log_usage_for_task_sums_all_rows() {
    let store = SqliteStore::open_in_memory().unwrap();
    for i in 0..513 {
        write_sqlite_turn(
            &store,
            ctx_for(&format!("turn-{i:03}"), "ctx-a", "task-a", 0),
            i,
            2,
            3,
            Some(("USD", 0.01)),
        )
        .await;
    }

    let agg = store
        .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(agg.rows, 513);
    assert_eq!(agg.input_tokens, 1026);
    assert_eq!(agg.output_tokens, 1539);
    assert_eq!(agg.thought_tokens, Some(513));
    assert_eq!(agg.cached_read_tokens, Some(1026));
    assert_eq!(agg.cached_write_tokens, None);
    assert_eq!(agg.cost.as_ref().unwrap().currency, "USD");
    assert!((agg.cost.as_ref().unwrap().amount - 5.13).abs() < 0.000_001);
    assert_eq!(agg.at_ms, 512);
}

#[tokio::test]
async fn sqlite_turn_log_usage_for_task_cost_none_on_mixed_currency() {
    let store = SqliteStore::open_in_memory().unwrap();
    write_sqlite_turn(&store, ctx_for("turn-usd", "ctx-a", "task-a", 0), 10, 2, 3, Some(("USD", 0.10))).await;
    write_sqlite_turn(&store, ctx_for("turn-eur", "ctx-a", "task-a", 0), 20, 5, 7, Some(("EUR", 0.20))).await;

    let agg = store
        .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(agg.input_tokens, 7);
    assert_eq!(agg.output_tokens, 10);
    assert!(agg.cost.is_none());
}

#[tokio::test]
async fn sqlite_latest_turn_log_row_for_session_returns_latest() {
    let store = SqliteStore::open_in_memory().unwrap();
    write_sqlite_turn(&store, ctx_for("turn-old", "ctx-a", "task-a", 0), 10, 1, 1, None).await;
    write_sqlite_turn(&store, ctx_for("turn-new", "ctx-a", "task-a", 0), 20, 1, 1, None).await;
    write_sqlite_turn(&store, ctx_for("turn-other", "ctx-b", "task-a", 0), 30, 1, 1, None).await;

    let row = store
        .latest_turn_log_row_for_session(&ContextId::parse("ctx-a").unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(row.turn_id.as_str(), "turn-new");
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-store sqlite_turn_log_row_lookup sqlite_turn_log_rows_for_task_orders_and_limits sqlite_turn_log_usage_for_task_sums_all_rows sqlite_turn_log_usage_for_task_cost_none_on_mixed_currency sqlite_latest_turn_log_row_for_session_returns_latest -- --nocapture
```
Expected FAIL: SQLite uses default `TaskStore` stubs for the new methods.

- [ ] Step: factor a row mapper and update `turn_log_rows`.
```rust
// crates/bridge-store/src/sqlite.rs, near traceparent_from_string()
fn row_to_turn_log_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<bridge_core::task_store::TurnLogRow> {
    Ok(bridge_core::task_store::TurnLogRow {
        turn_id: bridge_core::ids::TurnId::parse(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        session_id: bridge_core::ids::ContextId::parse(row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        task_id: row
            .get::<_, Option<String>>(2)?
            .map(bridge_core::ids::TaskId::parse)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        workflow: row.get(3)?,
        node: row.get(4)?,
        attempt: row.get::<_, i64>(5)? as u32,
        agent: row.get(6)?,
        model: row.get(7)?,
        effort: row.get(8)?,
        mode: row.get(9)?,
        prompt_id: row.get(10)?,
        started_ms: row.get(11)?,
        completed_ms: row.get(12)?,
        latency_ms: row.get::<_, Option<i64>>(13)?.map(|v| v as u64),
        ttft_ms: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
        outcome: row.get(15)?,
        failure_class: row.get(16)?,
        input_tokens: row.get::<_, Option<i64>>(17)?.map(|v| v as u64),
        output_tokens: row.get::<_, Option<i64>>(18)?.map(|v| v as u64),
        thought_tokens: row.get::<_, Option<i64>>(19)?.map(|v| v as u64),
        cached_read_tokens: row.get::<_, Option<i64>>(20)?.map(|v| v as u64),
        cached_write_tokens: row.get::<_, Option<i64>>(21)?.map(|v| v as u64),
        cost_amount: row.get(22)?,
        cost_currency: row.get(23)?,
        traceparent: traceparent_from_string(row.get(24)?),
    })
}

const TURN_LOG_SELECT: &str = "SELECT turn_id, session_id, task_id, workflow, node, attempt, agent, model, effort, mode,
        prompt_id, started_ms, completed_ms, latency_ms, ttft_ms, outcome, failure_class,
        input_tokens, output_tokens, thought_tokens, cached_read_tokens, cached_write_tokens,
        cost_amount, cost_currency, traceparent
 FROM turn_log";
```

```rust
// crates/bridge-store/src/sqlite.rs, replace turn_log_rows body with mapper use
async fn turn_log_rows(&self) -> Result<Vec<bridge_core::task_store::TurnLogRow>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let sql = format!("{TURN_LOG_SELECT} ORDER BY turn_id");
    let mut stmt = conn.prepare(&sql).map_err(|_| BridgeError::StoreFailure)?;
    let rows = stmt
        .query_map([], row_to_turn_log_row)
        .map_err(|_| BridgeError::StoreFailure)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| BridgeError::StoreFailure)?;
    Ok(rows)
}
```

- [ ] Step: implement SQLite turn row and task row list.
```rust
// crates/bridge-store/src/sqlite.rs, inside impl TaskStore for SqliteStore after turn_log_rows()
async fn turn_log_row(
    &self,
    turn_id: &bridge_core::ids::TurnId,
) -> Result<Option<bridge_core::task_store::TurnLogRow>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let sql = format!("{TURN_LOG_SELECT} WHERE turn_id=?1");
    conn.query_row(&sql, rusqlite::params![turn_id.as_str()], row_to_turn_log_row)
        .optional()
        .map_err(|_| BridgeError::StoreFailure)
}

async fn turn_log_rows_for_task(
    &self,
    task: &TaskId,
    limit: usize,
) -> Result<Vec<bridge_core::task_store::TurnLogRow>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let sql = format!("{TURN_LOG_SELECT} WHERE task_id=?1 ORDER BY completed_ms, turn_id LIMIT ?2");
    let mut stmt = conn.prepare(&sql).map_err(|_| BridgeError::StoreFailure)?;
    let rows = stmt
        .query_map(rusqlite::params![task.as_str(), limit as i64], row_to_turn_log_row)
        .map_err(|_| BridgeError::StoreFailure)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| BridgeError::StoreFailure)?;
    Ok(rows)
}
```

- [ ] Step: implement unbounded SQLite usage rollup.
```rust
// crates/bridge-store/src/sqlite.rs, inside impl TaskStore for SqliteStore
async fn turn_log_usage_for_task(
    &self,
    task: &TaskId,
) -> Result<Option<bridge_core::task_store::TaskUsageAgg>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let (
        rows,
        input_tokens,
        output_tokens,
        thought_tokens,
        cached_read_tokens,
        cached_write_tokens,
        sum_cost_amount,
        distinct_currency_count,
        min_cost_currency,
        at_ms,
    ): (
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        i64,
        Option<String>,
        Option<i64>,
    ) = conn
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    SUM(thought_tokens),
                    SUM(cached_read_tokens),
                    SUM(cached_write_tokens),
                    SUM(cost_amount),
                    COUNT(DISTINCT cost_currency),
                    MIN(cost_currency),
                    MAX(completed_ms)
             FROM turn_log WHERE task_id=?1",
            rusqlite::params![task.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .map_err(|_| BridgeError::StoreFailure)?;

    if rows == 0 {
        return Ok(None);
    }

    let cost = if distinct_currency_count == 1 {
        match (sum_cost_amount, min_cost_currency) {
            (Some(amount), Some(currency)) => Some(bridge_core::orch::UsageCost { amount, currency }),
            _ => None,
        }
    } else {
        None
    };

    Ok(Some(bridge_core::task_store::TaskUsageAgg {
        rows: rows as u64,
        input_tokens: input_tokens as u64,
        output_tokens: output_tokens as u64,
        thought_tokens: thought_tokens.map(|v| v as u64),
        cached_read_tokens: cached_read_tokens.map(|v| v as u64),
        cached_write_tokens: cached_write_tokens.map(|v| v as u64),
        cost,
        at_ms: at_ms.unwrap_or(0),
    }))
}
```

- [ ] Step: implement latest warm-session lookup.
```rust
// crates/bridge-store/src/sqlite.rs, inside impl TaskStore for SqliteStore
async fn latest_turn_log_row_for_session(
    &self,
    session: &bridge_core::ids::ContextId,
) -> Result<Option<bridge_core::task_store::TurnLogRow>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let sql = format!(
        "{TURN_LOG_SELECT} WHERE session_id=?1 ORDER BY completed_ms DESC, turn_id DESC LIMIT 1"
    );
    conn.query_row(
        &sql,
        rusqlite::params![session.as_str()],
        row_to_turn_log_row,
    )
    .optional()
    .map_err(|_| BridgeError::StoreFailure)
}
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-store sqlite_turn_log_row_lookup sqlite_turn_log_rows_for_task_orders_and_limits sqlite_turn_log_usage_for_task_sums_all_rows sqlite_turn_log_usage_for_task_cost_none_on_mixed_currency sqlite_latest_turn_log_row_for_session_returns_latest -- --nocapture
```
Expected PASS: SQLite reads rows by key, limits task ref rows, rolls up all cost/token rows without LIMIT, drops mixed-currency cost, and returns the latest warm turn.

- [ ] Step: commit.
```bash
git add crates/bridge-store/src/sqlite.rs && git commit -m "feat: add sqlite turn drilldown reads"
```

---

### Task 4: SQLite Bounded Journal and Artifact Reads

**Files:** Modify `crates/bridge-store/src/sqlite.rs:1312`, `crates/bridge-store/src/sqlite.rs:1409`; test `crates/bridge-store/src/sqlite.rs:1560`.
**Interfaces:** Consumes Task 2 `JournalRead`, `NodeCheckpointOutput`. Produces SQLite implementations of `journal_jsonl_bounded`, `node_checkpoint_nodes`, `node_checkpoint_output`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-store/src/sqlite.rs, inside #[cfg(test)] mod tests
#[tokio::test]
async fn sqlite_journal_jsonl_bounded_body_and_counts() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = TaskId::parse("task-journal").unwrap();
    let op = OperationId::parse("op-journal").unwrap();
    store.create(&trec(task.as_str(), 1)).await.unwrap();

    store
        .record_event_sequenced(
            &task,
            &op,
            10,
            bridge_core::orch::OrchEventKind::Progress {
                message: "one".into(),
            },
        )
        .await
        .unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            11,
            bridge_core::orch::OrchEventKind::Progress {
                message: "two".into(),
            },
        )
        .await
        .unwrap();

    let read = store
        .journal_jsonl_bounded(&task, 10, 10_000)
        .await
        .unwrap();

    match read {
        bridge_core::task_store::JournalRead::Body { jsonl, events, bytes } => {
            assert_eq!(events, 2);
            assert_eq!(bytes as usize, jsonl.len());
            assert!(jsonl.ends_with('\n'));
            let parsed = jsonl
                .lines()
                .map(|line| serde_json::from_str::<bridge_core::orch::OrchEvent>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(parsed.len(), 2);
            assert_eq!(parsed[0].seq, 1);
            assert_eq!(parsed[1].seq, 2);
        }
        other => panic!("expected body, got {other:?}"),
    }
}

#[tokio::test]
async fn sqlite_journal_jsonl_bounded_too_large_over_events() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = TaskId::parse("task-journal").unwrap();
    let op = OperationId::parse("op-journal").unwrap();
    store.create(&trec(task.as_str(), 1)).await.unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            10,
            bridge_core::orch::OrchEventKind::Progress {
                message: "one".into(),
            },
        )
        .await
        .unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            11,
            bridge_core::orch::OrchEventKind::Progress {
                message: "two".into(),
            },
        )
        .await
        .unwrap();

    assert!(matches!(
        store.journal_jsonl_bounded(&task, 1, 10_000).await.unwrap(),
        bridge_core::task_store::JournalRead::TooLarge { events: 2, .. }
    ));
}

#[tokio::test]
async fn sqlite_journal_jsonl_bounded_too_large_over_bytes() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = TaskId::parse("task-journal").unwrap();
    let op = OperationId::parse("op-journal").unwrap();
    store.create(&trec(task.as_str(), 1)).await.unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            10,
            bridge_core::orch::OrchEventKind::Progress {
                message: "one".into(),
            },
        )
        .await
        .unwrap();

    assert!(matches!(
        store.journal_jsonl_bounded(&task, 10, 1).await.unwrap(),
        bridge_core::task_store::JournalRead::TooLarge { events: 1, .. }
    ));
}

#[tokio::test]
async fn sqlite_node_checkpoint_nodes_metadata_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = TaskId::parse("task-artifact").unwrap();
    let op = OperationId::parse("op-artifact").unwrap();
    store.create(&trec(task.as_str(), 1)).await.unwrap();

    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("legacy").unwrap(),
            "legacy output",
            true,
            10,
        )
        .await
        .unwrap();
    store
        .put_node_checkpoint_sequenced(
            &task,
            &NodeId::parse("later").unwrap(),
            &op,
            "later output",
            true,
            11,
            None,
        )
        .await
        .unwrap();

    let nodes = store.node_checkpoint_nodes(&task).await.unwrap();

    assert_eq!(
        nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["legacy", "later"]
    );
}

#[tokio::test]
async fn sqlite_node_checkpoint_output_too_large_single_statement() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = TaskId::parse("task-artifact").unwrap();
    store.create(&trec(task.as_str(), 1)).await.unwrap();
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("node-a").unwrap(),
            "abcdef",
            true,
            10,
        )
        .await
        .unwrap();

    let found = store
        .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), 6)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        found,
        bridge_core::task_store::NodeCheckpointOutput::Found {
            output: "abcdef".into(),
            ok: true,
            usage: None,
            bytes: 6
        }
    );

    let too_large = store
        .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), 5)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        too_large,
        bridge_core::task_store::NodeCheckpointOutput::TooLarge { bytes: 6 }
    );

    assert!(store
        .node_checkpoint_output(&task, &NodeId::parse("missing").unwrap(), 5)
        .await
        .unwrap()
        .is_none());
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-store sqlite_journal_jsonl_bounded_body_and_counts sqlite_journal_jsonl_bounded_too_large_over_events sqlite_journal_jsonl_bounded_too_large_over_bytes sqlite_node_checkpoint_nodes_metadata_only sqlite_node_checkpoint_output_too_large_single_statement -- --nocapture
```
Expected FAIL: SQLite uses default `TaskStore` stubs for bounded journal and checkpoint methods.

- [ ] Step: implement bounded journal read under one connection guard.
```rust
// crates/bridge-store/src/sqlite.rs, inside impl TaskStore for SqliteStore
async fn journal_jsonl_bounded(
    &self,
    task: &TaskId,
    max_events: usize,
    max_bytes: usize,
) -> Result<bridge_core::task_store::JournalRead, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let tx = conn
        .unchecked_transaction()
        .map_err(|_| BridgeError::StoreFailure)?;

    let (events, bytes): (i64, i64) = tx
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(event_json AS BLOB))+1),0)
             FROM task_journal WHERE task_id=?1",
            rusqlite::params![task.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| BridgeError::StoreFailure)?;

    if events as usize > max_events || bytes as usize > max_bytes {
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        return Ok(bridge_core::task_store::JournalRead::TooLarge {
            events: events as u64,
            bytes: bytes as u64,
        });
    }

    let jsonl = {
        let mut stmt = tx
            .prepare(
                "SELECT seq, event_json FROM task_journal
                 WHERE task_id=?1 ORDER BY seq",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = String::with_capacity(bytes as usize);
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            let event_json: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
            out.push_str(&event_json);
            out.push('\n');
        }
        out
    };

    tx.commit().map_err(|_| BridgeError::StoreFailure)?;
    Ok(bridge_core::task_store::JournalRead::Body {
        jsonl,
        events: events as u64,
        bytes: bytes as u64,
    })
}
```

- [ ] Step: implement checkpoint metadata and atomic size-checked output read.
```rust
// crates/bridge-store/src/sqlite.rs, inside impl TaskStore for SqliteStore
async fn node_checkpoint_nodes(&self, task: &TaskId) -> Result<Vec<NodeId>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT node_id FROM task_node_checkpoints
             WHERE task_id=?1 ORDER BY COALESCE(seq,0), node_id",
        )
        .map_err(|_| BridgeError::StoreFailure)?;

    let nodes = stmt
        .query_map(rusqlite::params![task.as_str()], |row| {
            let raw: String = row.get(0)?;
            NodeId::parse(raw).map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .map_err(|_| BridgeError::StoreFailure)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| BridgeError::StoreFailure)?;

    Ok(nodes)
}

async fn node_checkpoint_output(
    &self,
    task: &TaskId,
    node: &NodeId,
    max_bytes: usize,
) -> Result<Option<bridge_core::task_store::NodeCheckpointOutput>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let row = conn
        .query_row(
            "SELECT
                (CASE WHEN length(CAST(output AS BLOB)) <= ?3 THEN output END),
                ok,
                usage_json,
                length(CAST(output AS BLOB))
             FROM task_node_checkpoints
             WHERE task_id=?1 AND node_id=?2",
            rusqlite::params![task.as_str(), node.as_str(), max_bytes as i64],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| BridgeError::StoreFailure)?;

    let Some((output, ok, usage_json, bytes)) = row else {
        return Ok(None);
    };

    if output.is_none() {
        return Ok(Some(bridge_core::task_store::NodeCheckpointOutput::TooLarge {
            bytes: bytes as u64,
        }));
    }

    let usage = usage_json
        .as_deref()
        .map(serde_json::from_str::<bridge_core::orch::UsageSnapshot>)
        .transpose()
        .map_err(|_| BridgeError::StoreFailure)?;

    Ok(Some(bridge_core::task_store::NodeCheckpointOutput::Found {
        output: output.unwrap(),
        ok: ok != 0,
        usage,
        bytes: bytes as u64,
    }))
}
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-store sqlite_journal_jsonl_bounded_body_and_counts sqlite_journal_jsonl_bounded_too_large_over_events sqlite_journal_jsonl_bounded_too_large_over_bytes sqlite_node_checkpoint_nodes_metadata_only sqlite_node_checkpoint_output_too_large_single_statement -- --nocapture
```
Expected PASS: journal caps return `TooLarge` before body assembly; checkpoint output returns `TooLarge` without loading oversized text.

- [ ] Step: commit.
```bash
git add crates/bridge-store/src/sqlite.rs && git commit -m "feat: add sqlite bounded trace reads"
```

---

### Task 5: `workflow_spec_node_ids` Helper

**Files:** Modify `crates/bridge-core/src/ids.rs:55`; modify/test `crates/bridge-coordinator/src/detached.rs:1393`, `crates/bridge-coordinator/src/detached.rs:2048`.
**Interfaces:** Consumes existing `WorkflowSpecEnvelope`, `encode_workflow_spec(&WorkflowGraph) -> String`. Produces `workflow_spec_node_ids(spec_json: &str) -> Result<std::collections::BTreeSet<NodeId>, BridgeError>`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-coordinator/src/detached.rs, inside existing tests module
#[test]
fn workflow_spec_node_ids_reads_persisted_snapshot() {
    let graph = bridge_workflow::graph::WorkflowGraph {
        id: bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
        nodes: vec![
            bridge_workflow::graph::WorkflowNode {
                id: bridge_core::ids::NodeId::parse("reviewer").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: Vec::new(),
                retry: None,
            },
            bridge_workflow::graph::WorkflowNode {
                id: bridge_core::ids::NodeId::parse("synth").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{reviewer}}".into(),
                inputs: vec![bridge_core::ids::NodeId::parse("reviewer").unwrap()],
                retry: None,
            },
        ],
        panel: None,
    };
    let json = encode_workflow_spec(&graph);

    let nodes = workflow_spec_node_ids(&json).unwrap();

    assert_eq!(
        nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["reviewer", "synth"]
    );
}

#[test]
fn workflow_spec_node_ids_rejects_bad_snapshot() {
    assert!(workflow_spec_node_ids(r#"{"v":1,"graph":{"id":"w","nodes":[{"id":"BAD","agent":"codex","prompt_template":"","inputs":[]}]}}"#).is_err());
    assert!(workflow_spec_node_ids(r#"{"v":999,"graph":{"id":"w","nodes":[]}}"#).is_err());
    assert!(workflow_spec_node_ids("not json").is_err());
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-coordinator workflow_spec_node_ids_reads_persisted_snapshot workflow_spec_node_ids_rejects_bad_snapshot -- --nocapture
```
Expected FAIL: unresolved `workflow_spec_node_ids` and `NodeId` not orderable for `BTreeSet<NodeId>`.

- [ ] Step: make strict ids orderable.
```rust
// crates/bridge-core/src/ids.rs, update id_newtype_strict derive
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct $name(String);
```

- [ ] Step: add the helper next to `WorkflowSpecEnvelope`.
```rust
// crates/bridge-coordinator/src/detached.rs, after encode_workflow_spec()
pub fn workflow_spec_node_ids(
    spec_json: &str,
) -> Result<std::collections::BTreeSet<bridge_core::ids::NodeId>, bridge_core::error::BridgeError> {
    let env: WorkflowSpecEnvelope =
        serde_json::from_str(spec_json).map_err(|_| bridge_core::error::BridgeError::StoreFailure)?;
    if env.v != SUPPORTED_SNAPSHOT_VERSION {
        return Err(bridge_core::error::BridgeError::StoreFailure);
    }
    Ok(env.graph.nodes.into_iter().map(|node| node.id).collect())
}
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-coordinator workflow_spec_node_ids_reads_persisted_snapshot workflow_spec_node_ids_rejects_bad_snapshot -- --nocapture
```
Expected PASS: persisted workflow snapshots expose their node id set and reject unreadable snapshots.

- [ ] Step: commit.
```bash
git add crates/bridge-core/src/ids.rs crates/bridge-coordinator/src/detached.rs && git commit -m "feat: expose workflow snapshot node ids"
```

---

### Task 6: DTO Usage, Trace Refs, Percent-Encoding, and Async Status Builders

**Files:** Modify/test `crates/bridge-coordinator/src/coordinator.rs:37`, `crates/bridge-coordinator/src/coordinator.rs:44`, `crates/bridge-coordinator/src/coordinator.rs:55`, `crates/bridge-coordinator/src/coordinator.rs:80`, `crates/bridge-coordinator/src/coordinator.rs:94`, `crates/bridge-coordinator/src/coordinator.rs:107`, `crates/bridge-coordinator/src/coordinator.rs:142`, `crates/bridge-coordinator/src/coordinator.rs:653`, `crates/bridge-coordinator/src/coordinator.rs:1120`; modify `bin/a2a-bridge/src/main.rs:687`, `bin/a2a-bridge/src/main.rs:6210`.
**Interfaces:** Consumes Task 2 `turn_log_usage_for_task`, `turn_log_rows_for_task`, `latest_turn_log_row_for_session`, `node_checkpoint_nodes`. Produces `TraceRefs`, `TaskStatusDto.usage`, `TaskStatusDto.trace`, `SessionStatusDto.trace`, `Coordinator::with_trace_refs_config(mut self, enabled: bool, max_task_turns: usize) -> Self`, `Coordinator::status(...)` with async DTO population, `percent_encode_segment(raw: &str) -> String`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-coordinator/src/coordinator.rs, inside tests module
use bridge_core::orch::{TerminalUsage, UsageCost};
use bridge_core::ports::{TurnContext, TurnOutcome};
use bridge_core::task_store::{TurnLogFinished, TurnLogUsage};
use std::time::Duration;

#[test]
fn trace_refs_skip_absent_fields() {
    let value = serde_json::to_value(TraceRefs::default()).unwrap();
    assert_eq!(value, serde_json::json!({}));
}

#[tokio::test]
async fn task_status_dto_omits_usage_trace_when_none() {
    let fixture = coordinator_fixture(Arc::new(HashMap::new()));
    let id = task("task-no-rows");
    fixture.task_store.create(&working_record(id.clone())).await.unwrap();

    let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

    match dto {
        StatusDto::Task(task) => {
            let value = serde_json::to_value(task).unwrap();
            assert!(value.get("usage").is_none());
            assert!(value.get("trace").is_none());
        }
        StatusDto::Session(_) => panic!("expected task status"),
    }
}

fn dto_turn_ctx(turn: &str, task: &str, completed_ms: i64) -> (TurnContext, TurnLogFinished, TurnLogUsage) {
    let ctx = TurnContext {
        turn_id: bridge_core::ids::TurnId::parse(turn).unwrap(),
        session_id: ContextId::parse("ctx-dto").unwrap(),
        task_id: Some(TaskId::parse(task).unwrap()),
        workflow: Some("code-review".into()),
        node: Some("reviewer".into()),
        attempt: 0,
        agent: "codex".into(),
        model: Some("gpt-5.5".into()),
        effort: Some("high".into()),
        mode: None,
        prompt_id: Some("prompt/eval".into()),
        traceparent: None,
    };
    let finished = TurnLogFinished {
        ctx: ctx.clone(),
        started_ms: completed_ms - 10,
        completed_ms,
        latency: Duration::from_millis(10),
        ttft: None,
        outcome: TurnOutcome::Success,
    };
    let usage = TurnLogUsage {
        ctx: ctx.clone(),
        usage: UsageSnapshot {
            used: Some(999),
            size: Some(1000),
            cost: Some(UsageCost {
                amount: 0.50,
                currency: "USD".into(),
            }),
            terminal: Some(TerminalUsage {
                total_tokens: 9999,
                input_tokens: 7,
                output_tokens: 11,
                thought_tokens: Some(3),
                cached_read_tokens: Some(5),
                cached_write_tokens: None,
            }),
            at_ms: completed_ms,
        },
    };
    (ctx, finished, usage)
}

#[tokio::test]
async fn task_usage_aggregates_from_turn_log_single_currency() {
    let fixture = coordinator_fixture(Arc::new(HashMap::new()));
    let id = task("task-usage");
    fixture.task_store.create(&working_record(id.clone())).await.unwrap();

    for (turn, completed_ms) in [("turn-a", 10), ("turn-b", 20)] {
        let (_ctx, finished, usage) = dto_turn_ctx(turn, id.as_str(), completed_ms);
        fixture.task_store.upsert_turn_finished(&finished).await.unwrap();
        fixture.task_store.update_turn_usage(&usage).await.unwrap();
    }

    let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

    match dto {
        StatusDto::Task(task) => {
            let usage = task.usage.unwrap();
            assert_eq!(usage.used, None);
            assert_eq!(usage.size, None);
            assert_eq!(usage.cost.as_ref().unwrap().currency, "USD");
            assert!((usage.cost.as_ref().unwrap().amount - 1.0).abs() < 0.000_001);
            let terminal = usage.terminal.unwrap();
            assert_eq!(terminal.input_tokens, 14);
            assert_eq!(terminal.output_tokens, 22);
            assert_eq!(terminal.thought_tokens, Some(6));
            assert_eq!(terminal.cached_read_tokens, Some(10));
            assert_eq!(terminal.cached_write_tokens, None);
            assert_eq!(usage.at_ms, 20);
        }
        StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn task_usage_omits_cost_for_mixed_currencies() {
    let fixture = coordinator_fixture(Arc::new(HashMap::new()));
    let id = task("task-mixed");
    fixture.task_store.create(&working_record(id.clone())).await.unwrap();

    let (_ctx, finished, mut usage) = dto_turn_ctx("turn-usd", id.as_str(), 10);
    fixture.task_store.upsert_turn_finished(&finished).await.unwrap();
    fixture.task_store.update_turn_usage(&usage).await.unwrap();

    let (_ctx, finished, mut usage2) = dto_turn_ctx("turn-eur", id.as_str(), 20);
    usage2.usage.cost = Some(UsageCost {
        amount: 0.25,
        currency: "EUR".into(),
    });
    fixture.task_store.upsert_turn_finished(&finished).await.unwrap();
    fixture.task_store.update_turn_usage(&usage2).await.unwrap();

    let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

    match dto {
        StatusDto::Task(task) => {
            let usage = task.usage.unwrap();
            assert!(usage.cost.is_none());
            assert_eq!(usage.terminal.unwrap().input_tokens, 14);
        }
        StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn task_usage_terminal_total_tokens_is_input_plus_output() {
    let fixture = coordinator_fixture(Arc::new(HashMap::new()));
    let id = task("task-total");
    fixture.task_store.create(&working_record(id.clone())).await.unwrap();

    let (_ctx, finished, usage) = dto_turn_ctx("turn-total", id.as_str(), 10);
    fixture.task_store.upsert_turn_finished(&finished).await.unwrap();
    fixture.task_store.update_turn_usage(&usage).await.unwrap();

    let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

    match dto {
        StatusDto::Task(task) => {
            let terminal = task.usage.unwrap().terminal.unwrap();
            assert_eq!(terminal.input_tokens, 7);
            assert_eq!(terminal.output_tokens, 11);
            assert_eq!(terminal.total_tokens, 18);
        }
        StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn trace_ref_segments_are_percent_encoded() {
    let fixture = coordinator_fixture(Arc::new(HashMap::new()));
    let coordinator = fixture
        .coordinator
        .with_trace_refs_config(true, 4);
    let id = TaskId::parse("task/with?chars").unwrap();
    fixture.task_store.create(&working_record(id.clone())).await.unwrap();

    let (_ctx, finished, usage) = dto_turn_ctx("turn/with#chars", id.as_str(), 10);
    fixture.task_store.upsert_turn_finished(&finished).await.unwrap();
    fixture.task_store.update_turn_usage(&usage).await.unwrap();

    let dto = coordinator.status(None, Some(id)).await.unwrap();

    match dto {
        StatusDto::Task(task) => {
            let trace = task.trace.unwrap();
            assert_eq!(trace.journal.unwrap(), "/tasks/task%2Fwith%3Fchars/journal.jsonl");
            assert_eq!(trace.turns.unwrap(), vec!["/turns/turn%2Fwith%23chars"]);
        }
        StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn task_trace_turn_refs_are_capped_but_usage_is_not() {
    let fixture = coordinator_fixture(Arc::new(HashMap::new()));
    let coordinator = fixture
        .coordinator
        .with_trace_refs_config(true, 2);
    let id = task("task-capped");
    fixture.task_store.create(&working_record(id.clone())).await.unwrap();

    for i in 0..3 {
        let (_ctx, finished, usage) = dto_turn_ctx(&format!("turn-{i}"), id.as_str(), 10 + i);
        fixture.task_store.upsert_turn_finished(&finished).await.unwrap();
        fixture.task_store.update_turn_usage(&usage).await.unwrap();
    }

    let dto = coordinator.status(None, Some(id)).await.unwrap();

    match dto {
        StatusDto::Task(task) => {
            assert_eq!(task.trace.unwrap().turns.unwrap().len(), 2);
            assert_eq!(task.usage.unwrap().terminal.unwrap().input_tokens, 21);
        }
        StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn session_status_includes_latest_warm_turn_trace_ref() {
    let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
        entry: entry(),
        backend: Arc::new(FakeBackend::new(None)),
        resolved: Arc::new(StdMutex::new(Vec::new())),
    });
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
    let session_manager = Arc::new(SessionManager::new_with_clock(
        registry.clone(),
        Duration::from_secs(60),
        clock.clone(),
    ));
    let task_store = Arc::new(MemoryTaskStore::new());
    let task_store_dyn: Arc<dyn TaskStore> = task_store.clone();
    let session_store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::default());
    let coordinator = Coordinator::new(
        session_manager.clone(),
        None,
        Arc::new(HashMap::new()),
        task_store_dyn,
        session_store,
        Arc::new(AllowPolicy),
        registry,
        clock,
        Some(SessionCwd::parse("/tmp").unwrap()),
        None,
        Arc::new(NoopObserver),
        3,
    )
    .with_trace_refs_config(true, 4);

    let ctx = ContextId::parse("ctx-warm").unwrap();
    let turn = bridge_core::ids::TurnId::parse("turn-warm-latest").unwrap();
    let turn_ctx = TurnContext {
        turn_id: turn.clone(),
        session_id: ctx.clone(),
        task_id: None,
        workflow: None,
        node: None,
        attempt: 0,
        agent: "codex".into(),
        model: None,
        effort: None,
        mode: None,
        prompt_id: None,
        traceparent: None,
    };
    task_store
        .upsert_turn_finished(&TurnLogFinished {
            ctx: turn_ctx,
            started_ms: 10,
            completed_ms: 20,
            latency: Duration::from_millis(10),
            ttft: None,
            outcome: TurnOutcome::Success,
        })
        .await
        .unwrap();

    session_manager
        .ensure_idle_for_test(ctx.clone(), AgentId::parse("codex").unwrap())
        .await;

    let dto = coordinator.status(Some(ctx), None).await.unwrap();

    match dto {
        StatusDto::Session(session) => {
            assert_eq!(session.trace.unwrap().turn.unwrap(), "/turns/turn-warm-latest");
        }
        StatusDto::Task(_) => panic!("expected session status"),
    }
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-coordinator trace_refs_skip_absent_fields task_status_dto_omits_usage_trace_when_none task_usage_aggregates_from_turn_log_single_currency task_usage_omits_cost_for_mixed_currencies task_usage_terminal_total_tokens_is_input_plus_output trace_ref_segments_are_percent_encoded task_trace_turn_refs_are_capped_but_usage_is_not session_status_includes_latest_warm_turn_trace_ref -- --nocapture
```
Expected FAIL: missing `TraceRefs`, DTO fields, async builders, percent encoding, and `with_trace_refs_config`.

- [ ] Step: add DTO fields and ref helpers.
```rust
// crates/bridge-coordinator/src/coordinator.rs, update imports
use std::collections::{BTreeMap, HashMap};
use bridge_core::orch::{AgentSessionCaps, TerminalUsage, UsageSnapshot};
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, near StatusDto
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct TraceRefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<BTreeMap<String, String>>,
}

fn percent_encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b));
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

fn turn_ref(turn_id: &bridge_core::ids::TurnId) -> String {
    format!("/turns/{}", percent_encode_segment(turn_id.as_str()))
}

fn journal_ref(task_id: &TaskId) -> String {
    format!(
        "/tasks/{}/journal.jsonl",
        percent_encode_segment(task_id.as_str())
    )
}

fn artifact_ref(task_id: &TaskId, node: &bridge_core::ids::NodeId) -> String {
    format!(
        "/tasks/{}/artifacts/{}",
        percent_encode_segment(task_id.as_str()),
        percent_encode_segment(node.as_str())
    )
}
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, add trace to SessionStatusDto
#[serde(skip_serializing_if = "Option::is_none")]
pub trace: Option<TraceRefs>,
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, add fields to TaskStatusDto
#[serde(skip_serializing_if = "Option::is_none")]
pub usage: Option<UsageSnapshot>,
#[serde(skip_serializing_if = "Option::is_none")]
pub trace: Option<TraceRefs>,
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, update From impls to preserve no-usage/no-trace path
impl From<&crate::session_manager::SessionStatusInfo> for SessionStatusDto {
    fn from(info: &crate::session_manager::SessionStatusInfo) -> Self {
        Self {
            state: info.state,
            agent: info.agent.clone(),
            generation: info.generation,
            idle_age_ms: info.idle_age_ms,
            capabilities: info.capabilities.clone(),
            usage: info.usage.clone(),
            over_threshold: info.over_threshold,
            trace: None,
        }
    }
}

impl From<&TaskRecord> for TaskStatusDto {
    fn from(rec: &TaskRecord) -> Self {
        Self {
            id: rec.id.clone(),
            workflow: rec.workflow.clone(),
            status: rec.status.as_str(),
            result: rec.result.clone(),
            error: rec.error.clone(),
            updated_ms: rec.updated_ms,
            usage: None,
            trace: None,
        }
    }
}
```

- [ ] Step: add Coordinator trace fields and builder.
```rust
// crates/bridge-coordinator/src/coordinator.rs, add fields to Coordinator
trace_refs_enabled: bool,
max_task_turns: usize,
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, initialize in Coordinator::new
trace_refs_enabled: false,
max_task_turns: 512,
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, add builder near with_permission_registry
#[must_use]
pub fn with_trace_refs_config(mut self, enabled: bool, max_task_turns: usize) -> Self {
    self.trace_refs_enabled = enabled;
    self.max_task_turns = max_task_turns;
    self
}
```

- [ ] Step: add async DTO builders and use them from `Coordinator::status`.
```rust
// crates/bridge-coordinator/src/coordinator.rs, inside impl Coordinator
async fn session_status_dto(
    &self,
    ctx: &ContextId,
    info: &crate::session_manager::SessionStatusInfo,
) -> Result<SessionStatusDto, BridgeError> {
    let mut dto = SessionStatusDto::from(info);
    if self.trace_refs_enabled {
        if let Some(row) = self.task_store.latest_turn_log_row_for_session(ctx).await? {
            dto.trace = Some(TraceRefs {
                turn: Some(turn_ref(&row.turn_id)),
                ..TraceRefs::default()
            });
        }
    }
    Ok(dto)
}

async fn task_status_dto(&self, rec: &TaskRecord) -> Result<TaskStatusDto, BridgeError> {
    let mut dto = TaskStatusDto::from(rec);

    if let Some(agg) = self.task_store.turn_log_usage_for_task(&rec.id).await? {
        dto.usage = Some(UsageSnapshot {
            used: None,
            size: None,
            cost: agg.cost,
            terminal: Some(TerminalUsage {
                total_tokens: agg.input_tokens + agg.output_tokens,
                input_tokens: agg.input_tokens,
                output_tokens: agg.output_tokens,
                thought_tokens: agg.thought_tokens,
                cached_read_tokens: agg.cached_read_tokens,
                cached_write_tokens: agg.cached_write_tokens,
            }),
            at_ms: if agg.at_ms == 0 { rec.updated_ms } else { agg.at_ms },
        });
    }

    if self.trace_refs_enabled {
        let turn_rows = self
            .task_store
            .turn_log_rows_for_task(&rec.id, self.max_task_turns)
            .await?;
        let turns = if turn_rows.is_empty() {
            None
        } else {
            Some(turn_rows.iter().map(|row| turn_ref(&row.turn_id)).collect())
        };

        let nodes = self.task_store.node_checkpoint_nodes(&rec.id).await?;
        let artifacts = if nodes.is_empty() {
            None
        } else {
            Some(
                nodes
                    .iter()
                    .map(|node| (node.as_str().to_string(), artifact_ref(&rec.id, node)))
                    .collect::<BTreeMap<_, _>>(),
            )
        };

        dto.trace = Some(TraceRefs {
            turn: None,
            turns,
            journal: Some(journal_ref(&rec.id)),
            artifacts,
        });
    }

    Ok(dto)
}
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, replace status() match arms
(Some(c), None) => {
    let info = self
        .session_manager
        .status(&c)
        .await
        .ok_or(BridgeError::SessionNotFound)?;
    Ok(StatusDto::Session(self.session_status_dto(&c, &info).await?))
}
(None, Some(t)) => {
    let rec = self
        .task_store
        .get(&t)
        .await?
        .ok_or(BridgeError::TaskNotFound)?;
    Ok(StatusDto::Task(self.task_status_dto(&rec).await?))
}
```

- [ ] Step: wire trace DTO config from `main`.
```rust
// bin/a2a-bridge/src/main.rs, add parameters to build_coordinator()
trace_refs_enabled: bool,
max_task_turns: usize,
```

```rust
// bin/a2a-bridge/src/main.rs, update build_coordinator() body
Arc::new(
    bridge_coordinator::Coordinator::new(
        session_manager,
        Some(executor),
        Arc::new(wf_map),
        task_store,
        session_store,
        policy,
        registry,
        clock,
        allowed_cwd_root,
        batch,
        observer,
        resume_cap,
    )
    .with_trace_refs_config(trace_refs_enabled, max_task_turns)
    .with_permission_registry(perm_registry),
)
```

```rust
// bin/a2a-bridge/src/main.rs, update serve call to build_coordinator()
traces_cfg.enabled,
traces_cfg.max_task_turns,
```

```rust
// bin/a2a-bridge/src/main.rs, update MCP or other non-serve build_coordinator() calls
false,
512,
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-coordinator trace_refs_skip_absent_fields task_status_dto_omits_usage_trace_when_none task_usage_aggregates_from_turn_log_single_currency task_usage_omits_cost_for_mixed_currencies task_usage_terminal_total_tokens_is_input_plus_output trace_ref_segments_are_percent_encoded task_trace_turn_refs_are_capped_but_usage_is_not session_status_includes_latest_warm_turn_trace_ref -- --nocapture
cargo test -p a2a-bridge turn_log_observer_enabled_for_traces_even_without_metrics -- --nocapture
```
Expected PASS: status usage is uncapped, trace refs are capped and percent-encoded, and session latest-turn refs are populated only when enabled.

- [ ] Step: commit.
```bash
git add crates/bridge-coordinator/src/coordinator.rs bin/a2a-bridge/src/main.rs && git commit -m "feat: add trace refs to status dto"
```

---

### Task 7: Common Trace HTTP Gates and `/turns/{turn_id}`

**Files:** Modify/test `crates/bridge-a2a-inbound/src/server.rs:25`, `crates/bridge-a2a-inbound/src/server.rs:264`, `crates/bridge-a2a-inbound/src/server.rs:789`, `crates/bridge-a2a-inbound/src/server.rs:3615`, `crates/bridge-a2a-inbound/src/server.rs:4415`, `crates/bridge-a2a-inbound/src/server.rs:4601`.
**Interfaces:** Consumes Task 1 `TraceHttpConfig`, Task 2 `TaskStore::turn_log_row`, existing `bearer_token(&HeaderMap) -> Option<String>`, `InboundRequest::with_token(&str)`, `AuthMiddleware::authorize`. Produces route handler `turn_row(State<Arc<InboundServer>>, Path<String>, HeaderMap) -> Response`, `trace_authorize`, `TurnLogRowDto`, `trace_json_response`, `trace_error_response`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-a2a-inbound/src/server.rs, inside #[cfg(test)] mod tests
mod trace_turn_route_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{TraceParent, TurnContext, TurnOutcome};
    use bridge_core::task_store::{MemoryTaskStore, TaskStore, TurnLogFinished, TurnLogUsage};
    use std::time::Duration;
    use tower::ServiceExt;

    fn build_with_task_store_and_trace(
        task_store: Arc<MemoryTaskStore>,
        auth: Arc<dyn AuthMiddleware>,
        trace_config: TraceHttpConfig,
    ) -> Arc<InboundServer> {
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry: Arc<dyn AgentRegistry> =
            FakeRegistry::single("kiro", Arc::new(FakeBackend::new()));
        let task_store_dyn: Arc<dyn TaskStore> = task_store;
        let coord = bridge_coordinator::Coordinator::new(
            Arc::new(crate::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            )),
            None,
            Arc::new(std::collections::HashMap::new()),
            task_store_dyn,
            store,
            Arc::new(AutoApprove),
            registry,
            Arc::new(bridge_coordinator::clock::SystemClock),
            None,
            None,
            Arc::new(bridge_observ::NoopObserver),
            3,
        )
        .with_trace_refs_config(trace_config.enabled, trace_config.max_task_turns);
        Arc::new(
            InboundServer::from_coordinator(
                Arc::new(coord),
                Arc::new(AlwaysKiro),
                auth,
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "kiro",
            )
            .with_trace_http_config(trace_config),
        )
    }

    fn trace_cfg(enabled: bool) -> TraceHttpConfig {
        TraceHttpConfig {
            enabled,
            journal_max_bytes: 1024,
            journal_max_events: 16,
            artifact_max_bytes: 1024,
            max_task_turns: 4,
        }
    }

    async fn seed_turn(store: &MemoryTaskStore, turn_id: &str, session_id: &str, task_id: Option<&str>) {
        let ctx = TurnContext {
            turn_id: bridge_core::ids::TurnId::parse(turn_id).unwrap(),
            session_id: ContextId::parse(session_id).unwrap(),
            task_id: task_id.map(|t| TaskId::parse(t).unwrap()),
            workflow: Some("code-review".into()),
            node: Some("reviewer".into()),
            attempt: 2,
            agent: "codex".into(),
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            mode: Some("default".into()),
            prompt_id: Some("prompt/eval".into()),
            traceparent: TraceParent::parse_header_value(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        };
        store
            .upsert_turn_finished(&TurnLogFinished {
                ctx: ctx.clone(),
                started_ms: 90,
                completed_ms: 100,
                latency: Duration::from_millis(10),
                ttft: Some(Duration::from_millis(3)),
                outcome: TurnOutcome::Success,
            })
            .await
            .unwrap();
        store
            .update_turn_usage(&TurnLogUsage {
                ctx,
                usage: UsageSnapshot {
                    used: None,
                    size: None,
                    cost: Some(UsageCost {
                        amount: 0.42,
                        currency: "USD".into(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: 99,
                        input_tokens: 7,
                        output_tokens: 11,
                        thought_tokens: None,
                        cached_read_tokens: None,
                        cached_write_tokens: None,
                    }),
                    at_ms: 100,
                },
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn trace_routes_404_when_disabled_even_without_bearer() {
        let store = Arc::new(MemoryTaskStore::new());
        let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(false));

        let resp = router(srv)
            .oneshot(Request::builder().uri("/turns/turn-a").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trace_routes_require_bearer_when_enabled() {
        let store = Arc::new(MemoryTaskStore::new());
        let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

        let resp = router(srv)
            .oneshot(Request::builder().uri("/turns/turn-a").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(axum::http::header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    #[tokio::test]
    async fn trace_routes_reject_bad_bearer() {
        let store = Arc::new(MemoryTaskStore::new());
        let srv = build_with_task_store_and_trace(store, Arc::new(RejectAuth), trace_cfg(true));

        let resp = router(srv)
            .oneshot(
                Request::builder()
                    .uri("/turns/turn-a")
                    .header("authorization", "Bearer bad")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn turn_route_returns_json_turn_log_row() {
        let store = Arc::new(MemoryTaskStore::new());
        seed_turn(&store, "turn-a", "ctx-a", Some("task-a")).await;
        let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

        let resp = router(srv)
            .oneshot(
                Request::builder()
                    .uri("/turns/turn-a")
                    .header("authorization", "Bearer ok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body = body_string(resp).await;
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["turn_id"], "turn-a");
        assert_eq!(value["task_id"], "task-a");
        assert_eq!(value["input_tokens"], 7);
        assert_eq!(value["cost_currency"], "USD");
        assert_eq!(
            value["traceparent"],
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
    }

    #[tokio::test]
    async fn turn_route_returns_warm_turn_row() {
        let store = Arc::new(MemoryTaskStore::new());
        seed_turn(&store, "turn-warm", "ctx-warm", None).await;
        let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

        let resp = router(srv)
            .oneshot(
                Request::builder()
                    .uri("/turns/turn-warm")
                    .header("authorization", "Bearer ok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["turn_id"], "turn-warm");
        assert!(value["task_id"].is_null());
    }
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-a2a-inbound trace_routes_404_when_disabled_even_without_bearer trace_routes_require_bearer_when_enabled trace_routes_reject_bad_bearer turn_route_returns_json_turn_log_row turn_route_returns_warm_turn_row -- --nocapture
```
Expected FAIL: `/turns/{turn_id}` is not mounted and handler types do not exist.

- [ ] Step: add imports, route mount, and common response helpers.
```rust
// crates/bridge-a2a-inbound/src/server.rs, update axum imports
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, add route in router()
let router = Router::new()
    .route("/.well-known/agent-card.json", get(serve_card))
    .route("/turns/:turn_id", get(turn_row))
    .route("/", post(jsonrpc));
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, near metrics()
fn unauthorized_bearer_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
    )
        .into_response()
}

fn trace_json_response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}

fn trace_empty_response(status: StatusCode) -> Response {
    status.into_response()
}

fn trace_authorize(
    srv: &InboundServer,
    headers: &HeaderMap,
) -> Result<bridge_core::domain::AuthContext, Response> {
    if !srv.trace_config.enabled {
        return Err(trace_empty_response(StatusCode::NOT_FOUND));
    }
    let Some(token) = bearer_token(headers) else {
        return Err(unauthorized_bearer_response());
    };
    srv.auth
        .authorize(&InboundRequest::with_token(&token))
        .map_err(|_| unauthorized_bearer_response())
}

fn audit_trace_fetch(
    caller: &str,
    route: &'static str,
    task_id: Option<&str>,
    turn_id: Option<&str>,
    node: Option<&str>,
    status: StatusCode,
    bytes: usize,
) {
    tracing::info!(
        caller = caller,
        route = route,
        task_id = task_id,
        turn_id = turn_id,
        node = node,
        status = status.as_u16(),
        bytes = bytes as u64,
        "trace_fetch"
    );
}
```

- [ ] Step: add turn-row DTO and handler.
```rust
// crates/bridge-a2a-inbound/src/server.rs, near trace helpers
#[derive(serde::Serialize)]
struct TurnLogRowDto {
    turn_id: String,
    session_id: String,
    task_id: Option<String>,
    workflow: Option<String>,
    node: Option<String>,
    attempt: u32,
    agent: String,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    prompt_id: Option<String>,
    started_ms: Option<i64>,
    completed_ms: Option<i64>,
    latency_ms: Option<u64>,
    ttft_ms: Option<u64>,
    outcome: Option<String>,
    failure_class: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    thought_tokens: Option<u64>,
    cached_read_tokens: Option<u64>,
    cached_write_tokens: Option<u64>,
    cost_amount: Option<f64>,
    cost_currency: Option<String>,
    traceparent: Option<String>,
}

impl From<bridge_core::task_store::TurnLogRow> for TurnLogRowDto {
    fn from(row: bridge_core::task_store::TurnLogRow) -> Self {
        Self {
            turn_id: row.turn_id.as_str().to_string(),
            session_id: row.session_id.as_str().to_string(),
            task_id: row.task_id.map(|t| t.as_str().to_string()),
            workflow: row.workflow,
            node: row.node,
            attempt: row.attempt,
            agent: row.agent,
            model: row.model,
            effort: row.effort,
            mode: row.mode,
            prompt_id: row.prompt_id,
            started_ms: row.started_ms,
            completed_ms: row.completed_ms,
            latency_ms: row.latency_ms,
            ttft_ms: row.ttft_ms,
            outcome: row.outcome,
            failure_class: row.failure_class,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            thought_tokens: row.thought_tokens,
            cached_read_tokens: row.cached_read_tokens,
            cached_write_tokens: row.cached_write_tokens,
            cost_amount: row.cost_amount,
            cost_currency: row.cost_currency,
            traceparent: row.traceparent.map(|tp| tp.to_header_value()),
        }
    }
}

async fn turn_row(
    State(srv): State<Arc<InboundServer>>,
    Path(turn_id_raw): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut caller = "unauthenticated".to_string();
    let response = match trace_authorize(&srv, &headers) {
        Ok(auth) => {
            caller = auth.caller_id().as_str().to_string();
            let turn_id = match bridge_core::ids::TurnId::parse(turn_id_raw.clone()) {
                Ok(id) => id,
                Err(_) => {
                    let response = trace_empty_response(StatusCode::NOT_FOUND);
                    audit_trace_fetch(&caller, "turn_row", None, Some(&turn_id_raw), None, StatusCode::NOT_FOUND, 0);
                    return response;
                }
            };
            match srv.task_store().turn_log_row(&turn_id).await {
                Ok(Some(row)) => {
                    let body = serde_json::to_value(TurnLogRowDto::from(row))
                        .unwrap_or_else(|_| serde_json::json!({ "error": "serialization failed" }));
                    let bytes = serde_json::to_vec(&body).map(|b| b.len()).unwrap_or(0);
                    let response = trace_json_response(StatusCode::OK, body);
                    audit_trace_fetch(&caller, "turn_row", None, Some(turn_id.as_str()), None, StatusCode::OK, bytes);
                    return response;
                }
                Ok(None) => trace_empty_response(StatusCode::NOT_FOUND),
                Err(_) => trace_empty_response(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        Err(response) => response,
    };

    let status = response.status();
    audit_trace_fetch(
        &caller,
        "turn_row",
        None,
        Some(&turn_id_raw),
        None,
        status,
        0,
    );
    response
}
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-a2a-inbound trace_routes_404_when_disabled_even_without_bearer trace_routes_require_bearer_when_enabled trace_routes_reject_bad_bearer turn_route_returns_json_turn_log_row turn_route_returns_warm_turn_row -- --nocapture
```
Expected PASS: disabled returns 404 before auth, enabled requires bearer, bad bearer is 401, and `/turns/{turn_id}` returns JSON with `nosniff`.

- [ ] Step: commit.
```bash
git add crates/bridge-a2a-inbound/src/server.rs && git commit -m "feat: add authenticated turn trace route"
```

---

### Task 8: Journal and Artifact HTTP Routes

**Files:** Modify/test `crates/bridge-a2a-inbound/src/server.rs:264`, `crates/bridge-a2a-inbound/src/server.rs:789`, `crates/bridge-a2a-inbound/src/server.rs:4601`.
**Interfaces:** Consumes Task 2 `JournalRead`, `NodeCheckpointOutput`; Task 5 `workflow_spec_node_ids`; Task 7 `trace_authorize`, `trace_empty_response`, `audit_trace_fetch`. Produces handlers `task_journal_jsonl` and `task_artifact`.

- [ ] Step: write the failing tests.
```rust
// crates/bridge-a2a-inbound/src/server.rs, inside trace_turn_route_tests module from Task 7
use bridge_core::task_store::{JournalRead, NodeCheckpointOutput, TaskRecord, TaskRecordStatus};
use bridge_core::ids::{NodeId, OperationId, WorkflowId, AgentId};
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};

fn working_task_record(id: &str, workflow_spec_json: Option<String>) -> TaskRecord {
    TaskRecord {
        id: TaskId::parse(id).unwrap(),
        workflow: "code-review".into(),
        status: TaskRecordStatus::Working,
        result: None,
        error: None,
        created_ms: 1,
        updated_ms: 1,
        input: "input".into(),
        workflow_spec_json,
        resume_attempts: 0,
        session_cwd: None,
        batch_id: None,
        item_id: None,
    }
}

fn completed_task_record(id: &str, workflow_spec_json: Option<String>) -> TaskRecord {
    let mut rec = working_task_record(id, workflow_spec_json);
    rec.status = TaskRecordStatus::Completed;
    rec.result = Some("done".into());
    rec.updated_ms = 2;
    rec
}

fn two_node_spec() -> String {
    let graph = WorkflowGraph {
        id: WorkflowId::parse("code-review").unwrap(),
        nodes: vec![
            WorkflowNode {
                id: NodeId::parse("reviewer").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: Vec::new(),
                retry: None,
            },
            WorkflowNode {
                id: NodeId::parse("synth").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "{{reviewer}}".into(),
                inputs: vec![NodeId::parse("reviewer").unwrap()],
                retry: None,
            },
        ],
        panel: None,
    };
    bridge_coordinator::detached::encode_workflow_spec(&graph)
}

#[tokio::test]
async fn journal_route_returns_ndjson_with_content_length() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-journal").unwrap();
    let op = OperationId::parse("op-journal").unwrap();
    store.create(&working_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            10,
            bridge_core::orch::OrchEventKind::Progress {
                message: "one".into(),
            },
        )
        .await
        .unwrap();

    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));
    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-journal/journal.jsonl")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/x-ndjson"
    );
    assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");
    assert!(resp.headers().get(axum::http::header::CONTENT_LENGTH).is_some());
    let body = body_string(resp).await;
    assert_eq!(body.lines().count(), 1);
    assert!(body.ends_with('\n'));
}

#[tokio::test]
async fn journal_route_empty_working_task_200() {
    let store = Arc::new(MemoryTaskStore::new());
    store
        .create(&working_task_record("task-empty-working", Some(two_node_spec())))
        .await
        .unwrap();
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-empty-working/journal.jsonl")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "");
}

#[tokio::test]
async fn journal_route_terminal_empty_journal_404() {
    let store = Arc::new(MemoryTaskStore::new());
    store
        .create(&completed_task_record("task-empty-terminal", Some(two_node_spec())))
        .await
        .unwrap();
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-empty-terminal/journal.jsonl")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn journal_route_413_over_byte_limit() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-large-journal").unwrap();
    let op = OperationId::parse("op-journal").unwrap();
    store.create(&working_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    store
        .record_event_sequenced(
            &task,
            &op,
            10,
            bridge_core::orch::OrchEventKind::Progress {
                message: "large".repeat(64),
            },
        )
        .await
        .unwrap();

    let mut cfg = trace_cfg(true);
    cfg.journal_max_bytes = 8;
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), cfg);

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-large-journal/journal.jsonl")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn journal_route_413_over_event_limit() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-many-journal").unwrap();
    let op = OperationId::parse("op-journal").unwrap();
    store.create(&working_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    for message in ["one", "two"] {
        store
            .record_event_sequenced(
                &task,
                &op,
                10,
                bridge_core::orch::OrchEventKind::Progress {
                    message: message.into(),
                },
            )
            .await
            .unwrap();
    }

    let mut cfg = trace_cfg(true);
    cfg.journal_max_events = 1;
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), cfg);

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-many-journal/journal.jsonl")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn artifact_route_returns_plain_text_nosniff() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-artifact").unwrap();
    store.create(&completed_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    store
        .put_node_checkpoint(&task, &NodeId::parse("reviewer").unwrap(), "artifact text", true, 10)
        .await
        .unwrap();
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-artifact/artifacts/reviewer")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/plain; charset=utf-8"
    );
    assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(body_string(resp).await, "artifact text");
}

#[tokio::test]
async fn artifact_route_validates_node_against_snapshot() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-artifact").unwrap();
    store.create(&completed_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    store
        .put_node_checkpoint(&task, &NodeId::parse("reviewer").unwrap(), "artifact text", true, 10)
        .await
        .unwrap();
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-artifact/artifacts/not-in-snapshot")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_route_404_for_known_unfinished_node() {
    let store = Arc::new(MemoryTaskStore::new());
    store
        .create(&working_task_record("task-unfinished", Some(two_node_spec())))
        .await
        .unwrap();
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-unfinished/artifacts/reviewer")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_route_413_when_output_too_large() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-large-artifact").unwrap();
    store.create(&completed_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    store
        .put_node_checkpoint(&task, &NodeId::parse("reviewer").unwrap(), "abcdef", true, 10)
        .await
        .unwrap();

    let mut cfg = trace_cfg(true);
    cfg.artifact_max_bytes = 5;
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), cfg);

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-large-artifact/artifacts/reviewer")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn node_id_invalid_maps_to_404() {
    let store = Arc::new(MemoryTaskStore::new());
    store
        .create(&working_task_record("task-invalid-node", Some(two_node_spec())))
        .await
        .unwrap();
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/task-invalid-node/artifacts/../secret")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trace_ref_after_purge_returns_404() {
    let store = Arc::new(MemoryTaskStore::new());
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let resp = router(srv)
        .oneshot(
            Request::builder()
                .uri("/tasks/purged-task/journal.jsonl")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-a2a-inbound journal_route_returns_ndjson_with_content_length journal_route_empty_working_task_200 journal_route_terminal_empty_journal_404 journal_route_413_over_byte_limit journal_route_413_over_event_limit artifact_route_returns_plain_text_nosniff artifact_route_validates_node_against_snapshot artifact_route_404_for_known_unfinished_node artifact_route_413_when_output_too_large node_id_invalid_maps_to_404 trace_ref_after_purge_returns_404 -- --nocapture
```
Expected FAIL: journal and artifact routes are not mounted.

- [ ] Step: mount journal and artifact routes.
```rust
// crates/bridge-a2a-inbound/src/server.rs, in router()
.route("/tasks/:id/journal.jsonl", get(task_journal_jsonl))
.route("/tasks/:id/artifacts/:node", get(task_artifact))
```

- [ ] Step: implement 413 JSON and NDJSON response helpers.
```rust
// crates/bridge-a2a-inbound/src/server.rs, near trace helpers
fn trace_too_large_response(kind: &'static str, bytes: u64, events: Option<u64>) -> Response {
    trace_json_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        serde_json::json!({
            "error": "trace payload too large",
            "kind": kind,
            "bytes": bytes,
            "events": events,
        }),
    )
}

fn trace_ndjson_response(body: String) -> Response {
    let len = body.len().to_string();
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CONTENT_LENGTH, len.as_str()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&len).unwrap(),
    );
    response
}

fn trace_text_response(body: String) -> Response {
    let mut response = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response();
    response
}
```

- [ ] Step: implement journal handler.
```rust
// crates/bridge-a2a-inbound/src/server.rs
async fn task_journal_jsonl(
    State(srv): State<Arc<InboundServer>>,
    Path(task_id_raw): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut caller = "unauthenticated".to_string();
    let auth = match trace_authorize(&srv, &headers) {
        Ok(auth) => {
            caller = auth.caller_id().as_str().to_string();
            auth
        }
        Err(response) => {
            let status = response.status();
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(&task_id_raw), None, None, status, 0);
            return response;
        }
    };
    caller = auth.caller_id().as_str().to_string();

    let task_id = match TaskId::parse(task_id_raw.clone()) {
        Ok(id) => id,
        Err(_) => {
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(&task_id_raw), None, None, StatusCode::NOT_FOUND, 0);
            return trace_empty_response(StatusCode::NOT_FOUND);
        }
    };

    let rec = match srv.task_store().get(&task_id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(task_id.as_str()), None, None, StatusCode::NOT_FOUND, 0);
            return trace_empty_response(StatusCode::NOT_FOUND);
        }
        Err(_) => {
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(task_id.as_str()), None, None, StatusCode::INTERNAL_SERVER_ERROR, 0);
            return trace_empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match srv
        .task_store()
        .journal_jsonl_bounded(
            &task_id,
            srv.trace_config.journal_max_events,
            srv.trace_config.journal_max_bytes,
        )
        .await
    {
        Ok(bridge_core::task_store::JournalRead::Body { jsonl, events, bytes }) => {
            if events == 0 && rec.status.is_terminal() {
                audit_trace_fetch(&caller, "task_journal_jsonl", Some(task_id.as_str()), None, None, StatusCode::NOT_FOUND, 0);
                return trace_empty_response(StatusCode::NOT_FOUND);
            }
            let response = trace_ndjson_response(jsonl);
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(task_id.as_str()), None, None, StatusCode::OK, bytes as usize);
            response
        }
        Ok(bridge_core::task_store::JournalRead::TooLarge { events, bytes }) => {
            let response = trace_too_large_response("journal", bytes, Some(events));
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(task_id.as_str()), None, None, StatusCode::PAYLOAD_TOO_LARGE, 0);
            response
        }
        Err(_) => {
            audit_trace_fetch(&caller, "task_journal_jsonl", Some(task_id.as_str()), None, None, StatusCode::INTERNAL_SERVER_ERROR, 0);
            trace_empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
```

- [ ] Step: implement artifact handler with snapshot validation.
```rust
// crates/bridge-a2a-inbound/src/server.rs
async fn task_artifact(
    State(srv): State<Arc<InboundServer>>,
    Path((task_id_raw, node_raw)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let mut caller = "unauthenticated".to_string();
    let auth = match trace_authorize(&srv, &headers) {
        Ok(auth) => {
            caller = auth.caller_id().as_str().to_string();
            auth
        }
        Err(response) => {
            let status = response.status();
            audit_trace_fetch(&caller, "task_artifact", Some(&task_id_raw), None, Some(&node_raw), status, 0);
            return response;
        }
    };
    caller = auth.caller_id().as_str().to_string();

    let task_id = match TaskId::parse(task_id_raw.clone()) {
        Ok(id) => id,
        Err(_) => {
            audit_trace_fetch(&caller, "task_artifact", Some(&task_id_raw), None, Some(&node_raw), StatusCode::NOT_FOUND, 0);
            return trace_empty_response(StatusCode::NOT_FOUND);
        }
    };
    let node = match bridge_core::ids::NodeId::parse(node_raw.clone()) {
        Ok(node) => node,
        Err(_) => {
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(&node_raw), StatusCode::NOT_FOUND, 0);
            return trace_empty_response(StatusCode::NOT_FOUND);
        }
    };

    let rec = match srv.task_store().get(&task_id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::NOT_FOUND, 0);
            return trace_empty_response(StatusCode::NOT_FOUND);
        }
        Err(_) => {
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::INTERNAL_SERVER_ERROR, 0);
            return trace_empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let allowed = if let Some(spec_json) = rec.workflow_spec_json.as_deref() {
        match bridge_coordinator::detached::workflow_spec_node_ids(spec_json) {
            Ok(nodes) => nodes.contains(&node),
            Err(_) => false,
        }
    } else {
        match srv.task_store().node_checkpoint_nodes(&task_id).await {
            Ok(nodes) => nodes.iter().any(|candidate| candidate == &node),
            Err(_) => {
                audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::INTERNAL_SERVER_ERROR, 0);
                return trace_empty_response(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    };

    if !allowed {
        audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::NOT_FOUND, 0);
        return trace_empty_response(StatusCode::NOT_FOUND);
    }

    match srv
        .task_store()
        .node_checkpoint_output(&task_id, &node, srv.trace_config.artifact_max_bytes)
        .await
    {
        Ok(Some(bridge_core::task_store::NodeCheckpointOutput::Found { output, bytes, .. })) => {
            let response = trace_text_response(output);
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::OK, bytes as usize);
            response
        }
        Ok(Some(bridge_core::task_store::NodeCheckpointOutput::TooLarge { bytes })) => {
            let response = trace_too_large_response("artifact", bytes, None);
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::PAYLOAD_TOO_LARGE, 0);
            response
        }
        Ok(None) => {
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::NOT_FOUND, 0);
            trace_empty_response(StatusCode::NOT_FOUND)
        }
        Err(_) => {
            audit_trace_fetch(&caller, "task_artifact", Some(task_id.as_str()), None, Some(node.as_str()), StatusCode::INTERNAL_SERVER_ERROR, 0);
            trace_empty_response(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-a2a-inbound journal_route_returns_ndjson_with_content_length journal_route_empty_working_task_200 journal_route_terminal_empty_journal_404 journal_route_413_over_byte_limit journal_route_413_over_event_limit artifact_route_returns_plain_text_nosniff artifact_route_validates_node_against_snapshot artifact_route_404_for_known_unfinished_node artifact_route_413_when_output_too_large node_id_invalid_maps_to_404 trace_ref_after_purge_returns_404 -- --nocapture
```
Expected PASS: journal route is bounded and `Content-Length` delimited; artifact route validates task-scoped node ids and size limits.

- [ ] Step: commit.
```bash
git add crates/bridge-a2a-inbound/src/server.rs && git commit -m "feat: add task journal and artifact trace routes"
```

---

### Task 9: Inbound Acceptance Coverage, Session Trace JSON, Metrics Independence, and Audit Logs

**Files:** Modify `crates/bridge-a2a-inbound/Cargo.toml:25`; modify/test `crates/bridge-a2a-inbound/src/server.rs:3049`, `crates/bridge-a2a-inbound/src/server.rs:4601`; modify `bin/a2a-bridge/src/main.rs:6210`.
**Interfaces:** Consumes Task 6 `Coordinator::status` trace DTOs; Task 7/8 handlers and audit fields. Produces JSON-RPC `SessionStatus` response with optional `trace`, route independence tests, audit-log tests.

- [ ] Step: write the failing tests.
```toml
# crates/bridge-a2a-inbound/Cargo.toml
[dev-dependencies]
tracing-subscriber.workspace = true
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, inside trace_turn_route_tests module
use std::io::Write;

#[tokio::test]
async fn task_status_includes_usage_and_trace_refs() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-status").unwrap();
    store.create(&working_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    seed_turn(&store, "turn-status", "ctx-status", Some(task.as_str())).await;
    store
        .put_node_checkpoint(&task, &NodeId::parse("reviewer").unwrap(), "artifact", true, 10)
        .await
        .unwrap();

    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));
    let status = srv.coordinator().status(None, Some(task)).await.unwrap();

    match status {
        bridge_coordinator::StatusDto::Task(dto) => {
            assert!(dto.usage.is_some());
            let trace = dto.trace.unwrap();
            assert_eq!(trace.journal.unwrap(), "/tasks/task-status/journal.jsonl");
            assert_eq!(trace.turns.unwrap(), vec!["/turns/turn-status"]);
            assert_eq!(
                trace.artifacts.unwrap().get("reviewer").unwrap(),
                "/tasks/task-status/artifacts/reviewer"
            );
        }
        bridge_coordinator::StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn task_status_usage_present_when_traces_disabled() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-status-no-trace").unwrap();
    store.create(&working_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    seed_turn(&store, "turn-status-no-trace", "ctx-status", Some(task.as_str())).await;

    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(false));
    let status = srv.coordinator().status(None, Some(task)).await.unwrap();

    match status {
        bridge_coordinator::StatusDto::Task(dto) => {
            assert!(dto.usage.is_some());
            assert!(dto.trace.is_none());
        }
        bridge_coordinator::StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn usage_uncapped_beyond_max_task_turns() {
    let store = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-uncapped").unwrap();
    store.create(&working_task_record(task.as_str(), Some(two_node_spec()))).await.unwrap();
    for i in 0..5 {
        seed_turn(&store, &format!("turn-uncapped-{i}"), "ctx-uncapped", Some(task.as_str())).await;
    }

    let mut cfg = trace_cfg(true);
    cfg.max_task_turns = 2;
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), cfg);
    let status = srv.coordinator().status(None, Some(task)).await.unwrap();

    match status {
        bridge_coordinator::StatusDto::Task(dto) => {
            assert_eq!(dto.trace.unwrap().turns.unwrap().len(), 2);
            assert_eq!(dto.usage.unwrap().terminal.unwrap().input_tokens, 35);
        }
        bridge_coordinator::StatusDto::Session(_) => panic!("expected task status"),
    }
}

#[tokio::test]
async fn session_status_includes_latest_warm_turn_trace_ref() {
    let store = Arc::new(MemoryTaskStore::new());
    seed_turn(&store, "turn-warm-latest", "ctx-warm-status", None).await;
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));
    srv.coordinator()
        .session_manager
        .ensure_idle_for_test(
            ContextId::parse("ctx-warm-status").unwrap(),
            AgentId::parse("kiro").unwrap(),
        )
        .await;

    let resp = router(srv)
        .oneshot(
            post_request(
                "SessionStatus",
                serde_json::json!({ "contextId": "ctx-warm-status" }),
                A2A_PINNED_VERSION,
            )
            .header("authorization", "Bearer ok")
            .body(jsonrpc_body(
                "SessionStatus",
                serde_json::json!({ "contextId": "ctx-warm-status" }),
            ))
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["result"]["trace"]["turn"],
        "/turns/turn-warm-latest"
    );
}

#[tokio::test]
async fn metrics_and_traces_independent() {
    let store = Arc::new(MemoryTaskStore::new());

    let metrics_only = Arc::new(
        Arc::into_inner(build_with_task_store_and_trace(
            store.clone(),
            Arc::new(AlwaysGrant),
            trace_cfg(false),
        ))
        .expect("unique server arc")
        .with_metrics_endpoint(Some(
            bridge_observ::PrometheusObserver::new(bridge_observ::LabelVocabulary::default())
                .unwrap()
                .endpoint(),
        )),
    );

    let metrics_resp = router(metrics_only.clone())
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(metrics_resp.status(), StatusCode::UNAUTHORIZED);

    let trace_resp = router(metrics_only)
        .oneshot(Request::builder().uri("/turns/turn-a").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(trace_resp.status(), StatusCode::NOT_FOUND);

    let traces_only = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));
    let trace_resp = router(traces_only.clone())
        .oneshot(Request::builder().uri("/turns/turn-a").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(trace_resp.status(), StatusCode::UNAUTHORIZED);

    let metrics_resp = router(traces_only)
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(metrics_resp.status(), StatusCode::NOT_FOUND);
}

#[derive(Clone)]
struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn trace_routes_audit_success_and_failure() {
    let store = Arc::new(MemoryTaskStore::new());
    seed_turn(&store, "turn-audit", "ctx-audit", Some("task-audit")).await;
    let srv = build_with_task_store_and_trace(store, Arc::new(AlwaysGrant), trace_cfg(true));

    let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer_logs = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(move || SharedLogWriter(writer_logs.clone()))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let ok = router(srv.clone())
        .oneshot(
            Request::builder()
                .uri("/turns/turn-audit")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let missing = router(srv)
        .oneshot(
            Request::builder()
                .uri("/turns/missing-audit")
                .header("authorization", "Bearer ok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let log_text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(log_text.contains("\"message\":\"trace_fetch\""));
    assert!(log_text.contains("\"route\":\"turn_row\""));
    assert!(log_text.contains("\"turn_id\":\"turn-audit\""));
    assert!(log_text.contains("\"status\":200"));
    assert!(log_text.contains("\"status\":404"));
    assert!(!log_text.contains("Bearer ok"));
}
```

- [ ] Step: run the failing tests.
```bash
cargo test -p bridge-a2a-inbound task_status_includes_usage_and_trace_refs task_status_usage_present_when_traces_disabled usage_uncapped_beyond_max_task_turns session_status_includes_latest_warm_turn_trace_ref metrics_and_traces_independent trace_routes_audit_success_and_failure -- --nocapture
```
Expected FAIL: `SessionStatus` response omits trace; audit test cannot compile until dev-dependency is added; main may not yet pass trace refs config to coordinator in all serve paths.

- [ ] Step: update `SessionStatus` JSON to use coordinator status trace.
```rust
// crates/bridge-a2a-inbound/src/server.rs, replace session_status body after ctx parse
match srv.coordinator().status(Some(ctx.clone()), None).await {
    Ok(bridge_coordinator::StatusDto::Session(s)) => {
        let mut result = json!({
            "contextId": ctx.as_str(),
            "state": s.state,
            "agent": s.agent,
            "generation": s.generation,
            "idleAgeMs": s.idle_age_ms,
            "capabilities": {
                "loadSession": s.capabilities.load_session,
                "resume": s.capabilities.resume,
                "close": s.capabilities.close,
                "list": s.capabilities.list,
                "delete": s.capabilities.delete,
            },
            "usage": {
                "used": s.usage.used,
                "size": s.usage.size,
                "windowFraction": srv.session_manager()
                    .status(&ctx)
                    .await
                    .map(|info| info.window_fraction())
                    .unwrap_or(0.0),
                "overThreshold": s.over_threshold,
                "cost": s.usage.cost.as_ref().map(|c| serde_json::json!({
                    "amount": c.amount, "currency": c.currency
                })),
                "atMs": s.usage.at_ms,
            },
            "pendingPermissions": srv.permission_registry()
                .as_ref()
                .map(|r| r.pending(&ctx))
                .unwrap_or_default(),
        });
        if let Some(trace) = s.trace {
            result["trace"] = serde_json::to_value(trace).expect("TraceRefs serializes");
        }
        jsonrpc_ok(id, result)
    }
    Ok(bridge_coordinator::StatusDto::Task(_)) => {
        bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest { field: "contextId" })
    }
    Err(e) => bridge_err_to_jsonrpc(id, &e),
}
```

- [ ] Step: ensure serve passes trace refs config into coordinator.
```rust
// bin/a2a-bridge/src/main.rs, in serve build_coordinator call
traces_cfg.enabled,
traces_cfg.max_task_turns,
```

- [ ] Step: run to green.
```bash
cargo test -p bridge-a2a-inbound task_status_includes_usage_and_trace_refs task_status_usage_present_when_traces_disabled usage_uncapped_beyond_max_task_turns session_status_includes_latest_warm_turn_trace_ref metrics_and_traces_independent trace_routes_audit_success_and_failure -- --nocapture
```
Expected PASS: status refs/usage acceptance, metrics-traces independence, and structured audit logs are covered.

- [ ] Step: commit.
```bash
git add crates/bridge-a2a-inbound/Cargo.toml crates/bridge-a2a-inbound/src/server.rs bin/a2a-bridge/src/main.rs && git commit -m "test: cover trace drilldown acceptance"
```

---

## Final Verification

- [ ] Step: run formatting check.
```bash
cargo fmt --all -- --check
```
Expected PASS.

- [ ] Step: run clippy.
```bash
cargo clippy --workspace --all-targets -- -D warnings
```
Expected PASS.

- [ ] Step: run full workspace tests serially.
```bash
cargo test --workspace -j 1
```
Expected PASS. Report total passed/failed/ignored from the final test summary.

- [ ] Step: run cargo-deny.
```bash
cargo deny check
```
Expected PASS.

- [ ] Step: commit any formatting-only fixes if `cargo fmt --all -- --check` required them.
```bash
git add . && git commit -m "chore: format trace drilldown changes"
```

## Self-review

- Acceptance 1 (`[traces]` defaults, validation, metrics independence): Task 1.
- Acceptance 2 (three routes exposed with status matrices): Tasks 7 and 8.
- Acceptance 3 (bearer when enabled, 404 when disabled): Task 7.
- Acceptance 4 (`/turns/{turn_id}` reads `turn_log_row`, JSON traceparent + nosniff): Task 7.
- Acceptance 5 (latest warm turn discoverable via `SessionStatusDto.trace.turn` and route): Tasks 6, 7, and 9.
- Acceptance 6 (bounded `journal.jsonl`, `Content-Length`, 413, terminal-empty 404, working-empty 200): Tasks 4 and 8.
- Acceptance 7 (artifact node validation, text/plain + nosniff, single-statement size check): Tasks 4, 5, and 8.
- Acceptance 8 (artifact serving never touches filesystem path): Task 8 uses only `TaskStore::node_checkpoint_output`.
- Acceptance 9 (structured audit per fetch): Tasks 7, 8, and 9.
- Acceptance 10 (`TaskStatusDto` optional usage/trace gating): Task 6 and Task 9.
- Acceptance 11 (usage and turn cost read only from uncapped `turn_log`; total tokens input+output): Tasks 2, 3, 6, and 9.
- Acceptance 12 (`max_task_turns` caps refs only, not accounting): Tasks 3, 6, and 9.
- Acceptance 13 (unknown/forbidden/mid-write/purged -> 404; refs relative and best-effort): Tasks 6, 7, and 8.
- Acceptance 14 (fmt, clippy, full tests, cargo deny): Final Verification.
- No placeholders remain; all task steps include concrete test code, implementation code, commands, expected results, and commit commands.
