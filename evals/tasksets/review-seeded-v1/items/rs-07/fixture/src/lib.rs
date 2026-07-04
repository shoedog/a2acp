use std::sync::Mutex;

pub struct Delta {
    pub seq: u64,
    pub text: String,
}

/// Buffers a turn's streamed deltas so a late-attaching SSE subscriber can
/// replay recent output. Only the most recent deltas need to be retained.
pub struct DeltaBuffer {
    pending: Mutex<Vec<Delta>>,
}

impl DeltaBuffer {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Record one streamed delta. Called on every token/delta from the agent.
    pub fn push(&self, delta: Delta) {
        let mut pending = self.pending.lock().unwrap();
        pending.push(delta);
    }

    /// A subscriber drains everything buffered so far.
    pub fn take(&self) -> Vec<Delta> {
        let mut pending = self.pending.lock().unwrap();
        std::mem::take(&mut *pending)
    }

    pub fn len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for DeltaBuffer {
    fn default() -> Self {
        Self::new()
    }
}
