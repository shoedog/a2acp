use std::collections::HashMap;
use std::sync::Arc;

use bridge_core::domain::{EffectiveConfig, QueuedInject};
use bridge_core::ids::{ContextId, OperationId, SessionGeneration, SessionId, TaskId};
use bridge_core::ports::{AgentBackend, Lease};
use tokio::sync::Mutex;

use crate::session_manager::SessionManager;

/// A task's binding to its resolved registry instance, created on the FIRST local
/// message. Holds the backend driving the task, the effective config applied to its
/// session, and the registry [`Lease`] keeping the slot alive for the task's
/// lifetime. The lease drops when the binding is removed from the map ([`BindingGuard`]
/// eviction on producer exit), decrementing the slot's active-task count.
pub struct TaskBinding {
    pub backend: Arc<dyn AgentBackend>,
    /// The effective config applied to the task's session — reused by binding-driven
    /// follow-ups (they prompt the bound backend without recomputing config). Kept on
    /// the binding so the resolved config is available for the task's whole lifetime.
    pub eff: EffectiveConfig,
    /// The registry lease keeping the slot alive for the task. Dropped (releasing the
    /// slot's active-task count) when the binding is removed on producer exit.
    pub lease: Box<dyn Lease>,
}

/// RAII eviction guard owned by a task's producer. While alive it represents the
/// task's binding; on `Drop` — whether the producer returns cleanly (Done/Failed/
/// Canceled) OR early (client disconnect / error) — it removes the [`TaskBinding`]
/// from the map (dropping the [`Lease`] → the slot's active-task count decrements)
/// and forgets the backend's per-session stash. This is the spec-critical
/// "eviction on EVERY producer exit": a leaked lease keeps a slot un-retirable
/// forever, so the guard must fire on the non-clean paths a manual cleanup might miss.
///
/// `Drop` is synchronous but the eviction is async (mutex lock + `forget_session`),
/// so it is performed on a spawned task. A follow-up that REUSES an existing binding
/// does NOT own a guard — only the FIRST message's producer evicts.
pub struct BindingGuard {
    pub bindings: Arc<Mutex<HashMap<TaskId, TaskBinding>>>,
    pub task: TaskId,
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
}

impl Drop for BindingGuard {
    fn drop(&mut self) {
        let bindings = self.bindings.clone();
        let task = self.task.clone();
        let session = self.session.clone();
        let backend = self.backend.clone();
        // Note: the spawn-in-Drop pattern means an eviction enqueued during runtime
        // shutdown may not run (the Tokio runtime may be torn down before the task
        // executes), leaving the binding and lease un-evicted. This is acceptable for
        // a single-process bridge that is exiting anyway.
        tokio::spawn(async move {
            // Take the binding out of the map and drop its Lease explicitly → the
            // slot's active-task count decrements. Then forget the per-session stash.
            if let Some(binding) = bindings.lock().await.remove(&task) {
                drop(binding.lease);
            }
            backend.forget_session(&session).await;
        });
    }
}

/// The local backend ready to drive a task, plus its RAII eviction guard. A
/// FIRST-message dispatch returns `Some(guard)` (the producer owns it → evicts the
/// binding/lease/stash on exit); a FOLLOW-UP that reused an existing binding returns
/// `None` (the original producer owns eviction — a follow-up must not evict a still-
/// live binding when its own short-lived call ends).
pub struct LocalDispatch {
    pub backend: Arc<dyn AgentBackend>,
    /// The session to prompt against — warm `ctx-…` session, or legacy `session-{task}`.
    pub session: SessionId,
    /// Warm-session summary seed to prepend to the prompt parts, when present.
    pub seed: Option<String>,
    /// Warm-session context injects to apply to the next prompt parts.
    pub injects: Vec<QueuedInject>,
    pub guard: Option<BindingGuard>,
    /// Warm path only: finishes the warm turn (→ Idle) on drop. Mutually exclusive with `guard`.
    pub warm_guard: Option<WarmTurnGuard>,
    /// Per-turn abort token (cancel-tokens F2). For a WARM turn it is the handle's `turn_abort`
    /// (a force-reset cancels it → the producer's biased select aborts before re-minting the released
    /// session). For a cold-bind dispatch it is a fresh, never-cancelled token (no warm handle to race).
    pub abort: tokio_util::sync::CancellationToken,
}

/// Drops the warm turn back to Idle on producer exit (mirrors BindingGuard::Drop's spawn pattern).
pub struct WarmTurnGuard {
    pub sm: Arc<SessionManager>,
    pub ctx: ContextId,
    pub generation: SessionGeneration,
    pub op: OperationId,
}

impl Drop for WarmTurnGuard {
    fn drop(&mut self) {
        let sm = self.sm.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        let op = self.op.clone();
        tokio::spawn(async move {
            sm.finish_turn(&ctx, generation, &op).await;
        });
    }
}
