use bridge_workflow::executor::WorkflowRunContext;

#[test]
fn downstream_exhaustive_workflow_context_literal_remains_source_compatible() {
    let defaults = WorkflowRunContext::default();
    let _context = WorkflowRunContext {
        session_cwd: None,
        make_rich_sink: None,
        observer: defaults.observer,
        parent_traceparent: None,
        task_id: None,
        prompt_id: None,
    };
}
