use std::sync::Arc;

use bridge_acp::acp_backend::ContainerReap;

#[test]
fn public_literal_remains_source_compatible_for_downstream_crates() {
    let reap_fn: bridge_core::reaper::ReapFn = Arc::new(|_, _| {});
    let container = ContainerReap {
        runtime: "docker".into(),
        name: "a2a-ro-source-compat".into(),
        reap_fn,
    };

    assert_eq!(container.runtime, "docker");
    assert_eq!(container.name, "a2a-ro-source-compat");
}
