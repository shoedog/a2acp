use bridge_workflow::executor::WorkflowRunContext;

#[test]
fn downstream_exhaustive_workflow_context_literal_sets_harvest_store() {
    let defaults = WorkflowRunContext::default();
    let _context = WorkflowRunContext {
        session_cwd: None,
        make_rich_sink: None,
        observer: defaults.observer,
        parent_traceparent: None,
        task_id: None,
        prompt_id: None,
        harvest_audit_store: std::sync::Arc::new(bridge_core::harvest::NoopHarvestAuditStore),
    };
}
