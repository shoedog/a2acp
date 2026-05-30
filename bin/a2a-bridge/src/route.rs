// route.rs — Composition-root routing: always routes inbound tasks to the Kiro backend.
// Routing lives in the binary, NOT in bridge-policy (spec §8, Task 15).

use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::RouteDecision;

/// v1 routing policy: every task is handled by the local Kiro backend.
pub struct AlwaysKiro;

impl RouteDecision for AlwaysKiro {
    fn route(&self, _meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        Ok(RouteTarget::Local(AgentId::parse("kiro")?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{RouteTarget, TaskMeta};
    use bridge_core::ports::RouteDecision;

    #[test]
    fn always_routes_to_kiro() {
        let r = AlwaysKiro.route(&TaskMeta::default()).unwrap();
        assert!(matches!(r, RouteTarget::Local(a) if a.as_str() == "kiro"));
    }
}
