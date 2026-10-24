//! Local workflow coverage and the durable prompt dispatch barrier.
use bridge_coordinator::detached::AttemptTelemetryState;
use bridge_core::{error::BridgeError, ids::AttemptId, workflow_history::WorkflowHistoryStore};
use bridge_store::sqlite::SqliteStore;
use std::sync::Arc;

pub async fn establish_coverage(store: &SqliteStore) -> Result<(), BridgeError> {
    // Initialize the selected sidecar before reserving the initial workflow owner.
    bridge_core::task_store::TaskStore::usage_summary_v1(store, &Default::default()).await?;
    Ok(())
}

pub fn dispatch_barrier(
    store: Arc<SqliteStore>,
    attempt: AttemptId,
    telemetry: AttemptTelemetryState,
) -> bridge_workflow::executor::PromptDispatchBarrier {
    Arc::new(move || {
        let (store, attempt, telemetry) = (store.clone(), attempt.clone(), telemetry.clone());
        Box::pin(async move {
            if let Err(error) = store
                .mark_prompt_acceptance(&attempt, "dispatch_uncertain")
                .await
            {
                telemetry.record(error.reason);
                eprintln!("telemetry_unavailable{{reason={}}}", error.reason.as_str());
            }
        })
    })
}
