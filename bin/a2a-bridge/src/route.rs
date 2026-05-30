// route.rs — Composition-root routing: always routes inbound tasks to the Kiro backend.
// Routing lives in the binary, NOT in bridge-policy (spec §8, Task 15).

use bridge_core::domain::TaskMeta;
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::RouteDecision;

/// v1 routing policy: every task is handled by the local Kiro backend.
pub struct AlwaysKiro;

impl RouteDecision for AlwaysKiro {
    fn route(&self, _meta: &TaskMeta) -> Result<AgentId, BridgeError> {
        AgentId::parse("kiro")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::TaskMeta;
    use bridge_core::ports::RouteDecision;

    #[test]
    fn always_routes_to_kiro() {
        assert_eq!(AlwaysKiro.route(&TaskMeta).unwrap().as_str(), "kiro");
    }
}
