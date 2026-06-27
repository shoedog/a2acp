use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use bridge_core::error::BridgeError;
use bridge_core::ids::{BatchId, TaskId, WorkflowId};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::task_store::{
    BatchItem, BatchRecord, BatchStatus, BatchSummary, ChildClaim, ResumeClaim, TaskRecord,
    TaskRecordStatus,
};
use bridge_workflow::executor::WorkflowRunContext;
use bridge_workflow::graph::WorkflowGraph;
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use serde_json::json;
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
    pub batch_cancels: Arc<Mutex<HashMap<BatchId, CancellationToken>>>,
}

impl BatchRuntime {
    pub fn new(max_concurrent: u32, default_concurrency: u32) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
            default_concurrency,
            max_concurrent,
            batch_cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
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
        if !seen.insert(item.item_id.clone()) {
            return Err(BridgeError::InvalidRequest { field: "item_id" });
        }
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
        items.into(),
        FuturesUnordered::new(),
        concurrency,
        token,
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
) -> Option<BoxFuture<'static, TaskId>> {
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
                make_rich_sink: None,
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
        None => WorkflowRunContext::default(),
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
        token,
        seed,
        ctx,
        hub,
    );
    Some(Box::pin(async move {
        let _permit = permit;
        let _ = h.await;
        task
    }))
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

        let inflight: FuturesUnordered<BoxFuture<'static, TaskId>> = FuturesUnordered::new();
        for child in children
            .iter()
            .filter(|c| c.status == TaskRecordStatus::Working)
        {
            let permit = deps
                .runtime
                .semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("batch semaphore closed");
            if let Some(fut) = resumed_child_future(deps, child, cap, permit).await {
                inflight.push(fut);
            }
        }

        let pending: VecDeque<BatchItem> = items
            .into_iter()
            .filter(|item| !existing.contains(&item.item_id))
            .collect();
        tokio::spawn(run_admission(
            deps.clone(),
            bid,
            pending,
            inflight,
            batch.concurrency,
            token,
        ));
    }
}

pub async fn run_admission(
    deps: BatchDeps,
    bid: BatchId,
    mut pending: VecDeque<BatchItem>,
    mut inflight: FuturesUnordered<BoxFuture<'static, TaskId>>,
    concurrency: u32,
    token: CancellationToken,
) {
    let mut live: HashMap<TaskId, CancellationToken> = HashMap::new();
    let mut drain_only = token.is_cancelled();
    let mut cancelled_children = false;
    let mut claim_failed = false;

    loop {
        'admit: while !drain_only
            && !claim_failed
            && inflight.len() < concurrency as usize
            && !pending.is_empty()
            && !token.is_cancelled()
        {
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

            let Some(item) = pending.front().cloned() else {
                drop(permit);
                continue;
            };
            let workflow_name = match deps.detached.task_store.get_batch(&bid).await {
                Ok(Some(batch)) => batch.workflow,
                _ => {
                    drop(permit);
                    claim_failed = true;
                    break;
                }
            };
            let Ok(workflow) = WorkflowId::parse(workflow_name) else {
                drop(permit);
                claim_failed = true;
                break;
            };
            let Some(graph) = deps.detached.workflows.get(&workflow).cloned() else {
                drop(permit);
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
                input: item.input.clone(),
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                resume_attempts: 0,
                session_cwd: session_cwd.as_ref().map(|c| c.as_str().to_string()),
                batch_id: Some(bid.clone()),
                item_id: Some(item.item_id.clone()),
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
                            make_rich_sink: None,
                        },
                        hub,
                    );
                    live.insert(task.clone(), ctok);
                    inflight.push(Box::pin(async move {
                        let _permit = permit;
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
            if token.is_cancelled() && !cancelled_children {
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

    if token.is_cancelled() && !cancelled_children {
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
        ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update},
        task_store::{
            BatchItem, BatchRecord, BatchStatus, MemoryTaskStore, TaskRecord, TaskRecordStatus,
            TaskStore,
        },
    };
    use bridge_workflow::executor::WorkflowExecutor;
    use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tokio::sync::{Notify, Semaphore as TokioSemaphore};

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
            input: format!("input-{item}"),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(BatchId::parse("batch-1").unwrap()),
            item_id: Some(item.into()),
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
            },
            runtime: BatchRuntime::new(max, max),
            allowed_cwd_root: None,
        };
        (deps, store)
    }

    fn items(n: usize) -> Vec<BatchItem> {
        (0..n)
            .map(|i| BatchItem {
                item_id: format!("item-{i}"),
                input: format!("input item-{i}"),
                session_cwd: None,
            })
            .collect()
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
            input: format!("input {item_id}"),
            workflow_spec_json: snapshot.then(|| encode_workflow_spec(&test_graph())),
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(batch.clone()),
            item_id: Some(item_id.into()),
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
            VecDeque::from(items(1)),
            FuturesUnordered::new(),
            1,
            token.clone(),
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
            VecDeque::from(items(1)),
            FuturesUnordered::new(),
            1,
            CancellationToken::new(),
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
                input: "input item-0".into(),
                workflow_spec_json: Some(encode_workflow_spec(&test_graph())),
                resume_attempts: 0,
                session_cwd: None,
                batch_id: Some(bid.clone()),
                item_id: Some("item-0".into()),
            })
            .await
            .unwrap();

        run_admission(
            deps,
            bid.clone(),
            VecDeque::from(items(1)),
            FuturesUnordered::new(),
            1,
            CancellationToken::new(),
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
}
