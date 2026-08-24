use bridge_workflow::cancellation_settlement as s;

fn f(value: s::WorkflowNodeCancellationSettlementV1<s::PreservationRequiredV1>) {
    value.into_disposition();
}

fn main() {}
