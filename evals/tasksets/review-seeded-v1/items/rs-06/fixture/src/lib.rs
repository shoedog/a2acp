#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub text: String,
}

/// A durable per-task event log. Seq numbers are 1-based and monotonic.
pub struct EventLog {
    events: Vec<Event>,
}

impl EventLog {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn append(&mut self, text: &str) {
        let seq = self.events.len() as u64 + 1;
        self.events.push(Event {
            seq,
            text: text.to_string(),
        });
    }

    /// Return the events a reconnecting subscriber has NOT seen yet. `cursor`
    /// is the seq of the last event it received (its Last-Event-ID). Only
    /// strictly-newer events should be replayed.
    pub fn events_since(&self, cursor: u64) -> Vec<Event> {
        self.events
            .iter()
            .filter(|e| e.seq >= cursor)
            .cloned()
            .collect()
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}
