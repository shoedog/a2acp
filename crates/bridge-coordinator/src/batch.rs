use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use bridge_core::error::BridgeError;
use bridge_core::ids::{BatchId, TaskId, WorkflowId};
use bridge_core::ports::{ObsEvent, Observer};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::task_store::{
    BatchItem, BatchRecord, BatchStatus, BatchSummary, ChildClaim, ResumeClaim, TaskRecord,
    TaskRecordStatus,
};
use bridge_workflow::executor::WorkflowRunContext;
use bridge_workflow::graph::WorkflowGraph;
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use serde_json::json;
use std::time::Instant;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::detached::{
    encode_workflow_spec, finalize_detached, new_detached_task_id, spawn_detached_workflow,
    TaskProgressHub, SUPPORTED_SNAPSHOT_VERSION,
};
use crate::params::validate_cwd_str;

#[derive(Clone)]
pub struct BatchRuntime {
    pub semaphore: Arc<Semaphore>,
    pub default_concurrency: u32,
    pub max_concurrent: u32,
    pub observer: Arc<dyn Observer>,
    queue_counts: Arc<StdMutex<QueueCounts>>,
    pub batch_cancels: Arc<Mutex<HashMap<BatchId, CancellationToken>>>,
}

#[derive(Clone, Copy, Default)]
struct QueueCounts {
    queued: u64,
    in_flight: u64,
}

enum QueueState {
    Waiting,
    Admitted,
    Released,
}

pub struct QueueAdmissionGuard {
    observer: Arc<dyn Observer>,
    queue_counts: Arc<StdMutex<QueueCounts>>,
    started: Instant,
    state: QueueState,
}

impl BatchRuntime {
    pub fn new(max_concurrent: u32, default_concurrency: u32, observer: Arc<dyn Observer>) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
            default_concurrency,
            max_concurrent,
            observer,
            queue_counts: Arc::new(StdMutex::new(QueueCounts::default())),
            batch_cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn new_noop(max_concurrent: u32, default_concurrency: u32) -> Self {
        Self::new(max_concurrent, default_concurrency, Arc::new(NoopObserver))
    }
}

impl QueueAdmissionGuard {
    pub fn waiting(runtime: &BatchRuntime) -> Self {
        let mut counts = runtime.queue_counts.lock().unwrap();
        counts.queued += 1;
        let queued = counts.queued;
        let in_flight = counts.in_flight;
        runtime.observer.record(&ObsEvent::QueueChanged {
            in_flight,
            queued,
            wait: None,
        });

        Self {
            observer: runtime.observer.clone(),
            queue_counts: runtime.queue_counts.clone(),
            started: Instant::now(),
            state: QueueState::Waiting,
        }
    }

    pub fn admitted(&mut self) {
        if matches!(self.state, QueueState::Waiting) {
            let mut counts = self.queue_counts.lock().unwrap();
            counts.queued = counts.queued.saturating_sub(1);
            counts.in_flight = counts.in_flight.saturating_add(1);
            let queued = counts.queued;
            let in_flight = counts.in_flight;
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
                let mut counts = self.queue_counts.lock().unwrap();
                counts.queued = counts.queued.saturating_sub(1);
                let queued = counts.queued;
                let in_flight = counts.in_flight;
                self.state = QueueState::Released;
                self.observer.record(&ObsEvent::QueueChanged {
                    in_flight,
                    queued,
                    wait: None,
                });
            }
            QueueState::Admitted => {
                let mut counts = self.queue_counts.lock().unwrap();
                counts.in_flight = counts.in_flight.saturating_sub(1);
                let in_flight = counts.in_flight;
                let queued = counts.queued;
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

#[derive(Clone, Copy)]
struct NoopObserver;

impl Observer for NoopObserver {
    fn record(&self, _e: &ObsEvent<'_>) {}
}

#[derive(Clone)]
pub struct BatchDeps {
    pub detached: crate::detached::DetachedDeps,
    pub runtime: BatchRuntime,
    pub allowed_cwd_root: Option<SessionCwd>,
}

pub struct BatchParams {
    pub workflow: String,
    pub concurrency: Option<u32>,
    pub items: Vec<BatchItem>,
}

pub fn new_batch_id() -> BatchId {
    BatchId::parse(format!("batch-{}", a2a::new_task_id())).expect("new_task_id is non-empty")
}

fn now_ms(deps: &BatchDeps) -> i64 {
    deps.detached.clock.now_ms()
}

pub async fn run_batch(deps: &BatchDeps, params: BatchParams) -> Result<BatchId, BridgeError> {
    let workflow = WorkflowId::parse(params.workflow)
        .map_err(|_| BridgeError::InvalidRequest { field: "workflow" })?;
    let _graph = deps
        .detached
        .workflows
        .get(&workflow)
        .ok_or(BridgeError::InvalidRequest { field: "workflow" })?;
    if params.items.is_empty() {
        return Err(BridgeError::InvalidRequest { field: "items" });
    }
    if params.concurrency == Some(0) {
        return Err(BridgeError::InvalidRequest {
            field: "concurrency",
        });
    }

    let mut seen = HashSet::new();
    let mut items = Vec::with_capacity(params.items.len());
    for mut item in params.items {
        if item.item_id.is_empty() {
            return Err(BridgeError::InvalidRequest { field: "item_id" });
        }
        if !seen.insert(item.item_id.clone()) {
            return Err(BridgeError::InvalidRequest { field: "item_id" });
        }
        bridge_core::task_spec::validate_input(&item.input)?;
        if let Some(raw) = &item.session_cwd {
            let cwd = validate_cwd_str(raw, deps.allowed_cwd_root.as_ref(), "batch.item.cwd")?;
            item.session_cwd = Some(cwd.as_str().to_string());
        }
        items.push(item);
    }

    let concurrency = params
        .concurrency
        .unwrap_or(deps.runtime.default_concurrency)
        .min(deps.runtime.max_concurrent);
    let bid = new_batch_id();
    let now = deps.detached.clock.now_ms();
    let items_json = serde_json::to_string(&json!({"v": 1, "items": &items}))
        .map_err(|_| BridgeError::StoreFailure)?;
    deps.detached
        .task_store
        .create_batch(&BatchRecord {
            id: bid.clone(),
            workflow: workflow.as_str().to_string(),
            concurrency,
            total: items.len() as u32,
            status: BatchStatus::Working,
            items_json,
            error: None,
            created_ms: now,
            updated_ms: now,
        })
        .await?;

    let token = CancellationToken::new();
    deps.runtime
        .batch_cancels
        .lock()
        .await
        .insert(bid.clone(), token.clone());
    tokio::spawn(run_admission(
        deps.clone(),
        bid.clone(),
        VecDeque::new(),
        items.into(),
        concurrency,
        token,
        0,
    ));
    Ok(bid)
}

pub async fn batch_status(deps: &BatchDeps, id: &BatchId) -> Result<BatchSummary, BridgeError> {
    let rec = deps
        .detached
        .task_store
        .get_batch(id)
        .await?
        .ok_or(BridgeError::TaskNotFound)?;
    let kids = deps.detached.task_store.batch_children(id).await?;
    let summary = summarize_batch(&rec, &kids);
    if let Some(term) = is_settleable(&summary) {
        deps.detached
            .task_store
            .settle_batch_if_status(id, rec.status, term, now_ms(deps))
            .await?;
        let rec = deps
            .detached
            .task_store
            .get_batch(id)
            .await?
            .ok_or(BridgeError::TaskNotFound)?;
        return Ok(summarize_batch(&rec, &kids));
    }
    Ok(summary)
}

pub async fn batch_list(deps: &BatchDeps, limit: usize) -> Result<Vec<BatchSummary>, BridgeError> {
    let batches = deps.detached.task_store.list_batches(limit).await?;
    let mut out = Vec::with_capacity(batches.len());
    for rec in batches {
        let kids = deps.detached.task_store.batch_children(&rec.id).await?;
        out.push(summarize_batch(&rec, &kids));
    }
    Ok(out)
}

pub async fn cancel_batch(deps: &BatchDeps, id: &BatchId) -> Result<bool, BridgeError> {
    let flipped = deps
        .detached
        .task_store
        .cancel_batch_if_working(id, now_ms(deps))
        .await?;
    if let Some(tok) = deps.runtime.batch_cancels.lock().await.get(id) {
        tok.cancel();
    }
    Ok(flipped)
}

/// Boot-resume entry point: orphan-sweep batch children whose parent is no longer active →
/// `Interrupted`, then resume non-batch tasks (W3b) and the active batches.
///
/// KNOWN LIMITATION (whole-branch review): `resume_all` only runs when `[batch]` is configured
/// (the adapters route here only when `batch_deps()` is `Some`). Booting a DB that holds live
/// batch rows under a config that DROPPED `[batch]` leaves those children `Working` un-swept.
/// Removing `[batch]` while batch rows are in-flight is unsupported; drain or cancel batches
/// before dropping the block.
pub async fn resume_all(deps: &BatchDeps, cap: u32) {
    match deps.detached.task_store.active_batches().await {
        Ok(active) => {
            let active: HashSet<BatchId> = active.into_iter().map(|b| b.id).collect();
            match deps.detached.task_store.working_tasks().await {
                Ok(working) => {
                    for task in working {
                        let Some(bid) = task.batch_id.as_ref() else {
                            continue;
                        };
                        if active.contains(bid) {
                            continue;
                        }
                        if let Err(e) = finalize_detached(
                            &deps.detached.task_store,
                            &deps.detached.progress_hubs,
                            &task.id,
                            TaskRecordStatus::Interrupted,
                            None,
                            Some("orphan batch child"),
                            None,
                        )
                        .await
                        {
                            tracing::warn!(task = task.id.as_str(), batch = bid.as_str(), error = ?e, "batch resume: orphan child sweep failed");
                        } else {
                            tracing::warn!(
                                task = task.id.as_str(),
                                batch = bid.as_str(),
                                "batch resume: interrupted orphan batch child"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "batch resume: working_tasks() failed; skipping orphan sweep");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = ?e, "batch resume: active_batches() failed; skipping orphan sweep");
        }
    }
    crate::detached::resume_non_batch_tasks(&deps.detached, cap).await;
    resume_batches(deps, cap).await;
}

#[derive(serde::Deserialize)]
struct BatchPlanEnvelope {
    v: u32,
    items: Vec<BatchItem>,
}

#[derive(serde::Deserialize)]
struct BatchWorkflowSpecEnvelope {
    v: u32,
    graph: WorkflowGraph,
}

fn decode_batch_plan(raw: &str) -> Option<Vec<BatchItem>> {
    let env: BatchPlanEnvelope = serde_json::from_str(raw).ok()?;
    (env.v == 1).then_some(env.items)
}

async fn cancel_working_children(deps: &BatchDeps, children: &[TaskRecord], reason: &str) {
    for child in children {
        if child.status != TaskRecordStatus::Working {
            continue;
        }
        if let Err(e) = finalize_detached(
            &deps.detached.task_store,
            &deps.detached.progress_hubs,
            &child.id,
            TaskRecordStatus::Canceled,
            None,
            Some(reason),
            None,
        )
        .await
        {
            tracing::warn!(task = child.id.as_str(), error = ?e, "batch resume: cancel child failed");
        }
        deps.detached
            .workflow_cancels
            .lock()
            .await
            .remove(&child.id);
    }
}

async fn resumed_child_future(
    deps: &BatchDeps,
    child: &TaskRecord,
    cap: u32,
    permit: OwnedSemaphorePermit,
) -> Option<(BoxFuture<'static, TaskId>, TaskId, CancellationToken)> {
    let task = child.id.clone();
    let Some(spec_json) = child.workflow_spec_json.as_deref() else {
        let _ = finalize_detached(
            &deps.detached.task_store,
            &deps.detached.progress_hubs,
            &task,
            TaskRecordStatus::Interrupted,
            None,
            Some("not resumable: no workflow snapshot"),
            None,
        )
        .await;
        drop(permit);
        return None;
    };

    let graph = match serde_json::from_str::<BatchWorkflowSpecEnvelope>(spec_json) {
        Ok(env) if env.v == SUPPORTED_SNAPSHOT_VERSION => env.graph,
        _ => {
            let _ = finalize_detached(
                &deps.detached.task_store,
                &deps.detached.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("not resumable: unreadable workflow snapshot"),
                None,
            )
            .await;
            drop(permit);
            return None;
        }
    };

    let cps = match deps.detached.task_store.node_checkpoints(&task).await {
        Ok(cps) => cps,
        Err(e) => {
            tracing::warn!(task = task.as_str(), error = ?e, "batch resume: node_checkpoints() failed; skipping task");
            drop(permit);
            return None;
        }
    };
    let seed: HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)> = cps
        .iter()
        .map(|(node, output, ok, usage)| {
            (
                node.as_str().to_string(),
                (output.clone(), *ok, usage.clone()),
            )
        })
        .collect();

    let terminal_id = match graph.terminal() {
        Some(n) => n.id.as_str().to_string(),
        None => {
            let _ = finalize_detached(
                &deps.detached.task_store,
                &deps.detached.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("not resumable: workflow snapshot has no terminal node"),
                None,
            )
            .await;
            drop(permit);
            return None;
        }
    };
    if let Some((output, ok, _usage)) = seed.get(&terminal_id) {
        let (status, result, error) = if *ok {
            (TaskRecordStatus::Completed, Some(output.as_str()), None)
        } else {
            (TaskRecordStatus::Failed, None, Some(output.as_str()))
        };
        let _ = finalize_detached(
            &deps.detached.task_store,
            &deps.detached.progress_hubs,
            &task,
            status,
            result,
            error,
            None,
        )
        .await;
        drop(permit);
        return None;
    }

    let attempt = match deps
        .detached
        .task_store
        .claim_resume_attempt(&task, cap, deps.detached.clock.now_ms())
        .await
    {
        Ok(ResumeClaim::Exhausted) => {
            let _ = finalize_detached(
                &deps.detached.task_store,
                &deps.detached.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("resume attempt cap exceeded"),
                None,
            )
            .await;
            drop(permit);
            return None;
        }
        Ok(ResumeClaim::Resumable { attempt }) => attempt,
        Err(e) => {
            tracing::warn!(task = task.as_str(), error = ?e, "batch resume: claim_resume_attempt() failed; skipping task");
            drop(permit);
            return None;
        }
    };

    let ctx = match child.session_cwd.as_deref() {
        Some(s) => match SessionCwd::parse(s) {
            Ok(c) => WorkflowRunContext {
                session_cwd: Some(c),
                task_id: Some(task.clone()),
                make_rich_sink: None,
                observer: deps.detached.observer.clone(),
                ..WorkflowRunContext::default()
            },
            Err(_) => {
                let _ = finalize_detached(
                    &deps.detached.task_store,
                    &deps.detached.progress_hubs,
                    &task,
                    TaskRecordStatus::Interrupted,
                    None,
                    Some("not resumable: unreadable session cwd"),
                    None,
                )
                .await;
                drop(permit);
                return None;
            }
        },
        None => WorkflowRunContext {
            task_id: Some(task.clone()),
            observer: deps.detached.observer.clone(),
            ..WorkflowRunContext::default()
        },
    };

    let hub = Arc::new(TaskProgressHub::new());
    deps.detached
        .progress_hubs
        .lock()
        .await
        .insert(task.clone(), hub.clone());
    let token = CancellationToken::new();
    deps.detached
        .workflow_cancels
        .lock()
        .await
        .insert(task.clone(), token.clone());
    let run_id = format!("{}-resume-{}", task.as_str(), attempt);
    let h = spawn_detached_workflow(
        &deps.detached,
        task.clone(),
        child.input.clone(),
        Arc::new(graph),
        run_id,
        token.clone(),
        seed,
        ctx,
        hub,
    );
    let task_id = child.id.clone();
    Some((
        Box::pin(async move {
            let _permit = permit;
            let _ = h.await;
            task
        }),
        task_id,
        token,
    ))
}

pub async fn resume_batches(deps: &BatchDeps, cap: u32) {
    let batches = match deps.detached.task_store.active_batches().await {
        Ok(batches) => batches,
        Err(e) => {
            tracing::warn!(error = ?e, "batch resume: active_batches() failed; skipping batch resume");
            return;
        }
    };

    for batch in batches {
        let bid = batch.id.clone();
        let token = CancellationToken::new();
        deps.runtime
            .batch_cancels
            .lock()
            .await
            .insert(bid.clone(), token.clone());

        let items = match decode_batch_plan(&batch.items_json) {
            Some(items) => items,
            None => {
                if let Ok(children) = deps.detached.task_store.batch_children(&bid).await {
                    cancel_working_children(deps, &children, "corrupt plan").await;
                }
                let _ = deps
                    .detached
                    .task_store
                    .fail_batch_if_status(
                        &bid,
                        batch.status,
                        "corrupt plan",
                        deps.detached.clock.now_ms(),
                    )
                    .await;
                deps.runtime.batch_cancels.lock().await.remove(&bid);
                continue;
            }
        };

        let children = match deps.detached.task_store.batch_children(&bid).await {
            Ok(children) => children,
            Err(e) => {
                tracing::warn!(batch = bid.as_str(), error = ?e, "batch resume: batch_children() failed; skipping batch");
                deps.runtime.batch_cancels.lock().await.remove(&bid);
                continue;
            }
        };
        let existing: HashSet<String> = children.iter().filter_map(|c| c.item_id.clone()).collect();

        if batch.status == BatchStatus::Canceling {
            cancel_working_children(deps, &children, "batch canceled during resume").await;
            let _ = deps
                .detached
                .task_store
                .settle_batch_if_status(
                    &bid,
                    BatchStatus::Canceling,
                    BatchStatus::Canceled,
                    deps.detached.clock.now_ms(),
                )
                .await;
            deps.runtime.batch_cancels.lock().await.remove(&bid);
            continue;
        }

        // Hand the still-Working children to `run_admission` as `to_resume`; it acquires their
        // permits inside its drain-aware loop (so the cap holds on boot WITHOUT the cross-batch
        // deadlock of acquiring inline here) and registers their tokens in `live` (so a later
        // CancelBatch reaches resumed children) — whole-branch review fixes.
        let to_resume: VecDeque<TaskRecord> = children
            .into_iter()
            .filter(|c| c.status == TaskRecordStatus::Working)
            .collect();
        let pending: VecDeque<BatchItem> = items
            .into_iter()
            .filter(|item| !existing.contains(&item.item_id))
            .collect();
        tokio::spawn(run_admission(
            deps.clone(),
            bid,
            to_resume,
            pending,
            batch.concurrency,
            token,
            cap,
        ));
    }
}

/// CAS a still-`Working` batch to `Failed` when its workflow snapshot/graph can no longer be
/// resolved, so a batch with a pending tail does not sit `Working` forever (whole-branch review).
async fn fail_batch_unavailable(deps: &BatchDeps, bid: &BatchId) {
    let _ = deps
        .detached
        .task_store
        .fail_batch_if_status(
            bid,
            BatchStatus::Working,
            "workflow unavailable",
            deps.detached.clock.now_ms(),
        )
        .await;
}

pub async fn run_admission(
    deps: BatchDeps,
    bid: BatchId,
    mut to_resume: VecDeque<TaskRecord>,
    mut pending: VecDeque<BatchItem>,
    concurrency: u32,
    token: CancellationToken,
    cap: u32,
) {
    let mut inflight: FuturesUnordered<BoxFuture<'static, TaskId>> = FuturesUnordered::new();
    let mut live: HashMap<TaskId, CancellationToken> = HashMap::new();
    let mut drain_only = token.is_cancelled();
    let mut cancelled_children = false;
    let mut claim_failed = false;

    loop {
        'admit: while !drain_only
            && !claim_failed
            && inflight.len() < concurrency as usize
            && (!to_resume.is_empty() || !pending.is_empty())
            && !token.is_cancelled()
        {
            let mut queue_guard = QueueAdmissionGuard::waiting(&deps.runtime);

            // Acquire a shared permit, but KEEP DRAINING `inflight` while we wait. Each
            // in-flight child's completion future OWNS its permit and only releases it when
            // polled; if we blocked solely on `acquire_owned()` here, a batch whose
            // `concurrency` exceeds the available shared permits would stop polling its own
            // completions and deadlock the serve-wide cap across batches.
            let permit = loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        drain_only = true;
                        break 'admit;
                    }
                    p = deps.runtime.semaphore.clone().acquire_owned() => {
                        break p.expect("batch semaphore closed");
                    }
                    done = inflight.next(), if !inflight.is_empty() => {
                        if let Some(t) = done {
                            live.remove(&t);
                        }
                    }
                }
            };

            queue_guard.admitted();

            // Resume an existing Working child first (its row already exists; re-run from
            // checkpoints). The permit is owned by the returned future, or dropped inside
            // `resumed_child_future` on a terminal/exhausted short-circuit — so acquiring the
            // permit HERE (inside the drain-aware loop) keeps the cap honored on boot without
            // the cross-batch deadlock of acquiring inline before the loop runs.
            if let Some(child) = to_resume.front().cloned() {
                if let Some((fut, task, ctok)) =
                    resumed_child_future(&deps, &child, cap, permit).await
                {
                    live.insert(task, ctok);
                    let queue_guard = queue_guard;
                    inflight.push(Box::pin(async move {
                        let _queue_guard = queue_guard;
                        fut.await
                    }));
                }
                to_resume.pop_front();
                continue;
            }

            let Some(item) = pending.front().cloned() else {
                drop(permit);
                continue;
            };
            // A missing batch row / unparsable or hot-removed workflow can't be admitted; fail
            // the whole batch (so it doesn't sit Working forever) and stop admitting.
            let workflow_name = match deps.detached.task_store.get_batch(&bid).await {
                Ok(Some(batch)) => batch.workflow,
                _ => {
                    drop(permit);
                    fail_batch_unavailable(&deps, &bid).await;
                    claim_failed = true;
                    break;
                }
            };
            let Ok(workflow) = WorkflowId::parse(workflow_name) else {
                drop(permit);
                fail_batch_unavailable(&deps, &bid).await;
                claim_failed = true;
                break;
            };
            let Some(graph) = deps.detached.workflows.get(&workflow).cloned() else {
                drop(permit);
                fail_batch_unavailable(&deps, &bid).await;
                claim_failed = true;
                break;
            };

            let task = new_detached_task_id();
            let hub = Arc::new(TaskProgressHub::new());
            let ctok = CancellationToken::new();
            deps.detached
                .progress_hubs
                .lock()
                .await
                .insert(task.clone(), hub.clone());
            deps.detached
                .workflow_cancels
                .lock()
                .await
                .insert(task.clone(), ctok.clone());

            let now = deps.detached.clock.now_ms();
            let session_cwd = item
                .session_cwd
                .as_deref()
                .and_then(|raw| SessionCwd::parse(raw).ok());
            let rec = TaskRecord {
                id: task.clone(),
                workflow: workflow.as_str().to_string(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: now,
                updated_ms: now,
                last_artifact_ms: None,
                input: item.input.clone(),
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                resume_attempts: 0,
                session_cwd: session_cwd.as_ref().map(|c| c.as_str().to_string()),
                batch_id: Some(bid.clone()),
                item_id: Some(item.item_id.clone()),
                artifacts_purged_at: None,
            };

            match deps
                .detached
                .task_store
                .claim_batch_child(&bid, &item.item_id, &rec)
                .await
            {
                Ok(ChildClaim::Created) => {
                    let batch_canceling = deps
                        .detached
                        .task_store
                        .get_batch(&bid)
                        .await
                        .ok()
                        .flatten()
                        .map(|b| b.status == BatchStatus::Canceling)
                        .unwrap_or(false);
                    let child_terminal = deps
                        .detached
                        .task_store
                        .get(&task)
                        .await
                        .ok()
                        .flatten()
                        .map(|r| r.status.is_terminal())
                        .unwrap_or(false);
                    if batch_canceling || child_terminal || token.is_cancelled() {
                        let _ = finalize_detached(
                            &deps.detached.task_store,
                            &deps.detached.progress_hubs,
                            &task,
                            TaskRecordStatus::Canceled,
                            None,
                            Some("batch canceled before spawn"),
                            Some(&hub),
                        )
                        .await;
                        deps.detached.workflow_cancels.lock().await.remove(&task);
                        deps.detached.progress_hubs.lock().await.remove(&task);
                        drop(permit);
                        pending.pop_front();
                        if token.is_cancelled() {
                            drain_only = true;
                        }
                        continue;
                    }

                    let h = spawn_detached_workflow(
                        &deps.detached,
                        task.clone(),
                        item.input.clone(),
                        graph,
                        task.as_str().to_string(),
                        ctok.clone(),
                        HashMap::new(),
                        WorkflowRunContext {
                            session_cwd,
                            task_id: Some(task.clone()),
                            make_rich_sink: None,
                            observer: deps.detached.observer.clone(),
                            ..WorkflowRunContext::default()
                        },
                        hub,
                    );
                    live.insert(task.clone(), ctok);
                    let queue_guard = queue_guard;
                    inflight.push(Box::pin(async move {
                        let _permit = permit;
                        let _queue_guard = queue_guard;
                        let _ = h.await;
                        task
                    }));
                    pending.pop_front();
                }
                Ok(ChildClaim::ExistingWorking) | Ok(ChildClaim::ExistingTerminal) => {
                    deps.detached.workflow_cancels.lock().await.remove(&task);
                    deps.detached.progress_hubs.lock().await.remove(&task);
                    drop(permit);
                    pending.pop_front();
                }
                Err(_) => {
                    let _ = deps
                        .detached
                        .task_store
                        .fail_batch_if_status(
                            &bid,
                            BatchStatus::Working,
                            "claim failed",
                            deps.detached.clock.now_ms(),
                        )
                        .await;
                    deps.detached.workflow_cancels.lock().await.remove(&task);
                    deps.detached.progress_hubs.lock().await.remove(&task);
                    drop(permit);
                    claim_failed = true;
                    break;
                }
            }
        }

        if inflight.is_empty()
            && (pending.is_empty() || token.is_cancelled() || drain_only || claim_failed)
        {
            break;
        }

        if drain_only || claim_failed {
            // Cancel in-flight siblings on a batch cancel OR a claim/workflow failure — a
            // failed batch must not leave live children running (whole-branch review).
            if (token.is_cancelled() || claim_failed) && !cancelled_children {
                for t in live.values() {
                    t.cancel();
                }
                cancelled_children = true;
            }
            if let Some(done) = inflight.next().await {
                live.remove(&done);
            } else {
                break;
            }
            continue;
        }

        tokio::select! {
            biased;
            _ = token.cancelled() => {
                for t in live.values() {
                    t.cancel();
                }
                cancelled_children = true;
                drain_only = true;
            }
            done = inflight.next() => {
                if let Some(done) = done {
                    live.remove(&done);
                }
            }
        }
    }

    if (token.is_cancelled() || claim_failed) && !cancelled_children {
        for t in live.values() {
            t.cancel();
        }
    }
    while let Some(done) = inflight.next().await {
        live.remove(&done);
    }

    if let Ok(Some(rec)) = deps.detached.task_store.get_batch(&bid).await {
        if let Ok(children) = deps.detached.task_store.batch_children(&bid).await {
            let s = summarize_batch(&rec, &children);
            if let Some(term) = is_settleable(&s) {
                let _ = deps
                    .detached
                    .task_store
                    .settle_batch_if_status(&bid, s.status, term, deps.detached.clock.now_ms())
                    .await;
            }
        }
    }
    deps.runtime.batch_cancels.lock().await.remove(&bid);
}

/// Pure roll-up over the durable plan (rec.total) + the child rows. The SINGLE owner
/// of the bucket math (RR-FIX-7) -- never re-implemented in a store impl.
pub fn summarize_batch(rec: &BatchRecord, children: &[TaskRecord]) -> BatchSummary {
    let mut ok = 0;
    let mut failed = 0;
    let mut canceled = 0;
    let mut running = 0;
    let mut kids = Vec::with_capacity(children.len());

    for c in children {
        match c.status {
            TaskRecordStatus::Completed => ok += 1,
            TaskRecordStatus::Failed | TaskRecordStatus::Interrupted => failed += 1,
            TaskRecordStatus::Canceled => canceled += 1,
            TaskRecordStatus::Working => running += 1,
        }
        kids.push((
            c.item_id.clone().unwrap_or_default(),
            c.id.clone(),
            c.status,
        ));
    }

    let pending = rec.total.saturating_sub(children.len() as u32);
    BatchSummary {
        id: rec.id.clone(),
        workflow: rec.workflow.clone(),
        status: rec.status,
        total: rec.total,
        ok,
        failed,
        canceled,
        running,
        pending,
        children: kids,
    }
}

/// The SINGLE settle predicate (RR-FIX-8): Working drained -> Completed; Canceling with
/// no running child -> Canceled. Returns the terminal to CAS into, or None.
pub fn is_settleable(s: &BatchSummary) -> Option<BatchStatus> {
    match s.status {
        BatchStatus::Working if s.pending == 0 && s.running == 0 => Some(BatchStatus::Completed),
        BatchStatus::Canceling if s.running == 0 => Some(BatchStatus::Canceled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::detached::DetachedDeps;
    use bridge_core::{
        domain::{AgentEntry, AgentKind, Part, RegistrySnapshot},
        ids::{AgentId, BatchId, NodeId, SessionId, TaskId, WorkflowId},
        ports::{
            AgentBackend, AgentRegistry, BackendStream, Lease, ObsEvent, Observer, Resolved, Update,
        },
        task_store::{
            BatchItem, BatchRecord, BatchStatus, MemoryTaskStore, TaskRecord, TaskRecordStatus,
            TaskStore,
        },
    };
    use bridge_observ::{DropCounter, TurnLogObserver};
    use bridge_workflow::executor::WorkflowExecutor;
    use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tokio::sync::{Barrier, Notify, Semaphore as TokioSemaphore};

    fn br(status: BatchStatus, total: u32) -> BatchRecord {
        BatchRecord {
            id: BatchId::parse("batch-1").unwrap(),
            workflow: "code-review".into(),
            concurrency: 2,
            total,
            status,
            items_json: r#"{"v":1,"items":[]}"#.into(),
            error: None,
            created_ms: 10,
            updated_ms: 10,
        }
    }

    fn child(item: &str, status: TaskRecordStatus) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(format!("task-{item}")).unwrap(),
            workflow: "code-review".into(),
            status,
            result: None,
            error: None,
            created_ms: 10,
            updated_ms: 10,
            last_artifact_ms: None,
            input: format!("input-{item}"),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(BatchId::parse("batch-1").unwrap()),
            item_id: Some(item.into()),
            artifacts_purged_at: None,
        }
    }

    struct NoopLease;
    impl Lease for NoopLease {}

    struct Gate {
        release: Arc<TokioSemaphore>,
        current: AtomicUsize,
        max: AtomicUsize,
        calls: AtomicUsize,
        notify: Notify,
        fail_on: Option<String>,
    }

    impl Gate {
        fn new(fail_on: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                release: Arc::new(TokioSemaphore::new(0)),
                current: AtomicUsize::new(0),
                max: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                notify: Notify::new(),
                fail_on: fail_on.map(str::to_string),
            })
        }

        async fn wait_calls(&self, n: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.calls.load(Ordering::SeqCst) < n {
                assert!(Instant::now() < deadline, "timed out waiting for {n} calls");
                self.notify.notified().await;
            }
        }

        fn release(&self, n: usize) {
            self.release.add_permits(n);
        }
    }

    struct GatedBackend {
        gate: Arc<Gate>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for GatedBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let prompt: String = parts.iter().map(|p| p.text.as_str()).collect();
            let now = self.gate.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.gate.calls.fetch_add(1, Ordering::SeqCst);
            self.gate.notify.notify_waiters();
            loop {
                let max = self.gate.max.load(Ordering::SeqCst);
                if now <= max {
                    break;
                }
                if self
                    .gate
                    .max
                    .compare_exchange(max, now, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break;
                }
            }
            let permit = self.gate.release.acquire().await.unwrap();
            drop(permit);
            self.gate.current.fetch_sub(1, Ordering::SeqCst);
            if self
                .gate
                .fail_on
                .as_ref()
                .map(|needle| prompt.contains(needle))
                .unwrap_or(false)
            {
                return Err(BridgeError::agent_crashed("planned failure"));
            }
            Ok(Box::pin(tokio_stream::iter(vec![
                Ok(Update::Text("ok".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct GatedRegistry {
        gate: Arc<Gate>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for GatedRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(GatedBackend {
                    gate: self.gate.clone(),
                }),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("agent").unwrap()
        }

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![AgentId::parse("agent").unwrap()]
        }
    }

    fn test_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("batch-test").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("agent").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: vec![],
                retry: None,
            }],
            panel: None,
        })
    }

    fn batch_deps(max: u32, gate: Arc<Gate>) -> (BatchDeps, Arc<dyn TaskStore>) {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let graph = test_graph();
        let deps = BatchDeps {
            detached: DetachedDeps {
                task_store: store.clone(),
                executor: Some(Arc::new(WorkflowExecutor::new(Arc::new(GatedRegistry {
                    gate,
                })))),
                workflows: Arc::new(HashMap::from([(graph.id.clone(), graph)])),
                workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
                progress_hubs: Arc::new(Mutex::new(HashMap::new())),
                clock: Arc::new(ManualClock::new(100)),
                observer: Arc::new(NoopObserver),
            },
            runtime: BatchRuntime::new(max, max, Arc::new(NoopObserver)),
            allowed_cwd_root: None,
        };
        (deps, store)
    }

    fn batch_deps_with_turnlog(
        max: u32,
        gate: Arc<Gate>,
    ) -> (BatchDeps, Arc<dyn TaskStore>, Arc<TurnLogObserver>) {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let turnlog = Arc::new(TurnLogObserver::new(
            store.clone(),
            DropCounter::disabled(),
            64,
            Arc::new(|| 1_000),
        ));
        let graph = test_graph();
        let deps = BatchDeps {
            detached: DetachedDeps {
                task_store: store.clone(),
                executor: Some(Arc::new(WorkflowExecutor::new(Arc::new(GatedRegistry {
                    gate,
                })))),
                workflows: Arc::new(HashMap::from([(graph.id.clone(), graph)])),
                workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
                progress_hubs: Arc::new(Mutex::new(HashMap::new())),
                clock: Arc::new(ManualClock::new(100)),
                observer: turnlog.clone(),
            },
            runtime: BatchRuntime::new(max, max, Arc::new(NoopObserver)),
            allowed_cwd_root: None,
        };
        (deps, store, turnlog)
    }

    fn items(n: usize) -> Vec<BatchItem> {
        (0..n)
            .map(|i| BatchItem {
                item_id: format!("item-{i}"),
                input: typed_code_review_input(i),
                session_cwd: None,
            })
            .collect()
    }

    fn typed_code_review_input(i: usize) -> String {
        format!(
            "---\ntask-type: code-review\n---\n# Review item-{i}\n\n## Description\nReview item-{i}.\n\n## Acceptance Criteria\n- Report findings\n"
        )
    }

    fn items_json(items: &[BatchItem]) -> String {
        serde_json::to_string(&json!({"v": 1, "items": items})).unwrap()
    }

    fn batch_record(id: &str, status: BatchStatus, total: u32, concurrency: u32) -> BatchRecord {
        BatchRecord {
            id: BatchId::parse(id).unwrap(),
            workflow: "batch-test".into(),
            concurrency,
            total,
            status,
            items_json: items_json(&items(total as usize)),
            error: None,
            created_ms: 1,
            updated_ms: 1,
        }
    }

    fn batch_child(
        batch: &BatchId,
        item_id: &str,
        status: TaskRecordStatus,
        snapshot: bool,
    ) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(format!("task-{}-{}", batch.as_str(), item_id)).unwrap(),
            workflow: "batch-test".into(),
            status,
            result: (status == TaskRecordStatus::Completed).then(|| "ok".into()),
            error: None,
            created_ms: 1,
            updated_ms: 1,
            last_artifact_ms: None,
            input: format!("input {item_id}"),
            workflow_spec_json: snapshot.then(|| encode_workflow_spec(&test_graph())),
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(batch.clone()),
            item_id: Some(item_id.into()),
            artifacts_purged_at: None,
        }
    }

    async fn wait_batch_status(
        store: &Arc<dyn TaskStore>,
        bid: &BatchId,
        status: BatchStatus,
    ) -> BatchRecord {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(rec) = store.get_batch(bid).await.unwrap() {
                if rec.status == status {
                    return rec;
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for batch status {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_turn_rows_for_task(
        store: &Arc<dyn TaskStore>,
        task: &TaskId,
    ) -> Vec<bridge_core::task_store::TurnLogRow> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let rows = store.turn_log_rows_for_task(task, 16).await.unwrap();
            if !rows.is_empty() {
                return rows;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for turn_log rows for {}",
                task.as_str()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn batch_child_turn_persists_task_id() {
        let gate = Gate::new(None);
        let (deps, store, turnlog) = batch_deps_with_turnlog(1, gate.clone());
        let bid = BatchId::parse("batch-child-owner").unwrap();
        store
            .create_batch(&batch_record(
                "batch-child-owner",
                BatchStatus::Working,
                1,
                1,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;
        gate.wait_calls(1).await;
        gate.release(1);
        wait_batch_status(&store, &bid, BatchStatus::Completed).await;
        turnlog.flush().await;

        let children = store.batch_children(&bid).await.unwrap();
        assert_eq!(children.len(), 1);
        let child_task = children[0].id.clone();
        let rows = wait_turn_rows_for_task(&store, &child_task).await;
        assert!(rows
            .iter()
            .all(|row| row.task_id == Some(child_task.clone())));
    }

    #[tokio::test]
    async fn batch_resume_turn_persists_task_id() {
        let gate = Gate::new(None);
        let (deps, store, turnlog) = batch_deps_with_turnlog(1, gate.clone());
        let bid = BatchId::parse("batch-resume-owner").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-owner",
                BatchStatus::Working,
                1,
                1,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;
        gate.wait_calls(1).await;
        gate.release(1);
        wait_batch_status(&store, &bid, BatchStatus::Completed).await;
        turnlog.flush().await;

        let child_task = TaskId::parse("task-batch-resume-owner-item-0").unwrap();
        let rows = wait_turn_rows_for_task(&store, &child_task).await;
        assert!(rows
            .iter()
            .all(|row| row.task_id == Some(child_task.clone())));
    }

    #[tokio::test]
    async fn batch_status_lazy_settles_dead_loop() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(2, gate);
        let bid = BatchId::parse("batch-lazy-settle").unwrap();
        store
            .create_batch(&batch_record(
                "batch-lazy-settle",
                BatchStatus::Working,
                2,
                2,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Completed,
                false,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-1",
                TaskRecordStatus::Failed,
                false,
            ))
            .await
            .unwrap();

        let summary = batch_status(&deps, &bid).await.unwrap();

        assert_eq!(summary.status, BatchStatus::Completed);
        assert_eq!(
            store.get_batch(&bid).await.unwrap().unwrap().status,
            BatchStatus::Completed
        );
    }

    #[tokio::test]
    async fn cancel_batch_cas_and_fires_token() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(2, gate);
        let bid = BatchId::parse("batch-cancel-token").unwrap();
        store
            .create_batch(&batch_record(
                "batch-cancel-token",
                BatchStatus::Working,
                1,
                1,
            ))
            .await
            .unwrap();
        let token = CancellationToken::new();
        deps.runtime
            .batch_cancels
            .lock()
            .await
            .insert(bid.clone(), token.clone());

        assert!(cancel_batch(&deps, &bid).await.unwrap());
        assert!(token.is_cancelled());
        assert_eq!(
            store.get_batch(&bid).await.unwrap().unwrap().status,
            BatchStatus::Canceling
        );
    }

    #[tokio::test]
    async fn resume_readmits_tail_and_reruns_working_no_double_spawn() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(4, gate.clone());
        let bid = BatchId::parse("batch-resume-readmits").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-readmits",
                BatchStatus::Working,
                4,
                4,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Completed,
                false,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-1",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;
        gate.wait_calls(3).await;
        gate.release(3);
        wait_batch_status(&store, &bid, BatchStatus::Completed).await;

        let children = store.batch_children(&bid).await.unwrap();
        assert_eq!(children.len(), 4);
        let mut counts = HashMap::new();
        for child in children {
            *counts.entry(child.item_id.unwrap()).or_insert(0usize) += 1;
        }
        for item in ["item-0", "item-1", "item-2", "item-3"] {
            assert_eq!(counts.get(item), Some(&1));
        }
    }

    #[tokio::test]
    async fn resume_holds_cap_on_boot() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(3, gate.clone());
        let a = BatchId::parse("batch-resume-cap-a").unwrap();
        let b = BatchId::parse("batch-resume-cap-b").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-cap-a",
                BatchStatus::Working,
                5,
                5,
            ))
            .await
            .unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-cap-b",
                BatchStatus::Working,
                5,
                5,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(&a, "item-0", TaskRecordStatus::Working, true))
            .await
            .unwrap();
        store
            .create(&batch_child(&b, "item-0", TaskRecordStatus::Working, true))
            .await
            .unwrap();

        let deps_for_resume = deps.clone();
        let h = tokio::spawn(async move {
            resume_batches(&deps_for_resume, 3).await;
        });
        gate.wait_calls(3).await;
        assert_eq!(gate.current.load(Ordering::SeqCst), 3);
        assert!(gate.max.load(Ordering::SeqCst) <= 3);
        gate.release(20);
        h.await.unwrap();
        wait_batch_status(&store, &a, BatchStatus::Completed).await;
        wait_batch_status(&store, &b, BatchStatus::Completed).await;
        assert!(gate.max.load(Ordering::SeqCst) <= 3);
    }

    #[tokio::test]
    async fn resume_canceling_cancels_children_admits_no_tail_settles_canceled() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(3, gate.clone());
        let bid = BatchId::parse("batch-resume-canceling").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-canceling",
                BatchStatus::Canceling,
                3,
                3,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;

        let rec = store.get_batch(&bid).await.unwrap().unwrap();
        let children = store.batch_children(&bid).await.unwrap();
        assert_eq!(rec.status, BatchStatus::Canceled);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].status, TaskRecordStatus::Canceled);
        assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn resume_sweeps_orphan_working_child_to_interrupted() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(1, gate);
        let bid = BatchId::parse("batch-resume-orphan").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-orphan",
                BatchStatus::Completed,
                1,
                1,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_all(&deps, 3).await;

        let child = store
            .batch_children(&bid)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(child.status, TaskRecordStatus::Interrupted);
    }

    #[tokio::test]
    async fn resume_corrupt_plan_fails_batch() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(1, gate);
        let bid = BatchId::parse("batch-resume-corrupt").unwrap();
        let mut rec = batch_record("batch-resume-corrupt", BatchStatus::Working, 1, 1);
        rec.items_json = r#"{"v":99,"items":[]}"#.into();
        store.create_batch(&rec).await.unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;

        let rec = store.get_batch(&bid).await.unwrap().unwrap();
        let child = store
            .batch_children(&bid)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(rec.status, BatchStatus::Failed);
        assert_eq!(rec.error.as_deref(), Some("corrupt plan"));
        assert_eq!(child.status, TaskRecordStatus::Canceled);
    }

    // Whole-branch review: seeded Working children EXCEEDING the cap must not deadlock boot
    // resume (the permit is acquired inside run_admission's drain-aware loop, not inline).
    #[tokio::test]
    async fn resume_seeded_working_over_cap_does_not_deadlock() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(1, gate.clone()); // cap 1, two Working children
        let bid = BatchId::parse("batch-resume-overcap").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-overcap",
                BatchStatus::Working,
                2,
                2,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-1",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;
        gate.wait_calls(1).await;
        assert!(gate.max.load(Ordering::SeqCst) <= 1);
        gate.release(20);
        // Completes (does not hang) and the cap held throughout.
        wait_batch_status(&store, &bid, BatchStatus::Completed).await;
        assert!(gate.max.load(Ordering::SeqCst) <= 1);
    }

    // Whole-branch review: a resumed child must be registered in `live` so a later CancelBatch
    // reaches it (without the fix, run_admission started with an empty `live`).
    #[tokio::test]
    async fn cancel_batch_cancels_resumed_children() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(3, gate.clone());
        let bid = BatchId::parse("batch-resume-cancelable").unwrap();
        store
            .create_batch(&batch_record(
                "batch-resume-cancelable",
                BatchStatus::Working,
                1,
                1,
            ))
            .await
            .unwrap();
        store
            .create(&batch_child(
                &bid,
                "item-0",
                TaskRecordStatus::Working,
                true,
            ))
            .await
            .unwrap();

        resume_batches(&deps, 3).await;
        gate.wait_calls(1).await; // the resumed child is in-flight, parked on the gate
        assert!(cancel_batch(&deps, &bid).await.unwrap());
        // The resumed child's token (in `live`) fires → it cancels → batch settles Canceled,
        // WITHOUT ever releasing the gate.
        wait_batch_status(&store, &bid, BatchStatus::Canceled).await;
    }

    // Whole-branch review: a resumed batch whose workflow is no longer registered must CAS to
    // Failed (not sit Working forever).
    #[tokio::test]
    async fn resume_missing_workflow_fails_batch() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(3, gate.clone());
        let bid = BatchId::parse("batch-missing-wf").unwrap();
        store
            .create_batch(&BatchRecord {
                id: bid.clone(),
                workflow: "no-such-workflow".into(),
                concurrency: 2,
                total: 1,
                status: BatchStatus::Working,
                items_json: r#"{"v":1,"items":[{"item_id":"item-0","input":"x"}]}"#.into(),
                error: None,
                created_ms: 1,
                updated_ms: 1,
            })
            .await
            .unwrap();

        resume_batches(&deps, 3).await;
        wait_batch_status(&store, &bid, BatchStatus::Failed).await;
    }

    #[tokio::test]
    async fn batch_caps_within_one_batch() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(2, gate.clone());
        let bid = run_batch(
            &deps,
            BatchParams {
                workflow: "batch-test".into(),
                concurrency: Some(2),
                items: items(5),
            },
        )
        .await
        .unwrap();

        gate.wait_calls(2).await;
        assert_eq!(gate.current.load(Ordering::SeqCst), 2);
        assert!(gate.max.load(Ordering::SeqCst) <= 2);
        gate.release(2);
        gate.wait_calls(4).await;
        assert!(gate.max.load(Ordering::SeqCst) <= 2);
        gate.release(8);

        wait_batch_status(&store, &bid, BatchStatus::Completed).await;
        assert!(gate.max.load(Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn run_batch_rejects_untyped_item() {
        let gate = Gate::new(None);
        let (deps, _store) = batch_deps(2, gate);

        match run_batch(
            &deps,
            BatchParams {
                workflow: "batch-test".into(),
                concurrency: Some(1),
                items: vec![BatchItem {
                    item_id: "item-0".into(),
                    input: "bare batch item".into(),
                    session_cwd: None,
                }],
            },
        )
        .await
        {
            Err(BridgeError::TaskSpecInvalid { .. }) => {}
            Err(other) => panic!("expected TaskSpecInvalid, got {other:?}"),
            Ok(id) => panic!("expected TaskSpecInvalid, got Ok({id:?})"),
        }
    }

    #[tokio::test]
    async fn serve_wide_cap_across_two_batches() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(3, gate.clone());
        let a = run_batch(
            &deps,
            BatchParams {
                workflow: "batch-test".into(),
                concurrency: Some(4),
                items: items(5),
            },
        )
        .await
        .unwrap();
        let b = run_batch(
            &deps,
            BatchParams {
                workflow: "batch-test".into(),
                concurrency: Some(4),
                items: items(5),
            },
        )
        .await
        .unwrap();

        gate.wait_calls(3).await;
        assert_eq!(gate.current.load(Ordering::SeqCst), 3);
        assert!(gate.max.load(Ordering::SeqCst) <= 3);
        gate.release(20);
        wait_batch_status(&store, &a, BatchStatus::Completed).await;
        wait_batch_status(&store, &b, BatchStatus::Completed).await;
        assert!(gate.max.load(Ordering::SeqCst) <= 3);
    }

    #[tokio::test]
    async fn continue_others_one_failure_does_not_abort() {
        let gate = Gate::new(Some("item-2"));
        let (deps, store) = batch_deps(3, gate.clone());
        let bid = run_batch(
            &deps,
            BatchParams {
                workflow: "batch-test".into(),
                concurrency: Some(3),
                items: items(3),
            },
        )
        .await
        .unwrap();

        gate.wait_calls(3).await;
        gate.release(3);
        let rec = wait_batch_status(&store, &bid, BatchStatus::Completed).await;
        let children = store.batch_children(&bid).await.unwrap();
        let summary = summarize_batch(&rec, &children);
        assert_eq!((summary.ok, summary.failed), (2, 1));
    }

    #[tokio::test]
    async fn cancel_batch_wakes_loop_blocked_on_permit() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(1, gate);
        let bid = BatchId::parse("batch-blocked").unwrap();
        store
            .create_batch(&BatchRecord {
                id: bid.clone(),
                workflow: "batch-test".into(),
                concurrency: 1,
                total: 1,
                status: BatchStatus::Working,
                items_json: r#"{"v":1,"items":[]}"#.into(),
                error: None,
                created_ms: 1,
                updated_ms: 1,
            })
            .await
            .unwrap();
        let held = deps
            .runtime
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let token = CancellationToken::new();
        let h = tokio::spawn(run_admission(
            deps,
            bid.clone(),
            VecDeque::new(),
            VecDeque::from(items(1)),
            1,
            token.clone(),
            0,
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), h)
            .await
            .expect("admission should wake on batch token")
            .unwrap();
        drop(held);
        assert!(store.batch_children(&bid).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn suppressed_spawn_is_durably_finalized() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(1, gate);
        let bid = BatchId::parse("batch-suppressed").unwrap();
        store
            .create_batch(&BatchRecord {
                id: bid.clone(),
                workflow: "batch-test".into(),
                concurrency: 1,
                total: 1,
                status: BatchStatus::Canceling,
                items_json: r#"{"v":1,"items":[]}"#.into(),
                error: None,
                created_ms: 1,
                updated_ms: 1,
            })
            .await
            .unwrap();

        run_admission(
            deps,
            bid.clone(),
            VecDeque::new(),
            VecDeque::from(items(1)),
            1,
            CancellationToken::new(),
            0,
        )
        .await;

        let children = store.batch_children(&bid).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].status, TaskRecordStatus::Canceled);
        assert_eq!(
            store.get_batch(&bid).await.unwrap().unwrap().status,
            BatchStatus::Canceled
        );
    }

    #[tokio::test]
    async fn claim_existing_working_is_not_respawned() {
        let gate = Gate::new(None);
        let (deps, store) = batch_deps(1, gate.clone());
        let bid = BatchId::parse("batch-existing").unwrap();
        store
            .create_batch(&BatchRecord {
                id: bid.clone(),
                workflow: "batch-test".into(),
                concurrency: 1,
                total: 1,
                status: BatchStatus::Working,
                items_json: r#"{"v":1,"items":[]}"#.into(),
                error: None,
                created_ms: 1,
                updated_ms: 1,
            })
            .await
            .unwrap();
        store
            .create(&TaskRecord {
                id: TaskId::parse("task-existing").unwrap(),
                workflow: "batch-test".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: "input item-0".into(),
                workflow_spec_json: Some(encode_workflow_spec(&test_graph())),
                resume_attempts: 0,
                session_cwd: None,
                batch_id: Some(bid.clone()),
                item_id: Some("item-0".into()),
                artifacts_purged_at: None,
            })
            .await
            .unwrap();

        run_admission(
            deps,
            bid.clone(),
            VecDeque::new(),
            VecDeque::from(items(1)),
            1,
            CancellationToken::new(),
            0,
        )
        .await;

        assert_eq!(gate.calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.batch_children(&bid).await.unwrap().len(), 1);
    }

    #[test]
    fn summary_buckets_every_task_status() {
        let rec = br(BatchStatus::Working, 5);
        let kids = vec![
            child("a", TaskRecordStatus::Completed),
            child("b", TaskRecordStatus::Failed),
            child("c", TaskRecordStatus::Canceled),
            child("d", TaskRecordStatus::Interrupted),
            child("e", TaskRecordStatus::Working),
        ];
        let s = summarize_batch(&rec, &kids);
        assert_eq!(
            (s.ok, s.failed, s.canceled, s.running, s.pending),
            (1, 2, 1, 1, 0)
        );
    }

    #[test]
    fn pending_is_total_minus_rows() {
        let rec = br(BatchStatus::Working, 4);
        let kids = vec![child("a", TaskRecordStatus::Completed)];
        assert_eq!(summarize_batch(&rec, &kids).pending, 3);
    }

    #[test]
    fn is_settleable_working_completes_only_when_drained() {
        let rec = br(BatchStatus::Working, 2);
        let done = vec![
            child("a", TaskRecordStatus::Completed),
            child("b", TaskRecordStatus::Failed),
        ];
        assert_eq!(
            is_settleable(&summarize_batch(&rec, &done)),
            Some(BatchStatus::Completed)
        );
        let running = vec![
            child("a", TaskRecordStatus::Working),
            child("b", TaskRecordStatus::Completed),
        ];
        assert_eq!(is_settleable(&summarize_batch(&rec, &running)), None);
    }

    #[test]
    fn is_settleable_canceling_settles_when_no_running() {
        let rec = br(BatchStatus::Canceling, 3);
        let kids = vec![
            child("a", TaskRecordStatus::Canceled),
            child("b", TaskRecordStatus::Completed),
        ];
        assert_eq!(
            is_settleable(&summarize_batch(&rec, &kids)),
            Some(BatchStatus::Canceled)
        );
    }

    #[derive(Default)]
    struct QueueRecorder(std::sync::Mutex<Vec<(u64, u64, bool)>>);

    impl Observer for QueueRecorder {
        fn record(&self, e: &ObsEvent<'_>) {
            if let ObsEvent::QueueChanged {
                in_flight,
                queued,
                wait,
            } = e
            {
                self.0.lock().expect("queue recorder mutex poisoned").push((
                    *in_flight,
                    *queued,
                    wait.is_some(),
                ));
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

    #[tokio::test]
    async fn concurrent_queue_guard_transitions_are_ordered_and_final_state_matches() {
        let observer = Arc::new(QueueRecorder::default());
        let runtime = BatchRuntime::new(2, 1, observer.clone());
        let rounds = 250usize;
        let admit_sync = Arc::new(Barrier::new(2));
        let drop_sync = Arc::new(Barrier::new(2));

        let p1 = {
            let runtime = runtime.clone();
            let admit_sync = admit_sync.clone();
            let drop_sync = drop_sync.clone();
            tokio::spawn(async move {
                for _ in 0..rounds {
                    let mut guard = QueueAdmissionGuard::waiting(&runtime);
                    admit_sync.wait().await;
                    guard.admitted();
                    tokio::task::yield_now().await;
                    drop_sync.wait().await;
                    drop(guard);
                }
            })
        };
        let p2 = {
            let runtime = runtime.clone();
            let admit_sync = admit_sync.clone();
            let drop_sync = drop_sync.clone();
            tokio::spawn(async move {
                for _ in 0..rounds {
                    let mut guard = QueueAdmissionGuard::waiting(&runtime);
                    admit_sync.wait().await;
                    guard.admitted();
                    tokio::task::yield_now().await;
                    drop_sync.wait().await;
                    drop(guard);
                }
            })
        };

        p1.await.unwrap();
        p2.await.unwrap();

        let events = observer.0.lock().unwrap().clone();
        assert_eq!(events.len(), rounds * 6);
        assert_eq!(
            events.iter().filter(|(_, _, wait)| *wait).count(),
            rounds * 2
        );

        for (in_flight, queued, _) in &events {
            assert!(*in_flight <= 2, "in_flight must stay bounded");
            assert!(*queued <= 2, "queued must stay bounded");
            assert!(*in_flight + *queued <= 2);
            assert!((*in_flight, *queued) != (1, 2));
        }

        let last = events
            .last()
            .expect("queue recorder should have final emission");
        let counts = runtime
            .queue_counts
            .lock()
            .expect("queue count mutex should not be poisoned");
        assert_eq!(last, &(0, 0, false));
        assert_eq!((counts.queued, counts.in_flight), (0, 0));
        assert_eq!((last.0, last.1), (counts.in_flight, counts.queued));
    }
}
