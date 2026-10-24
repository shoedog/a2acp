//! The sole prompt-usage capture and budget admission boundary.
use crate::{
    accounting::PreparedUsageAttemptV1, attestation::*, domain::*, error::BridgeError,
    governance::BudgetAdmissionOutcomeV1, ids::*, ports::*,
};
use futures::StreamExt;
use std::sync::Arc;

pub struct AccountingBackendV1 {
    inner: Arc<dyn AgentBackend>,
}
impl AccountingBackendV1 {
    pub fn new(inner: Arc<dyn AgentBackend>) -> Self {
        Self { inner }
    }
}
#[async_trait::async_trait]
impl AgentBackend for AccountingBackendV1 {
    async fn prompt(&self, _: &SessionId, _: Vec<Part>) -> Result<BackendStream, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
    async fn prompt_accounted_v1(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        mut attempt: PreparedUsageAttemptV1,
        observers: Option<BackendObservers>,
    ) -> Result<BackendStream, BridgeError> {
        let admission = match attempt.take_permit() {
            Some(permit) => BudgetAdmissionOutcomeV1::Admitted(permit),
            None => attempt.admit().await?,
        };
        match admission {
            BudgetAdmissionOutcomeV1::Ungoverned => attempt.update(true, None, false).await?,
            BudgetAdmissionOutcomeV1::Admitted(permit) => {
                // Consuming the single-use permit authorizes exactly this inner prompt.
                // Admission already committed dispatch intent atomically with liability.
                let _reservation = permit.into_reservation();
            }
            BudgetAdmissionOutcomeV1::Refused(mut refusal) => {
                refusal.persistence = attempt.preparation_failed().await.err().map(Box::new);
                return Err(BridgeError::BudgetRefused {
                    refusal: Box::new(refusal),
                });
            }
        }
        let result = match observers {
            Some(observers) => {
                self.inner
                    .prompt_with_observers(session, parts, observers)
                    .await
            }
            None => self.inner.prompt(session, parts).await,
        };
        // Errors can follow acceptance; only Done affirms provider settlement.
        let mut stream = result?;
        Ok(Box::pin(async_stream::try_stream! {
            while let Some(item) = stream.next().await {
                match &item {
                    Ok(Update::Usage(usage)) => attempt.update(false, Some(usage.clone()), false).await?,
                    Ok(Update::Done { .. }) => {
                        attempt.update(false, None, true).await?;
                        yield item?;
                        return;
                    }
                    _ => {}
                }
                yield item?;
            }
            // An unterminated provider stream is interrupted, never finalized by inference.
            Err(BridgeError::MissingTerminal)?;
        }))
    }
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.inner.cancel(session).await
    }
    async fn cancel_observed(
        &self,
        session: &SessionId,
        observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.inner.cancel_observed(session, observer).await
    }
    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        self.inner.resource_flight_v1()
    }
    fn attach_resource_flight_owner_v1(
        &self,
        session: &SessionId,
    ) -> Result<BackendResourceFlightV1, BridgeError> {
        self.inner.attach_resource_flight_owner_v1(session)
    }
    fn transfer_cleanup_deadline_v1(
        &self,
        session: &SessionId,
        reason: crate::resource_flight::BoundedRecoveryReasonV1,
    ) -> Result<crate::retained_resource_flight::CleanupDeadlineTransferV1, BridgeError> {
        self.inner.transfer_cleanup_deadline_v1(session, reason)
    }
    fn prefix_attestation_capability(&self) -> PrefixAttestationCapability {
        self.inner.prefix_attestation_capability()
    }
    fn terminal_evidence_capability(&self) -> crate::terminal_evidence::EvidenceCapability {
        self.inner.terminal_evidence_capability()
    }
    fn bridge_owned_acp_child_liveness(&self) -> crate::terminal_evidence::AcpChildLiveness {
        self.inner.bridge_owned_acp_child_liveness()
    }
    async fn configure_turn(&self, session: &SessionId, meta: crate::permission::TurnMeta) {
        self.inner.configure_turn(session, meta).await
    }
    async fn configure_turn_liveness(
        &self,
        session: &SessionId,
        liveness: crate::turn_liveness::TurnLivenessRequestV1,
    ) {
        self.inner.configure_turn_liveness(session, liveness).await
    }
    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &crate::domain::SessionSpec,
    ) -> Result<(), BridgeError> {
        self.inner.configure_session(session, spec).await
    }
    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &crate::execution_policy::BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        self.inner.configure_bound_session(session, spec).await
    }
    fn session_catalog(&self, session: &SessionId) -> Option<crate::catalog::AgentCaps> {
        self.inner.session_catalog(session)
    }
    async fn forget_session(&self, session: &SessionId) {
        self.inner.forget_session(session).await
    }
    async fn forget_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.inner.forget_session_checked(session).await
    }
    async fn forget_session_observed(
        &self,
        session: &SessionId,
        observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.inner.forget_session_observed(session, observer).await
    }
    async fn release_session(&self, session: &SessionId) {
        self.inner.release_session(session).await
    }
    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.inner.release_session_checked(session).await
    }
    async fn release_session_observed(
        &self,
        session: &SessionId,
        observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.inner.release_session_observed(session, observer).await
    }
    async fn preserve_checkout_v1(
        &self,
        session: &SessionId,
        reason: CheckoutPreservationReasonV1,
    ) -> CheckoutPreservationV1 {
        self.inner.preserve_checkout_v1(session, reason).await
    }
    async fn settle_workflow_checkout_v1(
        &self,
        session: &SessionId,
        outcome: WorkflowCheckoutOutcomeV1,
    ) -> CheckoutSettlementV1 {
        self.inner
            .settle_workflow_checkout_v1(session, outcome)
            .await
    }
    async fn reconcile_config(
        &self,
        session: &SessionId,
        spec: &crate::domain::SessionSpec,
    ) -> Result<crate::orch::ReconcileOutcome, BridgeError> {
        self.inner.reconcile_config(session, spec).await
    }
    fn capabilities(&self) -> crate::orch::AgentSessionCaps {
        self.inner.capabilities()
    }
    async fn set_host_custody(&self, held: bool) -> Result<(), BridgeError> {
        self.inner.set_host_custody(held).await
    }
    async fn retire(&self) -> Result<(), BridgeError> {
        self.inner.retire().await
    }
}
