//! bridge-observ — tracing and Prometheus exports for operational observability.

use bridge_core::orch::{TerminalUsage, UsageSnapshot};
use bridge_core::ports::{
    FailureClass, ObsEvent, Observer, TurnContext, TurnOutcome, UsageFinalization,
};
use bridge_core::task_store::{TaskStore, TurnLogFinished, TurnLogUsage};
use prometheus::{
    CounterVec, Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::collections::HashSet;
use std::{
    panic,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};

/// Fallback no-op implementation, useful where observability is disabled.
pub struct NoopObserver;

impl Observer for NoopObserver {
    fn record(&self, _e: &ObsEvent<'_>) {}
}

/// Observability fanout sink: forward each event to all configured observers.
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
            let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| sink.record(e)));
        }
    }
}

/// Emission-time deduplication decorator.
///
/// A turn's final lifecycle event or finalization event is forwarded at most once to the
/// wrapped observer, so sink fanout stays in lock-step for both sink types.
pub struct DedupObserver {
    inner: Arc<dyn Observer>,
    dedupe: Arc<TurnDedupe>,
}

impl DedupObserver {
    pub fn new(inner: Arc<dyn Observer>) -> Self {
        Self {
            inner,
            dedupe: Arc::new(TurnDedupe::default()),
        }
    }
}

impl Observer for DedupObserver {
    fn record(&self, e: &ObsEvent<'_>) {
        match e {
            ObsEvent::TurnFinished { ctx, .. } => {
                if !self.dedupe.mark_finished(&ctx.turn_id) {
                    return;
                }
            }
            ObsEvent::UsageFinalized { ctx, .. } => {
                if !self.dedupe.mark_usage(&ctx.turn_id) {
                    return;
                }
            }
            _ => {}
        }
        self.inner.record(e);
    }
}

/// Install a JSON tracing subscriber (env-filter driven). Idempotent-safe: if a
/// global subscriber is already registered this call is a no-op.
pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

/// Like [`init`] but writes the JSON trace stream to STDERR, leaving STDOUT clean for protocols that
/// own it (the MCP stdio transport). Idempotent-safe.
pub fn init_stderr() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

/// Build an `info`-level span carrying the four A2A correlation ids.
pub fn task_span(
    task_id: &str,
    session_id: &str,
    caller_id: &str,
    agent_id: &str,
) -> tracing::Span {
    tracing::info_span!(
        "task",
        task_id = %task_id,
        session_id = %session_id,
        caller_id = %caller_id,
        agent_id = %agent_id,
    )
}

#[derive(Clone, Debug, Default)]
pub struct LabelVocabulary {
    pub agents: HashSet<String>,
    pub models: HashSet<String>,
    pub efforts: HashSet<String>,
}

#[derive(Default)]
pub struct TurnDedupe {
    seen: Mutex<(HashSet<String>, HashSet<String>)>,
}

impl TurnDedupe {
    pub fn mark_finished(&self, turn_id: &bridge_core::ids::TurnId) -> bool {
        let mut lock = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        lock.0.insert(turn_id.as_str().to_string())
    }

    pub fn mark_usage(&self, turn_id: &bridge_core::ids::TurnId) -> bool {
        let mut lock = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        lock.1.insert(turn_id.as_str().to_string())
    }

    pub fn seed(&self, turn_id: &bridge_core::ids::TurnId) {
        let mut lock = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let id = turn_id.as_str().to_string();
        lock.0.insert(id.clone());
        lock.1.insert(id);
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
            counter
                .with_label_values(&[normalize_sink_label(sink)])
                .inc();
        }
    }
}

fn normalize_sink_label(sink: &str) -> &str {
    match sink {
        "turn_log" => "turn_log",
        "turnlog" => "turn_log",
        "prometheus" => "prometheus",
        _ => "other",
    }
}

type NowMs = Arc<dyn Fn() -> i64 + Send + Sync>;

enum TurnLogCommand {
    Finished(TurnLogFinished),
    Usage(TurnLogUsage),
    Flush(oneshot::Sender<()>),
}

pub struct TurnLogObserver {
    tx: Option<mpsc::Sender<TurnLogCommand>>,
    dropped: DropCounter,
    now_ms: NowMs,
}

impl TurnLogObserver {
    pub fn new(
        store: Arc<dyn TaskStore>,
        dropped: DropCounter,
        capacity: usize,
        now_ms: NowMs,
    ) -> Self {
        if capacity == 0 {
            return Self {
                tx: None,
                dropped,
                now_ms,
            };
        }
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
            tx: Some(tx),
            dropped,
            now_ms,
        }
    }

    pub async fn flush(&self) {
        let Some(tx) = &self.tx else {
            return;
        };
        let (done_tx, done_rx) = oneshot::channel();
        if tx.send(TurnLogCommand::Flush(done_tx)).await.is_ok() {
            let _ = done_rx.await;
        }
    }

    fn try_send(&self, cmd: TurnLogCommand) {
        let Some(tx) = &self.tx else {
            self.dropped.observe("turn_log");
            return;
        };
        if tx.try_send(cmd).is_err() {
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
                if *fin != UsageFinalization::TurnFinal {
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
            Opts::new(
                "bridge_turns_total",
                "Completed turns by bounded dimensions",
            ),
            &["agent", "model", "effort", "outcome"],
        )?;
        let buckets = vec![
            0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
        ];
        let turn_duration = HistogramVec::new(
            HistogramOpts::new("bridge_turn_duration_seconds", "Turn latency")
                .buckets(buckets.clone()),
            &["agent", "model"],
        )?;
        let turn_ttft = HistogramVec::new(
            HistogramOpts::new("bridge_turn_ttft_seconds", "Turn time to first token"),
            &["agent"],
        )?;
        let turns_in_flight = IntGauge::new("bridge_turns_in_flight", "Currently running turns")?;
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
            Opts::new(
                "bridge_turn_cost_dropped_total",
                "Costs dropped because currency was invalid",
            ),
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
            Box::new(turn_duration.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(turn_ttft.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(turns_in_flight.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(queue_depth.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(queue_wait.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(cost_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(cost_dropped.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(tokens_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(dropped_total.clone()) as Box<dyn prometheus::core::Collector>,
        ] {
            registry.register(collector)?;
        }

        Ok(Self {
            registry,
            vocab,
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

    pub fn drop_counter(&self) -> DropCounter {
        DropCounter::new(self.dropped_total.clone())
    }

    fn labels(&self, ctx: &TurnContext) -> (String, String, String) {
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
                self.tokens_total
                    .with_label_values(&[agent, kind])
                    .inc_by(value);
            }
        }
    }
}

impl Observer for PrometheusObserver {
    fn record(&self, e: &ObsEvent<'_>) {
        match e {
            ObsEvent::TurnFinished {
                ctx,
                latency,
                ttft,
                outcome,
            } => {
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
                    self.queue_wait
                        .with_label_values(&[])
                        .observe(wait.as_secs_f64());
                }
            }
            ObsEvent::UsageFinalized { ctx, usage, fin } => {
                if *fin != UsageFinalization::TurnFinal {
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
            | ObsEvent::TurnStarted { .. }
            | ObsEvent::TaskFinished { .. }
            | ObsEvent::NodeStarted { .. }
            | ObsEvent::NodeFinished { .. } => {}
        }
    }
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
    code.len() == 3 && code.as_bytes().iter().all(u8::is_ascii_uppercase)
}

#[cfg(test)]
mod obs_port_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{TraceParent, TurnContext, TurnOutcome, UsageFinalization};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

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
            latency: Duration::from_millis(7),
            ttft: Some(Duration::from_millis(2)),
            outcome: &TurnOutcome::Success,
        });
    }

    #[test]
    fn traceparent_parses_roundtrips_and_rejects_malformed() {
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let parsed = TraceParent::parse_header_value(raw).unwrap();
        assert_eq!(parsed.to_header_value(), raw);
        assert!(TraceParent::parse_header_value("00-not-hex-00f067aa0ba902b7-01").is_none());
        assert!(TraceParent::parse_header_value(
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        )
        .is_none());
        assert!(TraceParent::parse_header_value(
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01"
        )
        .is_none());
    }

    #[test]
    fn fanout_record_catches_panics_and_continues() {
        struct PanickingSink;
        impl Observer for PanickingSink {
            fn record(&self, _e: &ObsEvent<'_>) {
                panic!("simulated panic");
            }
        }

        struct RecordingSink {
            count: AtomicUsize,
        }
        impl Observer for RecordingSink {
            fn record(&self, _e: &ObsEvent<'_>) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let panicking = Arc::new(PanickingSink);
        let recording = Arc::new(RecordingSink {
            count: AtomicUsize::new(0),
        });
        let observer = FanoutObserver::new(vec![panicking, recording.clone()]);

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
            traceparent: None,
        };

        observer.record(&ObsEvent::TaskStarted { ctx: &ctx });
        assert_eq!(recording.count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dedup_observer_forwards_turn_finished_and_usage_finalized_once() {
        struct RecordingSink {
            turn_events: AtomicUsize,
            usage_events: AtomicUsize,
        }
        impl Observer for RecordingSink {
            fn record(&self, _e: &ObsEvent<'_>) {
                match _e {
                    ObsEvent::TurnFinished { .. } => {
                        self.turn_events.fetch_add(1, Ordering::SeqCst);
                    }
                    ObsEvent::UsageFinalized { .. } => {
                        self.usage_events.fetch_add(1, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        }

        let first = Arc::new(RecordingSink {
            turn_events: AtomicUsize::new(0),
            usage_events: AtomicUsize::new(0),
        });
        let fanout = FanoutObserver::new(vec![first.clone()]);
        let observer = DedupObserver::new(Arc::new(fanout));
        let outcome = TurnOutcome::Success;

        let ctx = TurnContext {
            turn_id: TurnId::parse("turn-dedup").unwrap(),
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
            traceparent: None,
        };

        let event = ObsEvent::TurnFinished {
            ctx: &ctx,
            latency: Duration::from_millis(25),
            ttft: Some(Duration::from_millis(5)),
            outcome: &outcome,
        };
        let usage = UsageSnapshot {
            used: Some(1),
            size: Some(1),
            cost: Some(UsageCost {
                amount: 0.25,
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
            at_ms: 0,
        };

        observer.record(&event);
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &ctx,
            usage: &usage,
            fin: UsageFinalization::TurnFinal,
        });
        observer.record(&event);
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &ctx,
            usage: &usage,
            fin: UsageFinalization::TurnFinal,
        });

        assert_eq!(first.turn_events.load(Ordering::SeqCst), 1);
        assert_eq!(first.usage_events.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drop_counter_labels_are_bounded() {
        let observer = PrometheusObserver::new(LabelVocabulary::default()).unwrap();
        let drop_counter = observer.drop_counter();
        drop_counter.observe("turn_log");
        drop_counter.observe("unknown");

        let out = observer.endpoint().render().unwrap();
        assert!(out.contains("bridge_observer_dropped_total{sink=\"turn_log\"} 1"));
        assert!(out.contains("bridge_observer_dropped_total{sink=\"other\"} 1"));
    }
}

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
        let observer = TurnLogObserver::new(store, prom.drop_counter(), 0, Arc::new(|| 1));
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

    #[tokio::test]
    async fn turn_log_observer_flush_waits_for_barrier_on_full_queue() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let observer = TurnLogObserver::new(
            store.clone(),
            DropCounter::disabled(),
            1,
            Arc::new(|| 10_000),
        );
        let c = ctx("turn-log-flush-capacity-1");
        let outcome = TurnOutcome::Success;
        observer.record(&ObsEvent::TurnFinished {
            ctx: &c,
            latency: Duration::from_millis(250),
            ttft: None,
            outcome: &outcome,
        });
        observer.flush().await;

        let rows = store.turn_log_rows().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].turn_id.as_str(), "turn-log-flush-capacity-1");
    }
}

#[cfg(test)]
mod prometheus_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{ObsEvent, TurnContext, TurnOutcome, UsageFinalization};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
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
        observer.record(&ObsEvent::QueueChanged {
            in_flight: 1,
            queued: 2,
            wait: Some(Duration::from_millis(250)),
        });
        observer.record(&ObsEvent::QueueChanged {
            in_flight: 4,
            queued: 9,
            wait: None,
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

        let c2 = ctx("turn-2", "codex", Some("gpt-5.5"), Some("high"));
        let bad_currency = UsageSnapshot {
            cost: Some(UsageCost {
                amount: 99.0,
                currency: "us".to_string(),
            }),
            terminal: None,
            used: None,
            size: None,
            at_ms: 124,
        };
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c2,
            usage: &bad_currency,
            fin: UsageFinalization::TurnFinal,
        });

        let ars_currency = UsageSnapshot {
            cost: Some(UsageCost {
                amount: 1.5,
                currency: "ARS".to_string(),
            }),
            terminal: None,
            used: None,
            size: None,
            at_ms: 125,
        };
        let c3 = ctx("turn-3", "codex", Some("gpt-5.5"), Some("high"));
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c3,
            usage: &ars_currency,
            fin: UsageFinalization::TurnFinal,
        });

        let us_currency = UsageSnapshot {
            cost: Some(UsageCost {
                amount: 2.0,
                currency: "".to_string(),
            }),
            terminal: None,
            used: None,
            size: None,
            at_ms: 126,
        };
        let c4 = ctx("turn-4", "codex", Some("gpt-5.5"), Some("high"));
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c4,
            usage: &us_currency,
            fin: UsageFinalization::TurnFinal,
        });

        let empty_currency = UsageSnapshot {
            cost: Some(UsageCost {
                amount: 3.0,
                currency: "dollars".to_string(),
            }),
            terminal: None,
            used: None,
            size: None,
            at_ms: 127,
        };
        let c5 = ctx("turn-5", "codex", Some("gpt-5.5"), Some("high"));
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &c5,
            usage: &empty_currency,
            fin: UsageFinalization::TurnFinal,
        });

        let out = observer.endpoint().render().unwrap();
        assert!(out.contains("bridge_turns_total{agent=\"codex\",effort=\"high\",model=\"gpt-5.5\",outcome=\"success\"} 1"));
        assert!(
            out.contains("bridge_turn_duration_seconds_sum{agent=\"codex\",model=\"gpt-5.5\"} 1.5")
        );
        assert!(out.contains("bridge_turn_ttft_seconds_sum{agent=\"codex\"} 0.1"));
        assert!(out.contains("bridge_turns_in_flight 4"));
        assert!(out.contains("bridge_queue_depth 9"));
        assert!(out.contains("bridge_queue_wait_seconds_sum 0.25"));
        assert!(out.contains(
            "bridge_turn_cost_total{agent=\"codex\",currency=\"USD\",model=\"gpt-5.5\"} 0.25"
        ));
        assert!(out.contains(
            "bridge_turn_cost_total{agent=\"codex\",currency=\"ARS\",model=\"gpt-5.5\"} 1.5"
        ));
        assert!(out.contains("bridge_turn_cost_dropped_total{agent=\"codex\"} 3"));
        assert!(out.contains("bridge_turn_tokens_total{agent=\"codex\",kind=\"input\"} 3"));
        assert!(!out.contains("currency=\"us\""));
        assert!(!out.contains("currency=\"\""));
        assert!(!out.contains("currency=\"dollars\""));
    }

    #[test]
    fn prometheus_normalizes_unbounded_labels() {
        let observer = PrometheusObserver::new(LabelVocabulary::default()).unwrap();
        let c = ctx(
            "turn-dup",
            "unbounded-user-value",
            Some("custom-model"),
            Some("weird"),
        );
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
        assert!(out.contains("bridge_turns_total{agent=\"other\",effort=\"other\",model=\"other\",outcome=\"success\"} 2"));
    }

    #[test]
    fn dedup_observer_forwards_turn_finished_to_all_sinks_once() {
        struct RecordingSink {
            count: AtomicUsize,
        }
        impl Observer for RecordingSink {
            fn record(&self, _e: &ObsEvent<'_>) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let first = Arc::new(RecordingSink {
            count: AtomicUsize::new(0),
        });
        let second = Arc::new(RecordingSink {
            count: AtomicUsize::new(0),
        });
        let fanout = FanoutObserver::new(vec![first.clone(), second.clone()]);
        let observer = DedupObserver::new(Arc::new(fanout));
        let c = ctx("turn-dedup-2", "codex", Some("gpt-5.5"), Some("high"));
        let ok = TurnOutcome::Success;

        observer.record(&ObsEvent::TurnFinished {
            ctx: &c,
            latency: Duration::from_secs(1),
            ttft: None,
            outcome: &ok,
        });
        observer.record(&ObsEvent::TurnFinished {
            ctx: &c,
            latency: Duration::from_secs(1),
            ttft: None,
            outcome: &ok,
        });

        assert_eq!(first.count.load(Ordering::SeqCst), 1);
        assert_eq!(second.count.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    /// A `MakeWriter` that writes into a shared `Vec<u8>`.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl BufWriter {
        fn new() -> (Self, Arc<Mutex<Vec<u8>>>) {
            let buf = Arc::new(Mutex::new(Vec::new()));
            (BufWriter(buf.clone()), buf)
        }
    }

    impl<'a> MakeWriter<'a> for BufWriter {
        type Writer = LockedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LockedWriter(self.0.clone())
        }
    }

    struct LockedWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LockedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn span_carries_all_four_ids() {
        let (writer, buf) = BufWriter::new();

        let subscriber = tracing_subscriber::fmt().with_writer(writer).finish();

        with_default(subscriber, || {
            let span = task_span("t", "s", "c", "kiro");
            let _guard = span.enter();
            tracing::info!("hi");
        });

        let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        for key in ["task_id", "session_id", "caller_id", "agent_id"] {
            assert!(captured.contains(key), "missing {key} in: {captured}");
        }
    }

    #[test]
    fn init_is_idempotent() {
        // Calling init() twice must not panic.
        init();
        init();
    }

    #[test]
    fn init_stderr_is_idempotent() {
        // Calling init_stderr() twice must not panic.
        init_stderr();
        init_stderr();
    }
}
