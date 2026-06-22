use crate::domain::PermitDecision;
use crate::ids::{ContextId, OperationId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// Per-turn metadata threaded onto the ACP route so the reverse permission handler can build a gen-stamped key.
#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub context_id: ContextId,
    pub generation: u64,
    pub op: OperationId,
}

/// Gen+op-keyed identity of one pending permission rendezvous.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermKey {
    pub context_id: ContextId,
    pub generation: u64,
    pub op: OperationId,
    pub request_id: String,
}

/// The value sent through the pending oneshot. `Cancelled` is broadcast by resolve_context (Task 3/4).
#[derive(Debug)]
pub enum PermissionResolution {
    Decided(PermitDecision),
    Cancelled,
}

/// One offered permission option, surfaced to the operator via session/status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionOptionView {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// What `session/status` shows for a pending permission (Task 8 reads this from the registry).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingPermissionView {
    pub request_id: String,
    pub tool_call_id: String,
    pub generation: u64,
    pub op: OperationId,
    pub title: String,
    pub options: Vec<PermissionOptionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<String>,
    pub timeout_ms: u64,
}

struct PendingEntry {
    sender: oneshot::Sender<PermissionResolution>,
    view: PendingPermissionView,
}

/// Gen+op-keyed rendezvous for interactive permissions. Exact-once: a key resolves at most once
/// (remove-then-send under one lock). Shared as `Arc<PermissionRegistry>` into AcpBackend + SessionManager.
#[derive(Default)]
pub struct PermissionRegistry {
    inner: Mutex<HashMap<PermKey, PendingEntry>>,
}

/// Reaps its key on Drop (no-op if already resolved). The interactive handler holds it across the await so
/// EVERY exit (decision / timeout / cancel / responder-fail / task-drop) removes the entry.
pub struct PermitGuard {
    reg: Arc<PermissionRegistry>,
    key: PermKey,
}

impl Drop for PermitGuard {
    fn drop(&mut self) {
        self.reg.reap(&self.key);
    }
}

impl PermissionRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a pending permission. Returns the receiver the handler awaits + a guard that reaps on Drop.
    pub fn register(
        self: &Arc<Self>,
        key: PermKey,
        view: PendingPermissionView,
    ) -> (oneshot::Receiver<PermissionResolution>, PermitGuard) {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .lock()
            .expect("permission registry lock")
            .insert(key.clone(), PendingEntry { sender, view });
        (
            receiver,
            PermitGuard {
                reg: Arc::clone(self),
                key,
            },
        )
    }

    /// Resolve exactly one key (operator decision). Atomic take-under-lock -> no double-send / send-after-take.
    /// Returns true iff the key existed AND the receiver was still listening.
    pub fn resolve(&self, key: &PermKey, res: PermissionResolution) -> bool {
        let entry = self
            .inner
            .lock()
            .expect("permission registry lock")
            .remove(key);
        match entry {
            Some(entry) => entry.sender.send(res).is_ok(),
            None => false,
        }
    }

    /// Cancel ALL pending permissions for a context (cancel / clear / release). Constructs `Cancelled` per send
    /// (so `PermissionResolution` needs NO `Clone` bound). Returns the count cancelled.
    pub fn resolve_context_cancelled(&self, ctx: &ContextId) -> usize {
        let mut guard = self.inner.lock().expect("permission registry lock");
        let keys: Vec<PermKey> = guard
            .keys()
            .filter(|key| &key.context_id == ctx)
            .cloned()
            .collect();
        let mut count = 0;
        for key in keys {
            if let Some(entry) = guard.remove(&key) {
                let _ = entry.sender.send(PermissionResolution::Cancelled);
                count += 1;
            }
        }
        count
    }

    /// Remove without sending - the drop-guard path AND the Escalate no-op (Task 6).
    pub fn reap(&self, key: &PermKey) {
        self.inner
            .lock()
            .expect("permission registry lock")
            .remove(key);
    }

    /// Snapshot the pending permission views for a context (session/status reads this - Task 8).
    pub fn pending(&self, ctx: &ContextId) -> Vec<PendingPermissionView> {
        self.inner
            .lock()
            .expect("permission registry lock")
            .iter()
            .filter(|(key, _)| &key.context_id == ctx)
            .map(|(_, entry)| entry.view.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: &str, generation: u64, op: &str, request_id: &str) -> PermKey {
        PermKey {
            context_id: ContextId::parse(c).unwrap(),
            generation,
            op: OperationId::parse(op).unwrap(),
            request_id: request_id.into(),
        }
    }

    fn approve() -> PermitDecision {
        PermitDecision::Approve { option_id: None }
    }

    fn view(request_id: &str) -> PendingPermissionView {
        PendingPermissionView {
            request_id: request_id.into(),
            tool_call_id: format!("tool-{request_id}"),
            generation: 1,
            op: OperationId::parse("turn-1").unwrap(),
            title: format!("permission {request_id}"),
            options: vec![PermissionOptionView {
                option_id: "approved".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            }],
            raw_input: None,
            timeout_ms: 120_000,
        }
    }

    #[test]
    fn pending_view_round_trips() {
        let v = PendingPermissionView {
            request_id: "r".into(),
            tool_call_id: "t".into(),
            generation: 1,
            op: OperationId::parse("turn-1").unwrap(),
            title: "write /tmp/x".into(),
            options: vec![PermissionOptionView {
                option_id: "approved".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            }],
            raw_input: None,
            timeout_ms: 120_000,
        };
        let s = serde_json::to_string(&v).unwrap();
        let _back: PendingPermissionView = serde_json::from_str(&s).unwrap();
    }

    #[tokio::test]
    async fn register_resolve_exactly_once() {
        let reg = PermissionRegistry::new();
        let k = key("c", 1, "turn-1", "r");
        let (rx, _g) = reg.register(k.clone(), view("r"));
        assert!(reg.resolve(&k, PermissionResolution::Decided(approve())));
        assert!(
            !reg.resolve(&k, PermissionResolution::Decided(approve())),
            "second resolve no-ops"
        );
        assert!(matches!(
            rx.await.unwrap(),
            PermissionResolution::Decided(_)
        ));
    }

    #[tokio::test]
    async fn resolve_context_cancels_all_for_ctx() {
        let reg = PermissionRegistry::new();
        let (rx1, _g1) = reg.register(key("c", 1, "turn-1", "r1"), view("r1"));
        let (rx2, _g2) = reg.register(key("c", 1, "turn-1", "r2"), view("r2"));
        assert_eq!(
            reg.resolve_context_cancelled(&ContextId::parse("c").unwrap()),
            2
        );
        assert!(matches!(
            rx1.await.unwrap(),
            PermissionResolution::Cancelled
        ));
        assert!(matches!(
            rx2.await.unwrap(),
            PermissionResolution::Cancelled
        ));
    }

    #[tokio::test]
    async fn stale_generation_permit_rejected() {
        let reg = PermissionRegistry::new();
        let (_rx, _g) = reg.register(key("c", 2, "turn-2", "r"), view("r"));
        assert!(!reg.resolve(
            &key("c", 1, "turn-1", "r"),
            PermissionResolution::Decided(approve())
        ));
    }

    #[tokio::test]
    async fn drop_guard_reaps_on_handler_exit() {
        let reg = PermissionRegistry::new();
        let k = key("c", 1, "turn-1", "r");
        {
            let (_rx, _g) = reg.register(k.clone(), view("r"));
        }
        assert!(
            !reg.resolve(&k, PermissionResolution::Decided(approve())),
            "reaped on guard drop"
        );
        assert!(reg.pending(&ContextId::parse("c").unwrap()).is_empty());
    }

    #[tokio::test]
    async fn pending_lists_views_for_ctx() {
        let reg = PermissionRegistry::new();
        let (_rx, _g) = reg.register(key("c", 1, "turn-1", "r"), view("r"));
        let p = reg.pending(&ContextId::parse("c").unwrap());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].request_id, "r");
    }
}
