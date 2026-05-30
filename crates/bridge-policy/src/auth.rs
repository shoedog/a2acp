// auth.rs — AlwaysGrant: returns AuthContext with caller_id "anonymous" for every request.

use bridge_core::domain::{AuthContext, InboundRequest};
use bridge_core::error::BridgeError;
use bridge_core::ids::CallerId;
use bridge_core::ports::AuthMiddleware;

pub struct AlwaysGrant;

impl AuthMiddleware for AlwaysGrant {
    fn authorize(&self, _req: &InboundRequest) -> Result<AuthContext, BridgeError> {
        Ok(AuthContext::new(
            CallerId::parse("anonymous").expect("nonempty"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::InboundRequest;
    use bridge_core::ports::AuthMiddleware;

    #[test]
    fn grants_anonymous_context_for_anon_request() {
        let ctx = AlwaysGrant.authorize(&InboundRequest::anon()).unwrap();
        assert_eq!(ctx.caller_id().as_str(), "anonymous");
    }

    #[test]
    fn grants_for_tokened_request_too() {
        let ctx = AlwaysGrant
            .authorize(&InboundRequest::with_token("tok"))
            .unwrap();
        assert_eq!(ctx.caller_id().as_str(), "anonymous"); // v1 always-grant ignores token
    }
}
