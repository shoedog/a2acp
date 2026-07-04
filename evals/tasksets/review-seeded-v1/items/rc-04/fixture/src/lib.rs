use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Debug)]
pub struct NodeResult {
    pub node: String,
    pub output: String,
}

/// Collects one result per workflow node. The channel is intentionally
/// unbounded: a workflow DAG is loaded once and its node set is FIXED and small
/// (validated `<= MAX_NODES` at load), so the producer set sends at most
/// `nodes` messages total -- the queue can never grow beyond that.
pub struct Collector {
    tx: Sender<NodeResult>,
    rx: Receiver<NodeResult>,
    nodes: usize,
}

pub const MAX_NODES: usize = 64;

impl Collector {
    /// `nodes` is the DAG's validated node count (`<= MAX_NODES`).
    pub fn new(nodes: usize) -> Self {
        debug_assert!(nodes <= MAX_NODES);
        let (tx, rx) = channel();
        Self { tx, rx, nodes }
    }

    /// A handle each node uses to report its single result exactly once.
    pub fn sender(&self) -> Sender<NodeResult> {
        self.tx.clone()
    }

    /// Gather the results. At most `nodes` were ever sent.
    pub fn collect(self) -> Vec<NodeResult> {
        drop(self.tx);
        self.rx.into_iter().take(self.nodes).collect()
    }
}
