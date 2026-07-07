# M4 Slice 1 — Metrics Seam + Turn-Log + /metrics — Implementation Plan

> For agentic workers: use the executing-plans / subagent-driven-development workflow. Steps use `- [ ]` checkboxes.

**Goal:** Add an opt-in observability seam that records per-turn metrics, durable turn rows, queue signals, and authenticated Prometheus exposition without changing core-domain dependency direction.
**Architecture:** `bridge-core` owns only Prometheus-free event and DTO ports; adapters live in `bridge-observ`. `Coordinator`, inbound local producers, workflow node execution, and batch admission emit `ObsEvent`s through an injected `Arc<dyn Observer>`, normally `NoopObserver`; enabled serve builds a fanout of Prometheus plus async SQLite turn-log sinks. The SQLite `turn_log` table is the restart-safe source for cost/token counters and the future Slice 2 drill-down route.
**Tech Stack:** Rust 1.94.0, `tokio`, `axum`, `rusqlite`, `prometheus`, existing `bridge-core`/`bridge-observ`/`bridge-store`/`bridge-coordinator`.

## Global Constraints
toolchain 1.94.0
fmt -D warnings + clippy + full --workspace test (-j 1)
prometheus confined to bridge-observ
prometheus types never in bridge-core
ids never Prometheus labels
opt-in default OFF
observability must never block/panic a turn.

## File Structure
- `Cargo.toml` — add workspace-pinned `prometheus`.
- `crates/bridge-core/src/ids.rs` — add `TurnId`.
- `crates/bridge-core/src/ports.rs` — add `TraceParent`, `TurnContext`, `FailureClass`, `TurnOutcome`, `UsageFinalization`, `ObsEvent`, `Observer`.
- `crates/bridge-core/src/task_store.rs` — add turn-log DTOs and `TaskStore` turn-log methods; add in-memory test implementation.
- `crates/bridge-store/src/sqlite.rs` — add `turn_log` DDL/indexes and SQLite turn-log methods.
- `crates/bridge-observ/Cargo.toml` — add `bridge-core`, `tokio`, `prometheus`.
- `crates/bridge-observ/src/lib.rs` — add `NoopObserver`, `FanoutObserver`, `PrometheusObserver`, `TurnLogObserver`, `MetricsEndpoint`, shared dedupe gate.
- `crates/bridge-coordinator/src/session_manager.rs` — carry agent/model/effort/mode on `WarmTurn`.
- `crates/bridge-coordinator/src/dispatch.rs` — carry `TurnContext` on `LocalDispatch`.
- `crates/bridge-coordinator/src/coordinator.rs` — inject observer and emit turn/usage events at `collect_turn`.
- `crates/bridge-coordinator/src/batch.rs` — add queue RAII guard around semaphore admission.
- `crates/bridge-workflow/src/executor.rs` — add observer to `WorkflowRunContext` and emit per-node turn events.
- `crates/bridge-a2a-inbound/src/server.rs` — parse `traceparent`, wire local producer observations, add optional `/metrics`.
- `bin/a2a-bridge/src/config.rs` — add `[metrics]` config with default disabled.
- `bin/a2a-bridge/src/main.rs` — construct observer fanout, pass it through coordinator/batch/server, rebuild counters from `turn_log`.

### Task 1: Core Observer Port + Default Noop
**Files:** Modify `crates/bridge-core/src/ids.rs:26-37`; modify `crates/bridge-core/src/ports.rs:4-7, 178-180`; modify `crates/bridge-observ/Cargo.toml:8-10`; modify/test `crates/bridge-observ/src/lib.rs:1-40`.
**Interfaces:** Consumes existing `UsageSnapshot`, `TaskId`, `ContextId`, `WorkflowId`, `NodeId`. Produces `TurnId`, `TraceParent::parse_header_value(&str) -> Option<TraceParent>`, `TraceParent::to_header_value(&self) -> String`, `Observer::record(&self, e: &ObsEvent<'_>)`, `NoopObserver`.
**Cohesion with Slices 2 & 3:** `ObsEvent` includes Task/Node lifecycle and `traceparent` now so Slice 2/OTLP can build drill-down/span trees without changing `bridge-core`; no retention behavior is introduced.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-observ/src/lib.rs
#[cfg(test)]
mod obs_port_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::ports::{ObsEvent, Observer, TraceParent, TurnContext, TurnOutcome};

    #[test]
    fn noop_observer_accepts_turn_lifecycle_events() {
        let observer = NoopObserver;
        let ctx = TurnContext {
            turn_id: TurnId::parse("turn-1").unwrap(),
            session_id: ContextId::parse("ctx-1").unwrap(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: None,
            prompt_id: Some("eval/smoke".to_string()),
            traceparent: TraceParent::parse_header_value(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        };
        observer.record(&ObsEvent::TaskStarted { ctx: &ctx });
        observer.record(&ObsEvent::NodeStarted { ctx: &ctx });
        observer.record(&ObsEvent::TurnStarted { ctx: &ctx });
        observer.record(&ObsEvent::TurnFinished {
            ctx: &ctx,
            latency: std::time::Duration::from_millis(7),
            ttft: Some(std::time::Duration::from_millis(2)),
            outcome: &TurnOutcome::Success,
        });
    }

    #[test]
    fn traceparent_parses_roundtrips_and_rejects_malformed() {
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let parsed = TraceParent::parse_header_value(raw).unwrap();
        assert_eq!(parsed.to_header_value(), raw);
        assert!(TraceParent::parse_header_value("00-not-hex-00f067aa0ba902b7-01").is_none());
        assert!(TraceParent::parse_header_value("ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none());
    }
}
```
- [ ] Step: run it, expect FAIL with unresolved `TurnId`, `TraceParent`, `TurnContext`, `ObsEvent`, `NoopObserver`
```bash
cargo test -p bridge-observ obs_port_tests -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// crates/bridge-core/src/ids.rs, add after id_newtype!(ContextId);
id_newtype!(TurnId);
```

```rust
// crates/bridge-core/src/ports.rs, add imports
use crate::orch::UsageSnapshot;
use std::time::Duration;

// crates/bridge-core/src/ports.rs, add before Registry / config-source ports
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceParent {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub flags: u8,
}

impl TraceParent {
    pub fn parse_header_value(raw: &str) -> Option<Self> {
        let mut parts = raw.split('-');
        let version = parts.next()?;
        let trace = parts.next()?;
        let span = parts.next()?;
        let flags = parts.next()?;
        if parts.next().is_some() || version != "00" || trace.len() != 32 || span.len() != 16 || flags.len() != 2 {
            return None;
        }
        let mut trace_id = [0_u8; 16];
        let mut span_id = [0_u8; 8];
        for i in 0..16 {
            trace_id[i] = u8::from_str_radix(&trace[i * 2..i * 2 + 2], 16).ok()?;
        }
        for i in 0..8 {
            span_id[i] = u8::from_str_radix(&span[i * 2..i * 2 + 2], 16).ok()?;
        }
        if trace_id.iter().all(|b| *b == 0) || span_id.iter().all(|b| *b == 0) {
            return None;
        }
        Some(Self {
            trace_id,
            span_id,
            flags: u8::from_str_radix(flags, 16).ok()?,
        })
    }

    pub fn to_header_value(&self) -> String {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        }
        format!("00-{}-{}-{:02x}", hex(&self.trace_id), hex(&self.span_id), self.flags)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnContext {
    pub turn_id: TurnId,
    pub session_id: ContextId,
    pub task_id: Option<TaskId>,
    pub workflow: Option<String>,
    pub node: Option<String>,
    pub attempt: u32,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub prompt_id: Option<String>,
    pub traceparent: Option<TraceParent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    AgentCrashed,
    TimedOut,
    Overloaded,
    Config,
    Transport,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Success,
    Failed(FailureClass),
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageFinalization {
    TurnFinal,
    TaskFinal,
    Partial,
}

#[derive(Debug)]
pub enum ObsEvent<'a> {
    TaskStarted { ctx: &'a TurnContext },
    TaskFinished { ctx: &'a TurnContext, outcome: &'a TurnOutcome },
    NodeStarted { ctx: &'a TurnContext },
    NodeFinished { ctx: &'a TurnContext, outcome: &'a TurnOutcome },
    TurnStarted { ctx: &'a TurnContext },
    TurnFinished {
        ctx: &'a TurnContext,
        latency: Duration,
        ttft: Option<Duration>,
        outcome: &'a TurnOutcome,
    },
    QueueChanged {
        in_flight: u64,
        queued: u64,
        wait: Option<Duration>,
    },
    UsageFinalized {
        ctx: &'a TurnContext,
        usage: &'a UsageSnapshot,
        fin: UsageFinalization,
    },
}

pub trait Observer: Send + Sync {
    fn record(&self, e: &ObsEvent<'_>);
}
```

```toml
# crates/bridge-observ/Cargo.toml
[dependencies]
bridge-core = { path = "../bridge-core" }
tracing.workspace = true
tracing-subscriber.workspace = true
```

```rust
// crates/bridge-observ/src/lib.rs, add below tracing helpers
use bridge_core::ports::{ObsEvent, Observer};
use std::sync::Arc;

pub struct NoopObserver;

impl Observer for NoopObserver {
    fn record(&self, _e: &ObsEvent<'_>) {}
}

pub struct FanoutObserver {
    sinks: Vec<Arc<dyn Observer>>,
}

impl FanoutObserver {
    pub fn new(sinks: Vec<Arc<dyn Observer>>) -> Self {
        Self { sinks }
    }
}

impl Observer for FanoutObserver {
    fn record(&self, e: &ObsEvent<'_>) {
        for sink in &self.sinks {
            sink.record(e);
        }
    }
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-observ obs_port_tests -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-core/src/ids.rs crates/bridge-core/src/ports.rs crates/bridge-observ/Cargo.toml crates/bridge-observ/src/lib.rs && git commit -m "add core observability port"
```

### Task 2: Prometheus Adapter and Metric Catalog
**Files:** Modify `Cargo.toml:11-27`; modify `crates/bridge-observ/Cargo.toml:8-12`; modify/test `crates/bridge-observ/src/lib.rs:1-110`.
**Interfaces:** Consumes `ObsEvent`, `TurnContext`, `UsageSnapshot`. Produces `LabelVocabulary`, `TurnDedupe`, `PrometheusObserver::new(LabelVocabulary) -> Result<Self, prometheus::Error>`, `PrometheusObserver::endpoint(&self) -> MetricsEndpoint`, `MetricsEndpoint::render(&self) -> Result<String, prometheus::Error>`, `PrometheusObserver::drop_counter(&self) -> DropCounter`.
**Cohesion with Slices 2 & 3:** Metrics use only bounded labels; high-cardinality turn/task/prompt/trace ids stay out of Prometheus and remain available to Slice 2 through `turn_log`.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-observ/src/lib.rs
#[cfg(test)]
mod prometheus_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{
        ObsEvent, Observer, TurnContext, TurnOutcome, UsageFinalization,
    };
    use std::time::Duration;

    fn ctx(turn: &str, agent: &str, model: Option<&str>, effort: Option<&str>) -> TurnContext {
        TurnContext {
            turn_id: TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse("ctx-1").unwrap(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: agent.to_string(),
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
            mode: None,
            prompt_id: Some("prompt-a".to_string()),
            traceparent: None,
        }
    }

    #[test]
    fn prometheus_records_turn_latency_usage_queue_and_currency_rules() {
        let observer = PrometheusObserver::new(LabelVocabulary {
            agents: ["codex".to_string()].into_iter().collect(),
            models: ["gpt-5.5".to_string()].into_iter().collect(),
            efforts: ["high".to_string()].into_iter().collect(),
        })
        .unwrap();
        let c = ctx("turn-1", "codex", Some("gpt-5.5"), Some("high"));
        let ok = TurnOutcome::Success;
        observer.record(&ObsEvent::TurnStarted { ctx: &c });
        observer.record(&ObsEvent::QueueChanged {
            in_flight: 1,
            queued: 2,
            wait: Some(Duration::from_millis(250)),
        });
        observer.record(&ObsEvent::TurnFinished {
            ctx: &c,
            latency: Duration::from_millis(1500),
            ttft: Some(Duration::from_millis(100)),
            outcome: &ok,
        });
        let usage = UsageSnapshot {
            used: Some(10),
            size: Some(100),
            cost: Some(UsageCost {
                amount: 0.25,
                currency: "USD".to_string(),
            }),
            terminal: Some(TerminalUsage {
                total_tokens: 7,
                input_tokens: 3,
                output_tokens: 4,
                thought_tokens: Some(1),
                cached_read_tokens: Some(2),
                cached_write_tokens: Some(0),
            }),
            at_ms: 123,
        };
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c,
            usage: &usage,
            fin: UsageFinalization::TurnFinal,
        });

        let bad_currency = UsageSnapshot {
            cost: Some(UsageCost {
                amount: 99.0,
                currency: "ZZZ".to_string(),
            }),
            terminal: None,
            used: None,
            size: None,
            at_ms: 124,
        };
        let c2 = ctx("turn-2", "codex", Some("gpt-5.5"), Some("high"));
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c2,
            usage: &bad_currency,
            fin: UsageFinalization::TurnFinal,
        });

        let out = observer.endpoint().render().unwrap();
        assert!(out.contains("bridge_turns_total{agent=\"codex\",effort=\"high\",model=\"gpt-5.5\",outcome=\"success\"} 1"));
        assert!(out.contains("bridge_turn_duration_seconds_sum{agent=\"codex\",model=\"gpt-5.5\"} 1.5"));
        assert!(out.contains("bridge_turn_ttft_seconds_sum{agent=\"codex\"} 0.1"));
        assert!(out.contains("bridge_turns_in_flight 0"));
        assert!(out.contains("bridge_queue_depth 2"));
        assert!(out.contains("bridge_queue_wait_seconds_sum 0.25"));
        assert!(out.contains("bridge_turn_cost_total{agent=\"codex\",currency=\"USD\",model=\"gpt-5.5\"} 0.25"));
        assert!(out.contains("bridge_turn_cost_dropped_total{agent=\"codex\"} 1"));
        assert!(out.contains("bridge_turn_tokens_total{agent=\"codex\",kind=\"input\"} 3"));
        assert!(!out.contains("currency=\"ZZZ\""));
    }

    #[test]
    fn prometheus_dedupes_replayed_turn_ids_and_normalizes_unbounded_labels() {
        let observer = PrometheusObserver::new(LabelVocabulary::default()).unwrap();
        let c = ctx("turn-dup", "unbounded-user-value", Some("custom-model"), Some("weird"));
        let ok = TurnOutcome::Success;
        for _ in 0..2 {
            observer.record(&ObsEvent::TurnFinished {
                ctx: &c,
                latency: Duration::from_secs(1),
                ttft: None,
                outcome: &ok,
            });
        }
        let out = observer.endpoint().render().unwrap();
        assert!(out.contains("bridge_turns_total{agent=\"other\",effort=\"other\",model=\"other\",outcome=\"success\"} 1"));
    }
}
```
- [ ] Step: run it, expect FAIL with missing `PrometheusObserver`, `LabelVocabulary`, `MetricsEndpoint`
```bash
cargo test -p bridge-observ prometheus_tests -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```toml
# Cargo.toml [workspace.dependencies]
prometheus = "0.13"
```

```toml
# crates/bridge-observ/Cargo.toml
[dependencies]
bridge-core = { path = "../bridge-core" }
prometheus.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

```rust
// crates/bridge-observ/src/lib.rs, add
use bridge_core::orch::{TerminalUsage, UsageSnapshot};
use bridge_core::ports::{FailureClass, TurnOutcome, UsageFinalization};
use prometheus::{
    CounterVec, Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Clone, Debug, Default)]
pub struct LabelVocabulary {
    pub agents: HashSet<String>,
    pub models: HashSet<String>,
    pub efforts: HashSet<String>,
}

fn bounded_label(v: Option<&str>, allowed: &HashSet<String>) -> String {
    match v {
        Some(s) if allowed.contains(s) => s.to_string(),
        _ => "other".to_string(),
    }
}

fn outcome_label(outcome: &TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Success => "success",
        TurnOutcome::Canceled => "canceled",
        TurnOutcome::Failed(FailureClass::AgentCrashed) => "failed_agent_crashed",
        TurnOutcome::Failed(FailureClass::TimedOut) => "failed_timed_out",
        TurnOutcome::Failed(FailureClass::Overloaded) => "failed_overloaded",
        TurnOutcome::Failed(FailureClass::Config) => "failed_config",
        TurnOutcome::Failed(FailureClass::Transport) => "failed_transport",
        TurnOutcome::Failed(FailureClass::Other) => "failed_other",
    }
}

fn valid_iso4217(code: &str) -> bool {
    matches!(
        code,
        "USD" | "EUR" | "GBP" | "JPY" | "CAD" | "AUD" | "CHF" | "CNY" | "HKD" | "NZD"
            | "SEK" | "KRW" | "SGD" | "NOK" | "MXN" | "INR" | "BRL" | "ZAR" | "DKK"
            | "PLN" | "TWD" | "THB" | "MYR" | "IDR" | "CZK" | "HUF" | "ILS" | "CLP"
            | "PHP" | "AED" | "COP" | "SAR" | "RON" | "TRY"
    )
}

#[derive(Default)]
pub struct TurnDedupe {
    finished: Mutex<HashSet<String>>,
    usage: Mutex<HashSet<String>>,
}

impl TurnDedupe {
    pub fn mark_finished(&self, turn_id: &bridge_core::ids::TurnId) -> bool {
        self.finished
            .lock()
            .unwrap()
            .insert(turn_id.as_str().to_string())
    }

    pub fn mark_usage(&self, turn_id: &bridge_core::ids::TurnId) -> bool {
        self.usage
            .lock()
            .unwrap()
            .insert(turn_id.as_str().to_string())
    }

    pub fn seed(&self, turn_id: &bridge_core::ids::TurnId) {
        self.finished
            .lock()
            .unwrap()
            .insert(turn_id.as_str().to_string());
        self.usage
            .lock()
            .unwrap()
            .insert(turn_id.as_str().to_string());
    }
}

#[derive(Clone)]
pub struct DropCounter {
    counter: Option<IntCounterVec>,
}

impl DropCounter {
    fn new(counter: IntCounterVec) -> Self {
        Self {
            counter: Some(counter),
        }
    }

    pub fn disabled() -> Self {
        Self { counter: None }
    }

    pub fn observe(&self, sink: &str) {
        if let Some(counter) = &self.counter {
            counter.with_label_values(&[sink]).inc();
        }
    }
}

pub struct MetricsEndpoint {
    registry: Registry,
}

impl MetricsEndpoint {
    pub fn render(&self) -> Result<String, prometheus::Error> {
        let families = self.registry.gather();
        let mut buf = Vec::new();
        TextEncoder::new().encode(&families, &mut buf)?;
        Ok(String::from_utf8(buf).unwrap_or_default())
    }
}

pub struct PrometheusObserver {
    registry: Registry,
    vocab: LabelVocabulary,
    dedupe: Arc<TurnDedupe>,
    turns_total: CounterVec,
    turn_duration: HistogramVec,
    turn_ttft: HistogramVec,
    turns_in_flight: IntGauge,
    queue_depth: IntGauge,
    queue_wait: HistogramVec,
    cost_total: CounterVec,
    cost_dropped: IntCounterVec,
    tokens_total: IntCounterVec,
    dropped_total: IntCounterVec,
}

impl PrometheusObserver {
    pub fn new(vocab: LabelVocabulary) -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let turns_total = CounterVec::new(
            Opts::new("bridge_turns_total", "Completed turns by bounded dimensions"),
            &["agent", "model", "effort", "outcome"],
        )?;
        let buckets = vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];
        let turn_duration = HistogramVec::new(
            HistogramOpts::new("bridge_turn_duration_seconds", "Turn latency").buckets(buckets),
            &["agent", "model"],
        )?;
        let turn_ttft = HistogramVec::new(
            HistogramOpts::new("bridge_turn_ttft_seconds", "Turn time to first token"),
            &["agent"],
        )?;
        let turns_in_flight =
            IntGauge::new("bridge_turns_in_flight", "Currently running turns")?;
        let queue_depth = IntGauge::new("bridge_queue_depth", "Queued batch admissions")?;
        let queue_wait = HistogramVec::new(
            HistogramOpts::new("bridge_queue_wait_seconds", "Queue wait duration"),
            &[],
        )?;
        let cost_total = CounterVec::new(
            Opts::new("bridge_turn_cost_total", "Per-turn cost by currency"),
            &["agent", "model", "currency"],
        )?;
        let cost_dropped = IntCounterVec::new(
            Opts::new("bridge_turn_cost_dropped_total", "Costs dropped because currency was invalid"),
            &["agent"],
        )?;
        let tokens_total = IntCounterVec::new(
            Opts::new("bridge_turn_tokens_total", "Per-turn token totals"),
            &["agent", "kind"],
        )?;
        let dropped_total = IntCounterVec::new(
            Opts::new("bridge_observer_dropped_total", "Observer sink drops"),
            &["sink"],
        )?;

        for collector in [
            Box::new(turns_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(turn_duration.clone()),
            Box::new(turn_ttft.clone()),
            Box::new(turns_in_flight.clone()),
            Box::new(queue_depth.clone()),
            Box::new(queue_wait.clone()),
            Box::new(cost_total.clone()),
            Box::new(cost_dropped.clone()),
            Box::new(tokens_total.clone()),
            Box::new(dropped_total.clone()),
        ] {
            registry.register(collector)?;
        }

        Ok(Self {
            registry,
            vocab,
            dedupe: Arc::new(TurnDedupe::default()),
            turns_total,
            turn_duration,
            turn_ttft,
            turns_in_flight,
            queue_depth,
            queue_wait,
            cost_total,
            cost_dropped,
            tokens_total,
            dropped_total,
        })
    }

    pub fn endpoint(&self) -> MetricsEndpoint {
        MetricsEndpoint {
            registry: self.registry.clone(),
        }
    }

    pub fn dedupe(&self) -> Arc<TurnDedupe> {
        self.dedupe.clone()
    }

    pub fn drop_counter(&self) -> DropCounter {
        DropCounter::new(self.dropped_total.clone())
    }

    fn labels(&self, ctx: &bridge_core::ports::TurnContext) -> (String, String, String) {
        (
            bounded_label(Some(ctx.agent.as_str()), &self.vocab.agents),
            bounded_label(ctx.model.as_deref(), &self.vocab.models),
            bounded_label(ctx.effort.as_deref(), &self.vocab.efforts),
        )
    }

    fn add_tokens(&self, agent: &str, usage: &UsageSnapshot) {
        let Some(TerminalUsage {
            input_tokens,
            output_tokens,
            thought_tokens,
            cached_read_tokens,
            cached_write_tokens,
            ..
        }) = usage.terminal.as_ref()
        else {
            return;
        };
        for (kind, value) in [
            ("input", *input_tokens),
            ("output", *output_tokens),
            ("thought", thought_tokens.unwrap_or(0)),
            ("cached_read", cached_read_tokens.unwrap_or(0)),
            ("cached_write", cached_write_tokens.unwrap_or(0)),
        ] {
            if value > 0 {
                self.tokens_total.with_label_values(&[agent, kind]).inc_by(value);
            }
        }
    }
}

impl Observer for PrometheusObserver {
    fn record(&self, e: &ObsEvent<'_>) {
        match e {
            ObsEvent::TurnStarted { .. } => self.turns_in_flight.inc(),
            ObsEvent::TurnFinished {
                ctx,
                latency,
                ttft,
                outcome,
            } => {
                if !self.dedupe.mark_finished(&ctx.turn_id) {
                    return;
                }
                self.turns_in_flight.dec();
                let (agent, model, effort) = self.labels(ctx);
                self.turns_total
                    .with_label_values(&[&agent, &model, &effort, outcome_label(outcome)])
                    .inc();
                self.turn_duration
                    .with_label_values(&[&agent, &model])
                    .observe(latency.as_secs_f64());
                if let Some(ttft) = ttft {
                    self.turn_ttft
                        .with_label_values(&[&agent])
                        .observe(ttft.as_secs_f64());
                }
            }
            ObsEvent::QueueChanged {
                in_flight,
                queued,
                wait,
            } => {
                self.turns_in_flight.set(*in_flight as i64);
                self.queue_depth.set(*queued as i64);
                if let Some(wait) = wait {
                    self.queue_wait.with_label_values(&[]).observe(wait.as_secs_f64());
                }
            }
            ObsEvent::UsageFinalized { ctx, usage, fin } => {
                if *fin != UsageFinalization::TurnFinal || !self.dedupe.mark_usage(&ctx.turn_id) {
                    return;
                }
                let (agent, model, _) = self.labels(ctx);
                self.add_tokens(&agent, usage);
                match usage.cost.as_ref() {
                    Some(cost) if valid_iso4217(&cost.currency) => {
                        self.cost_total
                            .with_label_values(&[&agent, &model, &cost.currency])
                            .inc_by(cost.amount.max(0.0));
                    }
                    Some(_) => self.cost_dropped.with_label_values(&[&agent]).inc(),
                    None => {}
                }
            }
            ObsEvent::TaskStarted { .. }
            | ObsEvent::TaskFinished { .. }
            | ObsEvent::NodeStarted { .. }
            | ObsEvent::NodeFinished { .. } => {}
        }
    }
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-observ prometheus_tests -- --nocapture
```
- [ ] Step: commit
```bash
git add Cargo.toml crates/bridge-observ/Cargo.toml crates/bridge-observ/src/lib.rs && git commit -m "add prometheus observer"
```

### Task 3: Durable Turn-Log Store Schema and Store Port
**Files:** Modify/test `crates/bridge-core/src/task_store.rs:146-338, 431-480, 480-760`; modify/test `crates/bridge-store/src/sqlite.rs:117-168, 171-230, 447-620, 1379-2170`.
**Interfaces:** Consumes `TurnContext`, `TurnOutcome`, `UsageSnapshot`. Produces `TurnLogFinished`, `TurnLogUsage`, `TurnLogRow`, `TaskStore::upsert_turn_finished(&self, &TurnLogFinished)`, `TaskStore::update_turn_usage(&self, &TurnLogUsage)`, `TaskStore::turn_log_rows(&self)`.
**Cohesion with Slices 2 & 3:** Schema includes `traceparent`, eval dimensions, `turn_id` PK, `task_id`/`node`, and `completed_ms`; Slice 2 can read `GET /turns/{id}`, and Slice 3 can purge by `completed_ms` without touching resumable `tasks`.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-store/src/sqlite.rs
#[cfg(test)]
mod turn_log_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TaskId, TurnId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{FailureClass, TraceParent, TurnContext, TurnOutcome};
    use bridge_core::task_store::{TaskStore, TurnLogFinished, TurnLogUsage};
    use std::time::Duration;

    fn ctx(turn: &str, attempt: u32) -> TurnContext {
        TurnContext {
            turn_id: TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse("ctx-1").unwrap(),
            task_id: Some(TaskId::parse("task-1").unwrap()),
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

    #[tokio::test]
    async fn sqlite_turn_log_upserts_finished_then_usage_and_keeps_attempts_separate() {
        let store = SqliteStore::open_in_memory().unwrap();
        let first = TurnLogFinished {
            ctx: ctx("turn-a", 0),
            started_ms: 100,
            completed_ms: 250,
            latency: Duration::from_millis(150),
            ttft: Some(Duration::from_millis(12)),
            outcome: TurnOutcome::Failed(FailureClass::TimedOut),
        };
        store.upsert_turn_finished(&first).await.unwrap();
        store
            .update_turn_usage(&TurnLogUsage {
                ctx: first.ctx.clone(),
                usage: UsageSnapshot {
                    used: Some(50),
                    size: Some(1000),
                    cost: Some(UsageCost {
                        amount: 0.42,
                        currency: "USD".to_string(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: 12,
                        input_tokens: 5,
                        output_tokens: 7,
                        thought_tokens: Some(1),
                        cached_read_tokens: Some(2),
                        cached_write_tokens: Some(3),
                    }),
                    at_ms: 251,
                },
            })
            .await
            .unwrap();

        let retry = TurnLogFinished {
            ctx: ctx("turn-b", 1),
            started_ms: 300,
            completed_ms: 450,
            latency: Duration::from_millis(150),
            ttft: None,
            outcome: TurnOutcome::Success,
        };
        store.upsert_turn_finished(&retry).await.unwrap();

        let rows = store.turn_log_rows().await.unwrap();
        assert_eq!(rows.len(), 2);
        let row = rows.iter().find(|r| r.turn_id.as_str() == "turn-a").unwrap();
        assert_eq!(row.session_id.as_str(), "ctx-1");
        assert_eq!(row.task_id.as_ref().unwrap().as_str(), "task-1");
        assert_eq!(row.workflow.as_deref(), Some("code-review"));
        assert_eq!(row.node.as_deref(), Some("reviewer"));
        assert_eq!(row.attempt, 0);
        assert_eq!(row.outcome.as_deref(), Some("failed"));
        assert_eq!(row.failure_class.as_deref(), Some("timed_out"));
        assert_eq!(row.input_tokens, Some(5));
        assert_eq!(row.output_tokens, Some(7));
        assert_eq!(row.thought_tokens, Some(1));
        assert_eq!(row.cached_read_tokens, Some(2));
        assert_eq!(row.cached_write_tokens, Some(3));
        assert_eq!(row.cost_amount, Some(0.42));
        assert_eq!(row.cost_currency.as_deref(), Some("USD"));
        assert_eq!(
            row.traceparent.as_ref().unwrap().to_header_value(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        assert!(rows.iter().any(|r| r.turn_id.as_str() == "turn-b" && r.attempt == 1));
    }
}
```
- [ ] Step: run it, expect FAIL with missing turn-log DTOs/methods/table
```bash
cargo test -p bridge-store sqlite_turn_log_upserts_finished_then_usage_and_keeps_attempts_separate -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// crates/bridge-core/src/task_store.rs, add imports
use crate::ids::{BatchId, NodeId, OperationId, TaskId, TurnId, ContextId};

// crates/bridge-core/src/task_store.rs, add near TaskRecord
#[derive(Clone, Debug)]
pub struct TurnLogFinished {
    pub ctx: crate::ports::TurnContext,
    pub started_ms: i64,
    pub completed_ms: i64,
    pub latency: std::time::Duration,
    pub ttft: Option<std::time::Duration>,
    pub outcome: crate::ports::TurnOutcome,
}

#[derive(Clone, Debug)]
pub struct TurnLogUsage {
    pub ctx: crate::ports::TurnContext,
    pub usage: crate::orch::UsageSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnLogRow {
    pub turn_id: TurnId,
    pub session_id: ContextId,
    pub task_id: Option<TaskId>,
    pub workflow: Option<String>,
    pub node: Option<String>,
    pub attempt: u32,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub prompt_id: Option<String>,
    pub started_ms: Option<i64>,
    pub completed_ms: Option<i64>,
    pub latency_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
    pub outcome: Option<String>,
    pub failure_class: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub cost_amount: Option<f64>,
    pub cost_currency: Option<String>,
    pub traceparent: Option<crate::ports::TraceParent>,
}

// in TaskStore trait
async fn upsert_turn_finished(&self, _row: &TurnLogFinished) -> Result<(), BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn update_turn_usage(&self, _row: &TurnLogUsage) -> Result<(), BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn turn_log_rows(&self) -> Result<Vec<TurnLogRow>, BridgeError> {
    Ok(Vec::new())
}
```

```rust
// crates/bridge-core/src/task_store.rs, add field to MemoryTaskStore
turn_log: Mutex<HashMap<String, TurnLogRow>>,

// crates/bridge-core/src/task_store.rs, initialize in MemoryTaskStore::new()
turn_log: Mutex::new(HashMap::new()),

// crates/bridge-core/src/task_store.rs, add to impl TaskStore for MemoryTaskStore
async fn upsert_turn_finished(&self, row: &TurnLogFinished) -> Result<(), BridgeError> {
    let mut g = self.turn_log.lock().unwrap();
    let entry = g.entry(row.ctx.turn_id.as_str().to_string()).or_insert_with(|| TurnLogRow {
        turn_id: row.ctx.turn_id.clone(),
        session_id: row.ctx.session_id.clone(),
        task_id: row.ctx.task_id.clone(),
        workflow: row.ctx.workflow.clone(),
        node: row.ctx.node.clone(),
        attempt: row.ctx.attempt,
        agent: row.ctx.agent.clone(),
        model: row.ctx.model.clone(),
        effort: row.ctx.effort.clone(),
        mode: row.ctx.mode.clone(),
        prompt_id: row.ctx.prompt_id.clone(),
        started_ms: None,
        completed_ms: None,
        latency_ms: None,
        ttft_ms: None,
        outcome: None,
        failure_class: None,
        input_tokens: None,
        output_tokens: None,
        thought_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        cost_amount: None,
        cost_currency: None,
        traceparent: row.ctx.traceparent.clone(),
    });
    entry.started_ms = Some(row.started_ms);
    entry.completed_ms = Some(row.completed_ms);
    entry.latency_ms = Some(row.latency.as_millis() as u64);
    entry.ttft_ms = row.ttft.map(|d| d.as_millis() as u64);
    let (outcome, failure) = turn_log_outcome_strings(&row.outcome);
    entry.outcome = Some(outcome.to_string());
    entry.failure_class = failure.map(str::to_string);
    Ok(())
}

async fn update_turn_usage(&self, row: &TurnLogUsage) -> Result<(), BridgeError> {
    let mut g = self.turn_log.lock().unwrap();
    let entry = g.get_mut(row.ctx.turn_id.as_str()).ok_or(BridgeError::StoreFailure)?;
    if let Some(term) = row.usage.terminal.as_ref() {
        entry.input_tokens = Some(term.input_tokens);
        entry.output_tokens = Some(term.output_tokens);
        entry.thought_tokens = term.thought_tokens;
        entry.cached_read_tokens = term.cached_read_tokens;
        entry.cached_write_tokens = term.cached_write_tokens;
    }
    if let Some(cost) = row.usage.cost.as_ref() {
        entry.cost_amount = Some(cost.amount);
        entry.cost_currency = Some(cost.currency.clone());
    }
    Ok(())
}

async fn turn_log_rows(&self) -> Result<Vec<TurnLogRow>, BridgeError> {
    let mut rows: Vec<_> = self.turn_log.lock().unwrap().values().cloned().collect();
    rows.sort_by(|a, b| a.turn_id.as_str().cmp(b.turn_id.as_str()));
    Ok(rows)
}

// crates/bridge-core/src/task_store.rs, add helper
pub fn turn_log_outcome_strings(
    outcome: &crate::ports::TurnOutcome,
) -> (&'static str, Option<&'static str>) {
    use crate::ports::{FailureClass, TurnOutcome};
    match outcome {
        TurnOutcome::Success => ("success", None),
        TurnOutcome::Canceled => ("canceled", None),
        TurnOutcome::Failed(FailureClass::AgentCrashed) => ("failed", Some("agent_crashed")),
        TurnOutcome::Failed(FailureClass::TimedOut) => ("failed", Some("timed_out")),
        TurnOutcome::Failed(FailureClass::Overloaded) => ("failed", Some("overloaded")),
        TurnOutcome::Failed(FailureClass::Config) => ("failed", Some("config")),
        TurnOutcome::Failed(FailureClass::Transport) => ("failed", Some("transport")),
        TurnOutcome::Failed(FailureClass::Other) => ("failed", Some("other")),
    }
}
```

```rust
// crates/bridge-store/src/sqlite.rs, extend create_schema() execute_batch after task_journal
CREATE TABLE IF NOT EXISTS turn_log (
    turn_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    task_id TEXT,
    workflow TEXT,
    node TEXT,
    attempt INTEGER NOT NULL,
    agent TEXT NOT NULL,
    model TEXT,
    effort TEXT,
    mode TEXT,
    prompt_id TEXT,
    started_ms INTEGER,
    completed_ms INTEGER,
    latency_ms INTEGER,
    ttft_ms INTEGER,
    outcome TEXT,
    failure_class TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    thought_tokens INTEGER,
    cached_read_tokens INTEGER,
    cached_write_tokens INTEGER,
    cost_amount REAL,
    cost_currency TEXT,
    traceparent TEXT
);
CREATE INDEX IF NOT EXISTS idx_turn_log_completed ON turn_log(completed_ms);
CREATE INDEX IF NOT EXISTS idx_turn_log_task ON turn_log(task_id, node);
CREATE INDEX IF NOT EXISTS idx_turn_log_eval ON turn_log(prompt_id, model, effort);

// crates/bridge-store/src/sqlite.rs, add helpers
fn traceparent_to_string(tp: &Option<bridge_core::ports::TraceParent>) -> Option<String> {
    tp.as_ref().map(|t| t.to_header_value())
}

fn traceparent_from_string(raw: Option<String>) -> Option<bridge_core::ports::TraceParent> {
    raw.as_deref().and_then(bridge_core::ports::TraceParent::parse_header_value)
}

// crates/bridge-store/src/sqlite.rs, add to impl TaskStore for SqliteStore
async fn upsert_turn_finished(
    &self,
    row: &bridge_core::task_store::TurnLogFinished,
) -> Result<(), BridgeError> {
    let conn = self.conn.lock().unwrap();
    let (outcome, failure_class) = bridge_core::task_store::turn_log_outcome_strings(&row.outcome);
    conn.execute(
        "INSERT INTO turn_log(
            turn_id, session_id, task_id, workflow, node, attempt, agent, model, effort, mode,
            prompt_id, started_ms, completed_ms, latency_ms, ttft_ms, outcome, failure_class,
            traceparent
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(turn_id) DO UPDATE SET
            session_id=excluded.session_id,
            task_id=excluded.task_id,
            workflow=excluded.workflow,
            node=excluded.node,
            attempt=excluded.attempt,
            agent=excluded.agent,
            model=excluded.model,
            effort=excluded.effort,
            mode=excluded.mode,
            prompt_id=excluded.prompt_id,
            started_ms=excluded.started_ms,
            completed_ms=excluded.completed_ms,
            latency_ms=excluded.latency_ms,
            ttft_ms=excluded.ttft_ms,
            outcome=excluded.outcome,
            failure_class=excluded.failure_class,
            traceparent=excluded.traceparent",
        rusqlite::params![
            row.ctx.turn_id.as_str(),
            row.ctx.session_id.as_str(),
            row.ctx.task_id.as_ref().map(|t| t.as_str()),
            row.ctx.workflow,
            row.ctx.node,
            row.ctx.attempt as i64,
            row.ctx.agent,
            row.ctx.model,
            row.ctx.effort,
            row.ctx.mode,
            row.ctx.prompt_id,
            row.started_ms,
            row.completed_ms,
            row.latency.as_millis() as i64,
            row.ttft.map(|d| d.as_millis() as i64),
            outcome,
            failure_class,
            traceparent_to_string(&row.ctx.traceparent),
        ],
    )
    .map_err(|_| BridgeError::StoreFailure)?;
    Ok(())
}

async fn update_turn_usage(
    &self,
    row: &bridge_core::task_store::TurnLogUsage,
) -> Result<(), BridgeError> {
    let conn = self.conn.lock().unwrap();
    let term = row.usage.terminal.as_ref();
    let cost = row.usage.cost.as_ref();
    conn.execute(
        "UPDATE turn_log SET
            input_tokens=?2,
            output_tokens=?3,
            thought_tokens=?4,
            cached_read_tokens=?5,
            cached_write_tokens=?6,
            cost_amount=?7,
            cost_currency=?8
         WHERE turn_id=?1",
        rusqlite::params![
            row.ctx.turn_id.as_str(),
            term.map(|t| t.input_tokens as i64),
            term.map(|t| t.output_tokens as i64),
            term.and_then(|t| t.thought_tokens).map(|v| v as i64),
            term.and_then(|t| t.cached_read_tokens).map(|v| v as i64),
            term.and_then(|t| t.cached_write_tokens).map(|v| v as i64),
            cost.map(|c| c.amount),
            cost.map(|c| c.currency.as_str()),
        ],
    )
    .map_err(|_| BridgeError::StoreFailure)?;
    Ok(())
}

async fn turn_log_rows(&self) -> Result<Vec<bridge_core::task_store::TurnLogRow>, BridgeError> {
    let conn = self.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT turn_id, session_id, task_id, workflow, node, attempt, agent, model, effort, mode,
                    prompt_id, started_ms, completed_ms, latency_ms, ttft_ms, outcome, failure_class,
                    input_tokens, output_tokens, thought_tokens, cached_read_tokens, cached_write_tokens,
                    cost_amount, cost_currency, traceparent
             FROM turn_log ORDER BY turn_id",
        )
        .map_err(|_| BridgeError::StoreFailure)?;
    let rows = stmt
        .query_map([], |row| {
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
        })
        .map_err(|_| BridgeError::StoreFailure)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| BridgeError::StoreFailure)?;
    Ok(rows)
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-store sqlite_turn_log_upserts_finished_then_usage_and_keeps_attempts_separate -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-core/src/task_store.rs crates/bridge-store/src/sqlite.rs && git commit -m "add turn log store"
```

### Task 4: Async TurnLogObserver Sink
**Files:** Modify/test `crates/bridge-observ/Cargo.toml:8-12`; modify/test `crates/bridge-observ/src/lib.rs:1-260`.
**Interfaces:** Consumes `Arc<dyn TaskStore>`, `TurnDedupe`, `DropCounter`. Produces `TurnLogObserver::new(store, dedupe, drop_counter, capacity, now_ms) -> Self`, `TurnLogObserver::flush(&self) -> impl Future<Output = ()>`.
**Cohesion with Slices 2 & 3:** Writes are single-row upserts/updates through `TaskStore`; later retention can delete completed rows by `completed_ms` in its own short transaction without sharing writer state.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-observ/src/lib.rs
#[cfg(test)]
mod turn_log_observer_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{ObsEvent, Observer, TurnContext, TurnOutcome, UsageFinalization};
    use bridge_core::task_store::{MemoryTaskStore, TaskStore};
    use std::sync::Arc;
    use std::time::Duration;

    fn ctx(turn: &str) -> TurnContext {
        TurnContext {
            turn_id: TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse("ctx-log").unwrap(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: None,
            prompt_id: Some("prompt/log".to_string()),
            traceparent: None,
        }
    }

    #[tokio::test]
    async fn turn_log_observer_writes_finished_then_usage_off_path() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let observer = TurnLogObserver::new(
            store.clone(),
            Arc::new(TurnDedupe::default()),
            DropCounter::disabled(),
            8,
            Arc::new(|| 10_000),
        );
        let c = ctx("turn-log-1");
        let outcome = TurnOutcome::Success;
        observer.record(&ObsEvent::TurnFinished {
            ctx: &c,
            latency: Duration::from_millis(250),
            ttft: Some(Duration::from_millis(50)),
            outcome: &outcome,
        });
        let usage = UsageSnapshot {
            used: Some(1),
            size: Some(2),
            cost: Some(UsageCost {
                amount: 0.12,
                currency: "USD".to_string(),
            }),
            terminal: Some(TerminalUsage {
                total_tokens: 3,
                input_tokens: 1,
                output_tokens: 2,
                thought_tokens: None,
                cached_read_tokens: None,
                cached_write_tokens: None,
            }),
            at_ms: 10_001,
        };
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c,
            usage: &usage,
            fin: UsageFinalization::TurnFinal,
        });
        observer.flush().await;

        let rows = store.turn_log_rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].turn_id.as_str(), "turn-log-1");
        assert_eq!(rows[0].started_ms, Some(9_750));
        assert_eq!(rows[0].completed_ms, Some(10_000));
        assert_eq!(rows[0].latency_ms, Some(250));
        assert_eq!(rows[0].ttft_ms, Some(50));
        assert_eq!(rows[0].input_tokens, Some(1));
        assert_eq!(rows[0].cost_currency.as_deref(), Some("USD"));
    }

    #[tokio::test]
    async fn turn_log_observer_drops_when_queue_full_without_blocking() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let prom = PrometheusObserver::new(LabelVocabulary::default()).unwrap();
        let observer = TurnLogObserver::new(
            store,
            Arc::new(TurnDedupe::default()),
            prom.drop_counter(),
            0,
            Arc::new(|| 1),
        );
        let c = ctx("turn-log-drop");
        let outcome = TurnOutcome::Success;
        observer.record(&ObsEvent::TurnFinished {
            ctx: &c,
            latency: Duration::from_millis(1),
            ttft: None,
            outcome: &outcome,
        });
        let out = prom.endpoint().render().unwrap();
        assert!(out.contains("bridge_observer_dropped_total{sink=\"turn_log\"} 1"));
    }
}
```
- [ ] Step: run it, expect FAIL with missing `TurnLogObserver`
```bash
cargo test -p bridge-observ turn_log_observer_tests -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```toml
# crates/bridge-observ/Cargo.toml
tokio = { workspace = true }
```

```rust
// crates/bridge-observ/src/lib.rs, add
use bridge_core::task_store::{TaskStore, TurnLogFinished, TurnLogUsage};
use tokio::sync::{mpsc, oneshot};

type NowMs = Arc<dyn Fn() -> i64 + Send + Sync>;

enum TurnLogCommand {
    Finished(TurnLogFinished),
    Usage(TurnLogUsage),
    Flush(oneshot::Sender<()>),
}

pub struct TurnLogObserver {
    tx: mpsc::Sender<TurnLogCommand>,
    dedupe: Arc<TurnDedupe>,
    dropped: DropCounter,
    now_ms: NowMs,
}

impl TurnLogObserver {
    pub fn new(
        store: Arc<dyn TaskStore>,
        dedupe: Arc<TurnDedupe>,
        dropped: DropCounter,
        capacity: usize,
        now_ms: NowMs,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel(capacity);
        let dropped_for_task = dropped.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    TurnLogCommand::Finished(row) => {
                        if store.upsert_turn_finished(&row).await.is_err() {
                            dropped_for_task.observe("turn_log");
                        }
                    }
                    TurnLogCommand::Usage(row) => {
                        if store.update_turn_usage(&row).await.is_err() {
                            dropped_for_task.observe("turn_log");
                        }
                    }
                    TurnLogCommand::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        Self {
            tx,
            dedupe,
            dropped,
            now_ms,
        }
    }

    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.try_send(TurnLogCommand::Flush(tx)).is_ok() {
            let _ = rx.await;
        }
    }

    fn try_send(&self, cmd: TurnLogCommand) {
        if self.tx.try_send(cmd).is_err() {
            self.dropped.observe("turn_log");
        }
    }
}

impl Observer for TurnLogObserver {
    fn record(&self, e: &ObsEvent<'_>) {
        match e {
            ObsEvent::TurnFinished {
                ctx,
                latency,
                ttft,
                outcome,
            } => {
                if !self.dedupe.mark_finished(&ctx.turn_id) {
                    return;
                }
                let completed_ms = (self.now_ms)();
                let latency_ms = latency.as_millis() as i64;
                self.try_send(TurnLogCommand::Finished(TurnLogFinished {
                    ctx: (*ctx).clone(),
                    started_ms: completed_ms.saturating_sub(latency_ms),
                    completed_ms,
                    latency: *latency,
                    ttft: *ttft,
                    outcome: (*outcome).clone(),
                }));
            }
            ObsEvent::UsageFinalized { ctx, usage, fin } => {
                if *fin != UsageFinalization::TurnFinal || !self.dedupe.mark_usage(&ctx.turn_id) {
                    return;
                }
                self.try_send(TurnLogCommand::Usage(TurnLogUsage {
                    ctx: (*ctx).clone(),
                    usage: (*usage).clone(),
                }));
            }
            ObsEvent::TaskStarted { .. }
            | ObsEvent::TaskFinished { .. }
            | ObsEvent::NodeStarted { .. }
            | ObsEvent::NodeFinished { .. }
            | ObsEvent::TurnStarted { .. }
            | ObsEvent::QueueChanged { .. } => {}
        }
    }
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-observ turn_log_observer_tests -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-observ/Cargo.toml crates/bridge-observ/src/lib.rs && git commit -m "add async turn log observer"
```

### Task 5: Coordinator Turn Boundary Hooks
**Files:** Modify/test `crates/bridge-coordinator/src/session_manager.rs:107-117, 404-433, 493-526, 599-620, 772-795`; modify `crates/bridge-coordinator/src/coordinator.rs:5-13, 95-112, 127-160, 356-448`; modify `crates/bridge-coordinator/src/lib.rs:1-12`.
**Interfaces:** Consumes `Arc<dyn Observer>`. Produces `Coordinator::new(..., observer: Arc<dyn Observer>, ...) -> Self`, `Coordinator::observer(&self) -> Arc<dyn Observer>`, internal `classify_error(&BridgeError) -> FailureClass`, internal turn emission at `collect_turn`.
**Cohesion with Slices 2 & 3:** `TurnContext` carries `task_id`, eval dimensions, and future trace parent; the coordinator hook mints one `turn_id` per attempt so Slice 2 rows and Slice 3 purge accounting are per paid attempt.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-coordinator/src/coordinator.rs
#[cfg(test)]
mod observability_boundary_tests {
    use super::*;
    use bridge_core::domain::{AgentEntry, AgentKind, Effort, Part};
    use bridge_core::ids::{AgentId, ContextId, SessionId};
    use bridge_core::orch::{TerminalUsage, UsageSnapshot};
    use bridge_core::ports::{
        AgentBackend, AgentRegistry, BackendStream, Lease, ObsEvent, Observer, Resolved,
    };
    use bridge_core::task_store::{MemoryTaskStore, TaskStore};
    use futures::stream;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<String>>);

    impl Observer for RecordingObserver {
        fn record(&self, e: &ObsEvent<'_>) {
            let mut g = self.0.lock().unwrap();
            match e {
                ObsEvent::TurnStarted { ctx } => g.push(format!("start:{}", ctx.turn_id.as_str())),
                ObsEvent::TurnFinished { ctx, outcome, .. } => {
                    g.push(format!("finish:{}:{outcome:?}", ctx.turn_id.as_str()))
                }
                ObsEvent::UsageFinalized { ctx, usage, .. } => {
                    g.push(format!(
                        "usage:{}:{}",
                        ctx.turn_id.as_str(),
                        usage.terminal.as_ref().unwrap().input_tokens
                    ))
                }
                _ => {}
            }
        }
    }

    struct NoopLease;
    impl Lease for NoopLease {}

    struct FakeRegistry {
        backend: Arc<dyn AgentBackend>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: AgentId::parse("codex").unwrap(),
                    cmd: Some("fake".to_string()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: Some("gpt-5.5".to_string()),
                    effort: Some(Effort::High),
                    mode: Some("default".to_string()),
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    auth_method: None,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    extensions: Default::default(),
                }),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _snapshot: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![self.default_id()]
        }
    }

    struct UsageBackend;

    #[async_trait::async_trait]
    impl AgentBackend for UsageBackend {
        async fn prompt(&self, _session: &SessionId, _parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(stream::iter(vec![
                Ok(bridge_core::ports::Update::Usage(UsageSnapshot {
                    used: Some(3),
                    size: Some(10),
                    cost: None,
                    terminal: Some(TerminalUsage {
                        total_tokens: 5,
                        input_tokens: 2,
                        output_tokens: 3,
                        thought_tokens: None,
                        cached_read_tokens: None,
                        cached_write_tokens: None,
                    }),
                    at_ms: 0,
                })),
                Ok(bridge_core::ports::Update::Text("hello".to_string())),
                Ok(bridge_core::ports::Update::Done { stop_reason: "end_turn".to_string() }),
            ])))
        }
        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn coordinator_collect_turn_emits_started_finished_and_usage_once() {
        let observer = Arc::new(RecordingObserver::default());
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            backend: Arc::new(UsageBackend),
        });
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry.clone(),
            std::time::Duration::from_secs(60),
        ));
        let task_store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let session_store: Arc<dyn bridge_core::ports::SessionStore> =
            Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let coord = Coordinator::new(
            sm,
            None,
            Arc::new(std::collections::HashMap::new()),
            task_store,
            session_store,
            Arc::new(crate::tests::AllowPolicy),
            registry,
            Arc::new(crate::clock::SystemClock),
            None,
            None,
            3,
            observer.clone(),
        );

        let out = coord
            .prompt(OpParams {
                input: "hi".to_string(),
                context: Some(ContextId::parse("ctx-obs").unwrap()),
                agent: Some(AgentId::parse("codex").unwrap()),
                model: None,
                effort: None,
                mode: None,
                cwd: None,
            })
            .await
            .unwrap();

        assert_eq!(out.text, "hello");
        let events = observer.0.lock().unwrap().clone();
        assert_eq!(events.iter().filter(|e| e.starts_with("start:")).count(), 1);
        assert_eq!(events.iter().filter(|e| e.starts_with("finish:")).count(), 1);
        assert_eq!(events.iter().filter(|e| e.starts_with("usage:")).count(), 1);
    }
}
```
- [ ] Step: run it, expect FAIL with old `Coordinator::new` signature and no observer events
```bash
cargo test -p bridge-coordinator coordinator_collect_turn_emits_started_finished_and_usage_once -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// crates/bridge-coordinator/src/session_manager.rs, extend WarmTurn
pub struct WarmTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub usage_warning: Option<UsageWarning>,
    pub generation: SessionGeneration,
    pub op: OperationId,
    pub abort: CancellationToken,
    pub seed: Option<String>,
    pub injects: Vec<QueuedInject>,
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
}

// add helper in impl SessionManager
fn warm_turn_from_handle(h: &mut WarmHandle, usage_warning: Option<UsageWarning>, op: OperationId, abort: CancellationToken) -> WarmTurn {
    WarmTurn {
        backend: h.backend.clone(),
        session: h.backend_session.clone(),
        usage_warning,
        generation: h.generation,
        op,
        abort,
        seed: h.pending_seed.take(),
        injects: std::mem::take(&mut h.pending_injects),
        agent: h.agent.clone(),
        model: h.fingerprint.config.model.clone(),
        effort: h.fingerprint.config.effort.as_ref().map(|e| e.to_string()),
        mode: h.fingerprint.config.mode.clone(),
    }
}
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, imports
use bridge_core::ports::{
    AgentRegistry, FailureClass, ObsEvent, Observer, PolicyEngine, SessionStore, TurnContext,
    TurnOutcome, UsageFinalization,
};
use std::time::Instant;

// Coordinator field
observer: Arc<dyn Observer>,

// Coordinator::new arg and initializer, append before resume_attempt_cap
observer: Arc<dyn Observer>,

// in Self initializer
observer,

// accessor
pub fn observer(&self) -> Arc<dyn Observer> {
    self.observer.clone()
}

fn classify_error(e: &BridgeError) -> FailureClass {
    match e {
        BridgeError::AgentCrashed { .. } => FailureClass::AgentCrashed,
        BridgeError::AgentTimedOut | BridgeError::CancelTimeout => FailureClass::TimedOut,
        BridgeError::AgentOverloaded => FailureClass::Overloaded,
        BridgeError::ConfigMismatch { .. }
        | BridgeError::ConfigReseedRequired { .. }
        | BridgeError::ConfigInvalid { .. }
        | BridgeError::UnknownAgent { .. }
        | BridgeError::ModelNotAvailable => FailureClass::Config,
        BridgeError::FrameError | BridgeError::UpstreamA2aError => FailureClass::Transport,
        _ => FailureClass::Other,
    }
}

fn new_turn_id() -> bridge_core::ids::TurnId {
    bridge_core::ids::TurnId::parse(format!("turn-{}", a2a::new_task_id()))
        .expect("a2a task id is non-empty")
}

fn turn_context_for_warm(
    ctx: &ContextId,
    task: Option<TaskId>,
    turn: &crate::session_manager::WarmTurn,
) -> TurnContext {
    TurnContext {
        turn_id: new_turn_id(),
        session_id: ctx.clone(),
        task_id: task,
        workflow: None,
        node: None,
        attempt: 0,
        agent: turn.agent.as_str().to_string(),
        model: turn.model.clone(),
        effort: turn.effort.clone(),
        mode: turn.mode.clone(),
        prompt_id: None,
        traceparent: None,
    }
}
```

```rust
// crates/bridge-coordinator/src/coordinator.rs, inside collect_turn before translator.run
let obs_ctx = turn_context_for_warm(&ctx, Some(task.clone()), &turn);
let started = Instant::now();
let mut ttft = None;
let mut last_usage: Option<UsageSnapshot> = None;
self.observer.record(&ObsEvent::TurnStarted { ctx: &obs_ctx });

// in loop, usage branch additionally set last_usage
last_usage = Some(snap.clone());

// after ev is obtained and before pushing, stamp ttft
if ttft.is_none() {
    ttft = Some(started.elapsed());
}

// after error check, before return Err
if let Some(Err(e)) = collected.iter().find(|r| r.is_err()) {
    let outcome = TurnOutcome::Failed(classify_error(e));
    self.observer.record(&ObsEvent::TurnFinished {
        ctx: &obs_ctx,
        latency: started.elapsed(),
        ttft,
        outcome: &outcome,
    });
    if let Some(usage) = &last_usage {
        self.observer.record(&ObsEvent::UsageFinalized {
            ctx: &obs_ctx,
            usage,
            fin: UsageFinalization::TurnFinal,
        });
    }
    return Err(e.clone());
}

// before final Ok(TurnOutput...)
let outcome = events
    .iter()
    .rev()
    .find_map(|e| (e.kind() == &EventKind::Terminal).then(|| e.outcome()).flatten())
    .map(|o| match o {
        TaskOutcome::Completed => TurnOutcome::Success,
        TaskOutcome::Failed => TurnOutcome::Failed(FailureClass::Other),
        TaskOutcome::Canceled => TurnOutcome::Canceled,
    })
    .unwrap_or(TurnOutcome::Success);
self.observer.record(&ObsEvent::TurnFinished {
    ctx: &obs_ctx,
    latency: started.elapsed(),
    ttft,
    outcome: &outcome,
});
if let Some(usage) = &last_usage {
    self.observer.record(&ObsEvent::UsageFinalized {
        ctx: &obs_ctx,
        usage,
        fin: UsageFinalization::TurnFinal,
    });
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-coordinator coordinator_collect_turn_emits_started_finished_and_usage_once -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-coordinator/src/session_manager.rs crates/bridge-coordinator/src/coordinator.rs crates/bridge-coordinator/src/lib.rs && git commit -m "emit observer events at coordinator turn boundary"
```

### Task 6: Inbound Traceparent and Local Producer Hooks
**Files:** Modify/test `crates/bridge-coordinator/src/dispatch.rs:72-89`; modify/test `crates/bridge-a2a-inbound/src/server.rs:85-120, 250-338, 408-487, 489-537, 1368-1465, 2270-2375, 3402-3409, 3490-3553`.
**Interfaces:** Consumes `InboundServer::coordinator().observer()`, `TraceParent::parse_header_value`. Produces `RoutedCall.traceparent`, `RoutedCall.prompt_id`, `LocalDispatch.obs_ctx: TurnContext`.
**Cohesion with Slices 2 & 3:** Valid inbound W3C `traceparent` is persisted into the turn context and child node contexts inherit it in later workflow wiring; invalid headers become `None`, so Slice 2 can display trace refs without fabricated IDs.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-a2a-inbound/src/server.rs
#[cfg(test)]
mod inbound_observability_tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use bridge_core::ports::TraceParent;
    use serde_json::json;

    #[test]
    fn gate_parses_valid_traceparent_and_prompt_id() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        let params = json!({
            "message": {
                "text": "hello",
                "metadata": {
                    "a2a-bridge.prompt_id": "eval/prompt-a"
                }
            }
        });
        let routed = srv.gate(&headers, &params).unwrap();
        assert_eq!(routed.prompt_id.as_deref(), Some("eval/prompt-a"));
        assert_eq!(
            routed.traceparent.unwrap().to_header_value(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
    }

    #[test]
    fn gate_ignores_malformed_traceparent() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static("bad"));
        let params = json!({"message": {"text": "hello"}});
        let routed = srv.gate(&headers, &params).unwrap();
        assert_eq!(routed.traceparent, None);
    }
}
```
- [ ] Step: run it, expect FAIL with missing `traceparent` and `prompt_id` fields
```bash
cargo test -p bridge-a2a-inbound inbound_observability_tests -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// crates/bridge-coordinator/src/dispatch.rs, add field to LocalDispatch
pub obs_ctx: bridge_core::ports::TurnContext,
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, add to RoutedCall
traceparent: Option<bridge_core::ports::TraceParent>,
prompt_id: Option<String>,

// in gate(), before Ok(RoutedCall)
let traceparent = headers
    .get("traceparent")
    .and_then(|v| v.to_str().ok())
    .and_then(bridge_core::ports::TraceParent::parse_header_value);
let prompt_id = params
    .get("message")
    .and_then(|m| m.get("metadata"))
    .and_then(|md| md.get("a2a-bridge.prompt_id"))
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

// include in RoutedCall initializer
traceparent,
prompt_id,
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, add helpers
fn mint_turn_id() -> bridge_core::ids::TurnId {
    bridge_core::ids::TurnId::parse(format!("turn-{}", a2a::new_task_id()))
        .expect("a2a task id is non-empty")
}

fn routed_session_context_id(routed: &RoutedCall) -> ContextId {
    routed
        .context_id
        .clone()
        .unwrap_or_else(|| ContextId::parse(routed.task.as_str()).expect("task id is non-empty"))
}

fn obs_ctx_for_dispatch(
    routed: &RoutedCall,
    agent: &AgentId,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
) -> bridge_core::ports::TurnContext {
    bridge_core::ports::TurnContext {
        turn_id: mint_turn_id(),
        session_id: routed_session_context_id(routed),
        task_id: Some(routed.task.clone()),
        workflow: None,
        node: None,
        attempt: 0,
        agent: agent.as_str().to_string(),
        model,
        effort,
        mode,
        prompt_id: routed.prompt_id.clone(),
        traceparent: routed.traceparent.clone(),
    }
}
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, change resolve_configure_bind signature
async fn resolve_configure_bind(
    srv: &InboundServer,
    agent_id: &AgentId,
    routed: &RoutedCall,
    session: &SessionId,
    overrides: Option<&AgentOverride>,
    session_cwd: Option<SessionCwd>,
) -> Result<LocalDispatch, BridgeError>

// follow-up return LocalDispatch initializer
obs_ctx: obs_ctx_for_dispatch(
    routed,
    agent_id,
    eff.model.clone(),
    eff.effort.as_ref().map(|e| e.to_string()),
    eff.mode.clone(),
),

// first-message return LocalDispatch initializer
obs_ctx: obs_ctx_for_dispatch(
    routed,
    agent_id,
    eff.model.clone(),
    eff.effort.as_ref().map(|e| e.to_string()),
    eff.mode.clone(),
),
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, warm_local_dispatch LocalDispatch initializer
obs_ctx: bridge_core::ports::TurnContext {
    turn_id: mint_turn_id(),
    session_id: ctx.clone(),
    task_id: Some(routed.task.clone()),
    workflow: None,
    node: None,
    attempt: 0,
    agent: turn.agent.as_str().to_string(),
    model: turn.model.clone(),
    effort: turn.effort.clone(),
    mode: turn.mode.clone(),
    prompt_id: routed.prompt_id.clone(),
    traceparent: routed.traceparent.clone(),
},
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, update resolve_configure_bind call sites
resolve_configure_bind(
    &srv,
    &agent_id,
    &routed,
    &routed.session,
    routed.overrides.as_ref(),
    routed.session_cwd.clone(),
).await

resolve_configure_bind(
    &srv,
    agent_id,
    &routed,
    &routed.session,
    routed.overrides.as_ref(),
    routed.session_cwd.clone(),
).await
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, in spawn_local_producer before tokio::spawn
let observer = srv.coordinator().observer();
let obs_ctx = dispatch.obs_ctx.clone();

// inside task, before translator.run
let started = std::time::Instant::now();
let mut ttft = None;
let mut last_usage: Option<bridge_core::orch::UsageSnapshot> = None;
observer.record(&bridge_core::ports::ObsEvent::TurnStarted { ctx: &obs_ctx });

// in event loop usage branch
last_usage = Some(snap.clone());

// when a non-usage event arrives
if ttft.is_none() {
    ttft = Some(started.elapsed());
}

// on abort arm before return
let outcome = bridge_core::ports::TurnOutcome::Canceled;
observer.record(&bridge_core::ports::ObsEvent::TurnFinished {
    ctx: &obs_ctx,
    latency: started.elapsed(),
    ttft,
    outcome: &outcome,
});
if let Some(usage) = &last_usage {
    observer.record(&bridge_core::ports::ObsEvent::UsageFinalized {
        ctx: &obs_ctx,
        usage,
        fin: bridge_core::ports::UsageFinalization::TurnFinal,
    });
}

// after loop before producer exits
let outcome = if translator_terminal {
    bridge_core::ports::TurnOutcome::Canceled
} else if errored {
    bridge_core::ports::TurnOutcome::Failed(bridge_core::ports::FailureClass::Other)
} else {
    bridge_core::ports::TurnOutcome::Success
};
observer.record(&bridge_core::ports::ObsEvent::TurnFinished {
    ctx: &obs_ctx,
    latency: started.elapsed(),
    ttft,
    outcome: &outcome,
});
if let Some(usage) = &last_usage {
    observer.record(&bridge_core::ports::ObsEvent::UsageFinalized {
        ctx: &obs_ctx,
        usage,
        fin: bridge_core::ports::UsageFinalization::TurnFinal,
    });
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-a2a-inbound inbound_observability_tests -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-coordinator/src/dispatch.rs crates/bridge-a2a-inbound/src/server.rs && git commit -m "wire inbound turn observations"
```

### Task 7: Queue RAII Guard for Batch Admission
**Files:** Modify/test `crates/bridge-coordinator/src/batch.rs:24-41, 571-614, 741-760`.
**Interfaces:** Consumes `Arc<dyn Observer>`. Produces `BatchRuntime::new(max_concurrent, default_concurrency, observer)`, `QueueAdmissionGuard`.
**Cohesion with Slices 2 & 3:** Queue metrics remain aggregate only; no task IDs become labels, and no durable storage is introduced for Slice 3 retention.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-coordinator/src/batch.rs
#[cfg(test)]
mod queue_observability_tests {
    use super::*;
    use bridge_core::ports::{ObsEvent, Observer};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct QueueRecorder(Mutex<Vec<(u64, u64, bool)>>);

    impl Observer for QueueRecorder {
        fn record(&self, e: &ObsEvent<'_>) {
            if let ObsEvent::QueueChanged {
                in_flight,
                queued,
                wait,
            } = e
            {
                self.0.lock().unwrap().push((*in_flight, *queued, wait.is_some()));
            }
        }
    }

    #[tokio::test]
    async fn waiting_guard_drop_restores_queue_depth_on_cancel() {
        let observer = Arc::new(QueueRecorder::default());
        let runtime = BatchRuntime::new(0, 1, observer.clone());
        {
            let _guard = QueueAdmissionGuard::waiting(&runtime);
        }
        let events = observer.0.lock().unwrap().clone();
        assert_eq!(events, vec![(0, 1, false), (0, 0, false)]);
    }

    #[tokio::test]
    async fn admitted_guard_observes_wait_and_releases_inflight_on_drop() {
        let observer = Arc::new(QueueRecorder::default());
        let runtime = BatchRuntime::new(1, 1, observer.clone());
        {
            let mut guard = QueueAdmissionGuard::waiting(&runtime);
            guard.admitted();
        }
        let events = observer.0.lock().unwrap().clone();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], (0, 1, false));
        assert_eq!(events[1].0, 1);
        assert_eq!(events[1].1, 0);
        assert!(events[1].2);
        assert_eq!(events[2], (0, 0, false));
    }
}
```
- [ ] Step: run it, expect FAIL with missing observer field and guard
```bash
cargo test -p bridge-coordinator queue_observability_tests -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// crates/bridge-coordinator/src/batch.rs, imports
use bridge_core::ports::{ObsEvent, Observer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// BatchRuntime fields
pub observer: Arc<dyn Observer>,
pub queued: Arc<AtomicU64>,
pub in_flight: Arc<AtomicU64>,

// BatchRuntime::new signature/body
pub fn new(max_concurrent: u32, default_concurrency: u32, observer: Arc<dyn Observer>) -> Self {
    Self {
        semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
        default_concurrency,
        max_concurrent,
        batch_cancels: Arc::new(Mutex::new(HashMap::new())),
        observer,
        queued: Arc::new(AtomicU64::new(0)),
        in_flight: Arc::new(AtomicU64::new(0)),
    }
}

enum QueueState {
    Waiting,
    Admitted,
    Released,
}

pub struct QueueAdmissionGuard {
    observer: Arc<dyn Observer>,
    queued: Arc<AtomicU64>,
    in_flight: Arc<AtomicU64>,
    started: Instant,
    state: QueueState,
}

impl QueueAdmissionGuard {
    pub fn waiting(runtime: &BatchRuntime) -> Self {
        let queued = runtime.queued.fetch_add(1, Ordering::SeqCst) + 1;
        let in_flight = runtime.in_flight.load(Ordering::SeqCst);
        runtime.observer.record(&ObsEvent::QueueChanged {
            in_flight,
            queued,
            wait: None,
        });
        Self {
            observer: runtime.observer.clone(),
            queued: runtime.queued.clone(),
            in_flight: runtime.in_flight.clone(),
            started: Instant::now(),
            state: QueueState::Waiting,
        }
    }

    pub fn admitted(&mut self) {
        if matches!(self.state, QueueState::Waiting) {
            let queued = self.queued.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
            let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.state = QueueState::Admitted;
            self.observer.record(&ObsEvent::QueueChanged {
                in_flight,
                queued,
                wait: Some(self.started.elapsed()),
            });
        }
    }
}

impl Drop for QueueAdmissionGuard {
    fn drop(&mut self) {
        match self.state {
            QueueState::Waiting => {
                let queued = self.queued.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
                let in_flight = self.in_flight.load(Ordering::SeqCst);
                self.state = QueueState::Released;
                self.observer.record(&ObsEvent::QueueChanged {
                    in_flight,
                    queued,
                    wait: None,
                });
            }
            QueueState::Admitted => {
                let in_flight = self.in_flight.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
                let queued = self.queued.load(Ordering::SeqCst);
                self.state = QueueState::Released;
                self.observer.record(&ObsEvent::QueueChanged {
                    in_flight,
                    queued,
                    wait: None,
                });
            }
            QueueState::Released => {}
        }
    }
}
```

```rust
// crates/bridge-coordinator/src/batch.rs, in run_admission before acquire loop
let mut queue_guard = QueueAdmissionGuard::waiting(&deps.runtime);

// when acquire succeeds
p = deps.runtime.semaphore.clone().acquire_owned() => {
    let permit = p.expect("batch semaphore closed");
    queue_guard.admitted();
    break permit;
}

// in spawned child future, hold guard with permit
inflight.push(Box::pin(async move {
    let _permit = permit;
    let _queue_guard = queue_guard;
    let _ = h.await;
    task
}));
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-coordinator queue_observability_tests -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-coordinator/src/batch.rs && git commit -m "add queue admission observations"
```

### Task 8: Optional `/metrics` Route
**Files:** Modify/test `crates/bridge-a2a-inbound/src/server.rs:25-32, 85-120, 133-155, 250-256, 676-686, 3402-3409`.
**Interfaces:** Consumes `Option<bridge_observ::MetricsEndpoint>`. Produces `InboundServer::with_metrics_endpoint(self, Option<MetricsEndpoint>) -> Self`, `GET /metrics`.
**Cohesion with Slices 2 & 3:** `/metrics` is independent of future `[traces]` drill-down routes; enabling metrics now does not imply Slice 2 HTTP surfaces later.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-a2a-inbound/src/server.rs
#[cfg(test)]
mod metrics_route_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn metrics_route_404_when_no_endpoint() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn metrics_route_requires_bearer_when_enabled() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant)).with_metrics_endpoint(Some(
            bridge_observ::PrometheusObserver::new(bridge_observ::LabelVocabulary::default())
                .unwrap()
                .endpoint(),
        ));
        let resp = router(srv)
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn metrics_route_returns_text_exposition_when_enabled_and_authorized() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant)).with_metrics_endpoint(Some(
            bridge_observ::PrometheusObserver::new(bridge_observ::LabelVocabulary::default())
                .unwrap()
                .endpoint(),
        ));
        let resp = router(srv)
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .header("authorization", "Bearer test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```
- [ ] Step: run it, expect FAIL with missing `bridge_observ` dependency and metrics endpoint field/route
```bash
cargo test -p bridge-a2a-inbound metrics_route_tests -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```toml
# crates/bridge-a2a-inbound/Cargo.toml
bridge-observ = { path = "../bridge-observ" }
```

```rust
// crates/bridge-a2a-inbound/src/server.rs, InboundServer field
metrics_endpoint: Option<bridge_observ::MetricsEndpoint>,

// from_coordinator initializer
metrics_endpoint: None,

// builder
#[must_use]
pub fn with_metrics_endpoint(mut self, endpoint: Option<bridge_observ::MetricsEndpoint>) -> Self {
    self.metrics_endpoint = endpoint;
    self
}

// router()
pub fn router(self: Arc<Self>) -> Router {
    let router = Router::new()
        .route("/.well-known/agent-card.json", get(serve_card))
        .route("/", post(jsonrpc));
    let router = if self.metrics_endpoint.is_some() {
        router.route("/metrics", get(metrics))
    } else {
        router
    };
    router.with_state(self)
}

// handler
async fn metrics(State(srv): State<Arc<InboundServer>>, headers: HeaderMap) -> Response {
    let Some(endpoint) = srv.metrics_endpoint.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(token) = bearer_token(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if srv
        .auth
        .authorize(&InboundRequest::with_token(&token))
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match endpoint.render() {
        Ok(body) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            body,
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "metrics exposition failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-a2a-inbound metrics_route_tests -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-a2a-inbound/Cargo.toml crates/bridge-a2a-inbound/src/server.rs && git commit -m "add optional metrics route"
```

### Task 9: Config, Main Wiring, Fanout, and Restart Rebuild
**Files:** Modify/test `bin/a2a-bridge/src/config.rs:118-167, 1034-1075`; modify `bin/a2a-bridge/src/main.rs:663-669, 677-710, 6027-6129, 6137-6148`; modify/test `crates/bridge-observ/src/lib.rs:260-360`.
**Interfaces:** Consumes `[metrics]` config. Produces `MetricsConfig`, `RegistryConfig::metrics_config(&self) -> Result<MetricsConfig, ConfigError>`, `PrometheusObserver::rebuild_from_turn_log(&self, rows: &[TurnLogRow])`.
**Cohesion with Slices 2 & 3:** `[metrics]` is separate from future `[traces]` and `[storage]`; turn-log writer is enabled by metrics config but table/rows are stored under the existing task store so later routes and retention do not need schema changes.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// bin/a2a-bridge/src/config.rs
#[cfg(test)]
mod metrics_config_tests {
    use super::*;

    #[test]
    fn metrics_defaults_off_with_prometheus_exporter_and_turn_log_true() {
        let cfg = RegistryConfig::parse(
            r#"
default = "codex"
[server]
addr = "127.0.0.1:0"
[[agents]]
id = "codex"
cmd = "codex"
"#,
        )
        .unwrap();
        let metrics = cfg.metrics_config().unwrap();
        assert!(!metrics.enabled);
        assert!(metrics.prometheus);
        assert!(metrics.turn_log);
    }

    #[test]
    fn metrics_rejects_unknown_exporter() {
        let cfg = RegistryConfig::parse(
            r#"
default = "codex"
[server]
addr = "127.0.0.1:0"
[metrics]
enabled = true
exporters = ["prometheus", "otel"]
[[agents]]
id = "codex"
cmd = "codex"
"#,
        )
        .unwrap();
        let err = cfg.metrics_config().unwrap_err().to_string();
        assert!(err.contains("unsupported [metrics].exporters"));
    }
}
```

```rust
// crates/bridge-observ/src/lib.rs
#[cfg(test)]
mod rebuild_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::task_store::TurnLogRow;

    #[test]
    fn rebuild_from_turn_log_populates_counters_and_seeds_dedupe() {
        let prom = PrometheusObserver::new(LabelVocabulary {
            agents: ["codex".to_string()].into_iter().collect(),
            models: ["gpt-5.5".to_string()].into_iter().collect(),
            efforts: ["high".to_string()].into_iter().collect(),
        })
        .unwrap();
        prom.rebuild_from_turn_log(&[TurnLogRow {
            turn_id: TurnId::parse("turn-boot").unwrap(),
            session_id: ContextId::parse("ctx-boot").unwrap(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: None,
            prompt_id: Some("prompt-a".to_string()),
            started_ms: Some(0),
            completed_ms: Some(1000),
            latency_ms: Some(1000),
            ttft_ms: Some(100),
            outcome: Some("success".to_string()),
            failure_class: None,
            input_tokens: Some(10),
            output_tokens: Some(20),
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            cost_amount: Some(0.5),
            cost_currency: Some("USD".to_string()),
            traceparent: None,
        }]);
        let out = prom.endpoint().render().unwrap();
        assert!(out.contains("bridge_turns_total{agent=\"codex\",effort=\"high\",model=\"gpt-5.5\",outcome=\"success\"} 1"));
        assert!(out.contains("bridge_turn_cost_total{agent=\"codex\",currency=\"USD\",model=\"gpt-5.5\"} 0.5"));
        assert!(!prom.dedupe().mark_usage(&TurnId::parse("turn-boot").unwrap()));
    }
}
```
- [ ] Step: run it, expect FAIL with missing metrics config and rebuild method
```bash
cargo test -p a2a-bridge metrics_config_tests -- --nocapture
cargo test -p bridge-observ rebuild_from_turn_log_populates_counters_and_seeds_dedupe -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// bin/a2a-bridge/src/config.rs, add to RegistryConfig
#[serde(default)]
pub metrics: MetricsToml,

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MetricsToml {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_metrics_exporters")]
    pub exporters: Vec<String>,
    #[serde(default = "default_true")]
    pub turn_log: bool,
}

impl Default for MetricsToml {
    fn default() -> Self {
        Self {
            enabled: false,
            exporters: default_metrics_exporters(),
            turn_log: true,
        }
    }
}

fn default_metrics_exporters() -> Vec<String> {
    vec!["prometheus".to_string()]
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub prometheus: bool,
    pub turn_log: bool,
}

impl RegistryConfig {
    pub fn metrics_config(&self) -> Result<MetricsConfig, ConfigError> {
        for exporter in &self.metrics.exporters {
            if exporter != "prometheus" {
                return Err(ConfigError::Registry(format!(
                    "unsupported [metrics].exporters value {exporter:?}"
                )));
            }
        }
        Ok(MetricsConfig {
            enabled: self.metrics.enabled,
            prometheus: self.metrics.exporters.iter().any(|e| e == "prometheus"),
            turn_log: self.metrics.turn_log,
        })
    }
}
```

```rust
// crates/bridge-observ/src/lib.rs, add method
impl PrometheusObserver {
    pub fn rebuild_from_turn_log(&self, rows: &[bridge_core::task_store::TurnLogRow]) {
        for row in rows {
            let ctx = bridge_core::ports::TurnContext {
                turn_id: row.turn_id.clone(),
                session_id: row.session_id.clone(),
                task_id: row.task_id.clone(),
                workflow: row.workflow.clone(),
                node: row.node.clone(),
                attempt: row.attempt,
                agent: row.agent.clone(),
                model: row.model.clone(),
                effort: row.effort.clone(),
                mode: row.mode.clone(),
                prompt_id: row.prompt_id.clone(),
                traceparent: row.traceparent.clone(),
            };
            let outcome = match row.outcome.as_deref() {
                Some("success") => bridge_core::ports::TurnOutcome::Success,
                Some("canceled") => bridge_core::ports::TurnOutcome::Canceled,
                Some("failed") => bridge_core::ports::TurnOutcome::Failed(
                    match row.failure_class.as_deref() {
                        Some("agent_crashed") => bridge_core::ports::FailureClass::AgentCrashed,
                        Some("timed_out") => bridge_core::ports::FailureClass::TimedOut,
                        Some("overloaded") => bridge_core::ports::FailureClass::Overloaded,
                        Some("config") => bridge_core::ports::FailureClass::Config,
                        Some("transport") => bridge_core::ports::FailureClass::Transport,
                        _ => bridge_core::ports::FailureClass::Other,
                    },
                ),
                _ => continue,
            };
            let (agent, model, effort) = self.labels(&ctx);
            self.turns_total
                .with_label_values(&[&agent, &model, &effort, outcome_label(&outcome)])
                .inc();
            if let Some(ms) = row.latency_ms {
                self.turn_duration
                    .with_label_values(&[&agent, &model])
                    .observe(ms as f64 / 1000.0);
            }
            if let Some(ms) = row.ttft_ms {
                self.turn_ttft
                    .with_label_values(&[&agent])
                    .observe(ms as f64 / 1000.0);
            }
            for (kind, value) in [
                ("input", row.input_tokens),
                ("output", row.output_tokens),
                ("thought", row.thought_tokens),
                ("cached_read", row.cached_read_tokens),
                ("cached_write", row.cached_write_tokens),
            ] {
                if let Some(value) = value.filter(|v| *v > 0) {
                    self.tokens_total.with_label_values(&[&agent, kind]).inc_by(value);
                }
            }
            match (&row.cost_amount, &row.cost_currency) {
                (Some(amount), Some(currency)) if valid_iso4217(currency) => {
                    self.cost_total
                        .with_label_values(&[&agent, &model, currency])
                        .inc_by(amount.max(0.0));
                }
                (Some(_), _) => self.cost_dropped.with_label_values(&[&agent]).inc(),
                _ => {}
            }
            self.dedupe.seed(&row.turn_id);
        }
    }
}
```

```rust
// bin/a2a-bridge/src/main.rs, change batch_runtime signature/body
fn batch_runtime(
    cfg: &RegistryConfig,
    observer: Arc<dyn bridge_core::ports::Observer>,
) -> Result<Option<bridge_coordinator::BatchRuntime>, config::ConfigError> {
    Ok(cfg.batch_config()?.map(|batch| {
        bridge_coordinator::BatchRuntime::new(
            batch.max_concurrent,
            batch.default_concurrency,
            observer,
        )
    }))
}

// after task_store creation
let metrics_cfg = cfg.metrics_config()?;
let prometheus_observer = if metrics_cfg.enabled && metrics_cfg.prometheus {
    let vocab = bridge_observ::LabelVocabulary {
        agents: probe_entries.keys().cloned().collect(),
        models: probe_entries.values().filter_map(|e| e.model.clone()).collect(),
        efforts: probe_entries
            .values()
            .filter_map(|e| e.effort.as_ref().map(|eff| eff.to_string()))
            .collect(),
    };
    Some(Arc::new(bridge_observ::PrometheusObserver::new(vocab)?))
} else {
    None
};

if let Some(prom) = &prometheus_observer {
    let rows = task_store.turn_log_rows().await.unwrap_or_else(|e| {
        tracing::warn!(error = ?e, "turn_log rebuild skipped");
        Vec::new()
    });
    prom.rebuild_from_turn_log(&rows);
}

let observer: Arc<dyn bridge_core::ports::Observer> = if !metrics_cfg.enabled {
    Arc::new(bridge_observ::NoopObserver)
} else {
    let mut sinks: Vec<Arc<dyn bridge_core::ports::Observer>> = Vec::new();
    if let Some(prom) = &prometheus_observer {
        sinks.push(prom.clone());
    }
    if metrics_cfg.turn_log {
        let dedupe = prometheus_observer
            .as_ref()
            .map(|p| p.dedupe())
            .unwrap_or_else(|| Arc::new(bridge_observ::TurnDedupe::default()));
        let dropped = prometheus_observer
            .as_ref()
            .map(|p| p.drop_counter())
            .unwrap_or_else(bridge_observ::DropCounter::disabled);
        sinks.push(Arc::new(bridge_observ::TurnLogObserver::new(
            task_store.clone(),
            dedupe,
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
    Arc::new(bridge_observ::FanoutObserver::new(sinks))
};

// use observer in batch and coordinator
let batch = batch_runtime(&cfg, observer.clone())?;

// Coordinator::new call append observer.clone()
observer.clone(),

// server builder
.with_metrics_endpoint(prometheus_observer.as_ref().map(|p| p.endpoint()))
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p a2a-bridge metrics_config_tests -- --nocapture
cargo test -p bridge-observ rebuild_from_turn_log_populates_counters_and_seeds_dedupe -- --nocapture
```
- [ ] Step: commit
```bash
git add bin/a2a-bridge/src/config.rs bin/a2a-bridge/src/main.rs crates/bridge-observ/src/lib.rs && git commit -m "wire metrics config and rebuild"
```

### Task 10: Workflow Node Turn Events and Final Verification
**Files:** Modify/test `crates/bridge-workflow/src/executor.rs:19-26, 207-304, 340-516`; modify `crates/bridge-a2a-inbound/src/server.rs:539-610, 741-754, 2422-2445`; modify `crates/bridge-coordinator/src/detached.rs:1198-1270, 1412-1661`.
**Interfaces:** Consumes `WorkflowRunContext.observer: Arc<dyn Observer>`, parent `traceparent`. Produces per-node `NodeStarted`, `TurnStarted`, `TurnFinished`, `UsageFinalized`, `NodeFinished`.
**Cohesion with Slices 2 & 3:** Node contexts set `workflow` and `node`, and inherit parent traceparent; Slice 2 can link task journal nodes to turn rows, while Slice 3 can purge all node turns by `completed_ms`.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// crates/bridge-workflow/src/executor.rs
#[cfg(test)]
mod observability_tests {
    use super::*;
    use bridge_core::ports::{ObsEvent, Observer};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Rec(Mutex<Vec<&'static str>>);

    impl Observer for Rec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::NodeStarted { .. } => "node_started",
                ObsEvent::TurnStarted { .. } => "turn_started",
                ObsEvent::UsageFinalized { .. } => "usage",
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::NodeFinished { .. } => "node_finished",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    #[tokio::test]
    async fn workflow_node_emits_lifecycle_around_usage() {
        let rec = Arc::new(Rec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: Some("prompt/workflow".to_string()),
        };
        let graph = WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: None,
            }],
            panel: None,
        };
        let registry = Arc::new(crate::tests::OneShotUsageRegistry::new());
        let exec = WorkflowExecutor::new(registry);
        let mut stream = exec.run_with_context(&graph, "input", "task-1", tokio_util::sync::CancellationToken::new(), ctx);
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec!["node_started", "turn_started", "usage", "turn_finished", "node_finished"]
        );
    }
}
```
- [ ] Step: run it, expect FAIL with missing `WorkflowRunContext` fields and observer events
```bash
cargo test -p bridge-workflow workflow_node_emits_lifecycle_around_usage -- --nocapture
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// crates/bridge-workflow/src/executor.rs, WorkflowRunContext
#[derive(Clone)]
pub struct WorkflowRunContext {
    pub session_cwd: Option<SessionCwd>,
    pub make_rich_sink: Option<Arc<dyn RichEventSinkFactory>>,
    pub observer: Arc<dyn bridge_core::ports::Observer>,
    pub parent_traceparent: Option<bridge_core::ports::TraceParent>,
    pub task_id: Option<bridge_core::ids::TaskId>,
    pub prompt_id: Option<String>,
}

impl Default for WorkflowRunContext {
    fn default() -> Self {
        Self {
            session_cwd: None,
            make_rich_sink: None,
            observer: Arc::new(bridge_observ::NoopObserver),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        }
    }
}
```

```rust
// crates/bridge-workflow/Cargo.toml
bridge-observ = { path = "../bridge-observ" }
```

```rust
// crates/bridge-workflow/src/executor.rs, helper
fn node_turn_context(
    wf_id: &str,
    node: &WorkflowNode,
    run_id: &str,
    ctx: &WorkflowRunContext,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    attempt: u32,
) -> bridge_core::ports::TurnContext {
    bridge_core::ports::TurnContext {
        turn_id: bridge_core::ids::TurnId::parse(format!("turn-{}", a2a::new_task_id()))
            .expect("a2a task id is non-empty"),
        session_id: bridge_core::ids::ContextId::parse(run_id).unwrap_or_else(|_| {
            bridge_core::ids::ContextId::parse(format!("workflow-{run_id}"))
                .expect("fallback context id is non-empty")
        }),
        task_id: ctx.task_id.clone(),
        workflow: Some(wf_id.to_string()),
        node: Some(node.id.as_str().to_string()),
        attempt,
        agent: node.agent.as_str().to_string(),
        model,
        effort,
        mode,
        prompt_id: ctx.prompt_id.clone(),
        traceparent: ctx.parent_traceparent.clone(),
    }
}
```

```rust
// crates/bridge-workflow/src/executor.rs, in run_node around each prompt attempt
let obs_ctx = node_turn_context(
    wf_id,
    node,
    run_id,
    ctx,
    resolved.entry.model.clone(),
    resolved.entry.effort.as_ref().map(|e| e.to_string()),
    resolved.entry.mode.clone(),
    attempt_index,
);
ctx.observer.record(&bridge_core::ports::ObsEvent::NodeStarted { ctx: &obs_ctx });
ctx.observer.record(&bridge_core::ports::ObsEvent::TurnStarted { ctx: &obs_ctx });
let started = std::time::Instant::now();
let mut ttft = None;

// on first text/done/error event
if ttft.is_none() {
    ttft = Some(started.elapsed());
}

// when Update::Usage(mut u) is seen after u.at_ms stamp
ctx.observer.record(&bridge_core::ports::ObsEvent::UsageFinalized {
    ctx: &obs_ctx,
    usage: &u,
    fin: bridge_core::ports::UsageFinalization::TurnFinal,
});

// after node exits
let outcome = match exit {
    NodeTurnExit::Normal => bridge_core::ports::TurnOutcome::Success,
    NodeTurnExit::Canceled => bridge_core::ports::TurnOutcome::Canceled,
    NodeTurnExit::Error(ref e) => bridge_core::ports::TurnOutcome::Failed(match e {
        BridgeError::AgentCrashed { .. } => bridge_core::ports::FailureClass::AgentCrashed,
        BridgeError::AgentTimedOut | BridgeError::CancelTimeout => bridge_core::ports::FailureClass::TimedOut,
        BridgeError::AgentOverloaded => bridge_core::ports::FailureClass::Overloaded,
        BridgeError::FrameError | BridgeError::UpstreamA2aError => bridge_core::ports::FailureClass::Transport,
        BridgeError::ConfigInvalid { .. } | BridgeError::UnknownAgent { .. } => bridge_core::ports::FailureClass::Config,
        _ => bridge_core::ports::FailureClass::Other,
    }),
};
ctx.observer.record(&bridge_core::ports::ObsEvent::TurnFinished {
    ctx: &obs_ctx,
    latency: started.elapsed(),
    ttft,
    outcome: &outcome,
});
ctx.observer.record(&bridge_core::ports::ObsEvent::NodeFinished {
    ctx: &obs_ctx,
    outcome: &outcome,
});
```

```rust
// crates/bridge-a2a-inbound/src/server.rs and crates/bridge-coordinator/src/detached.rs,
// every WorkflowRunContext literal must add:
observer: srv.coordinator().observer(),
parent_traceparent: routed.traceparent.clone(),
task_id: Some(routed.task.clone()),
prompt_id: routed.prompt_id.clone(),
```
- [ ] Step: run tests, expect PASS
```bash
cargo test -p bridge-workflow workflow_node_emits_lifecycle_around_usage -- --nocapture
```
- [ ] Step: commit
```bash
git add crates/bridge-workflow/Cargo.toml crates/bridge-workflow/src/executor.rs crates/bridge-a2a-inbound/src/server.rs crates/bridge-coordinator/src/detached.rs && git commit -m "emit workflow node turn observations"
```

### Task 11: Slice-1 Full Gate
**Files:** No source changes unless verification finds a Slice-1 regression.
**Interfaces:** Consumes all earlier task outputs. Produces verified Slice-1 acceptance report.
- [ ] Step: write the failing test  (ACTUAL test code in a ```rust block)
```rust
// No new test file. This task runs the repository gates required by AGENTS.md after all Slice 1 tests exist.
```
- [ ] Step: run it, expect FAIL only if fmt/clippy/tests reveal an issue
```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -j 1
```
- [ ] Step: minimal implementation  (ACTUAL code)
```rust
// Fix only failures caused by Slice 1 changes. Do not rebaseline unrelated failures.
```
- [ ] Step: run tests, expect PASS
```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -j 1
```
- [ ] Step: commit
```bash
git add . && git commit -m "verify slice one observability"
```

## Acceptance Coverage

| Slice-1 acceptance | Covered by tasks |
|---|---|
| 1. `Observer`/`ObsEvent` in `bridge-core`, Prometheus-free; adapters in `bridge-observ`; domain has no Prometheus reference. | Tasks 1, 2, 4 |
| 2. `/metrics` valid exposition after run; disabled/no-exporter 404; no bearer 401. | Tasks 2, 8, 9 |
| 3. Turn counters/histograms increment exactly once per turn across drive paths; bypass contract through shared hooks. | Tasks 5, 6, 10 |
| 4. Queue depth cancellation-safe; queue wait records. | Task 7 |
| 5. Cost covers warm inline turns, dedupes replayed `turn_id`, records retry attempts, rebuilds from `turn_log`, never sums unknown currencies. | Tasks 2, 3, 4, 5, 6, 9 |
| 6. Failing turn-log write never fails turn; dropped counter increments. | Task 4 |
| 7. `turn_log` row per finished turn with eval columns; eval query over `prompt_id` x `model` works from stored rows. | Tasks 3, 4, 6, 10 |
| 8. fmt + clippy + full suite green. | Task 11 |