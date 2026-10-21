//! Independent provider observations for workflow accounting contract tests.
//! No executor/dispatcher activation or production backend construction is implied.
use bridge_core::{
    domain::{EffectiveConfig, Part, SessionSpec},
    error::BridgeError,
    ids::SessionId,
    orch::UsageSnapshot,
    ports::{AgentBackend, BackendStream, Update},
};
use futures::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
pub struct WorkflowRecording {
    pub prompts: Mutex<Vec<(SessionId, Vec<Part>)>>,
    pub applied: Mutex<Vec<(SessionId, EffectiveConfig)>>,
    pub usage: Arc<Mutex<Vec<(SessionId, UsageSnapshot)>>>,
    pub reject_config: Mutex<Option<BridgeError>>,
    pub interrupt: AtomicBool,
}

#[async_trait::async_trait]
impl AgentBackend for WorkflowRecording {
    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        if let Some(error) = self.reject_config.lock().unwrap().clone() {
            return Err(error);
        }
        self.applied
            .lock()
            .unwrap()
            .push((session.clone(), spec.config.clone()));
        Ok(())
    }

    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompts.lock().unwrap().push((session.clone(), parts));
        let mut updates = vec![
            Ok(Update::Usage(crate::support::usage())),
            Ok(Update::Text("answer".into())),
        ];
        if !self.interrupt.load(Ordering::SeqCst) {
            updates.push(Ok(Update::done("end_turn")));
        }
        let usage = self.usage.clone();
        let session = session.clone();
        Ok(Box::pin(futures::stream::iter(updates).map(move |item| {
            if let Ok(Update::Usage(snapshot)) = &item {
                usage
                    .lock()
                    .unwrap()
                    .push((session.clone(), snapshot.clone()));
            }
            item
        })))
    }

    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// Exercise only the core capture boundary, with no optional observer installed.
pub async fn dispatch(
    backend: Arc<WorkflowRecording>,
    prepared: bridge_core::translator::PreparedAccountingTurnV1,
    session: &SessionId,
    parts: Vec<Part>,
) -> Result<Vec<Update>, BridgeError> {
    let captured = bridge_core::accounting_backend::AccountingBackendV1::new(backend);
    let stream = captured
        .prompt_accounted_v1(session, parts, prepared.into_attempt(), None)
        .await?;
    use futures::TryStreamExt;
    stream.try_collect().await
}
