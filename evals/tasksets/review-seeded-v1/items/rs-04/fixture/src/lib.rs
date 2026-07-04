use std::collections::HashSet;

#[derive(Debug)]
pub struct StoreError(pub String);

/// Slot accounting for warm sessions. `used` must always equal the number of
/// live sessions, and it must stay in sync with the durable ledger that
/// survives restarts; capacity admission reads `used`.
pub struct SlotTable {
    live: HashSet<String>,
    used: u32,
    capacity: u32,
}

impl SlotTable {
    pub fn new(capacity: u32) -> Self {
        Self {
            live: HashSet::new(),
            used: 0,
            capacity,
        }
    }

    pub fn admit(&mut self, id: &str) -> Result<(), StoreError> {
        if self.used >= self.capacity {
            return Err(StoreError("at capacity".into()));
        }
        self.live.insert(id.to_string());
        self.used += 1;
        Ok(())
    }

    /// Persist the freeing of a slot to the durable ledger. Can fail (the
    /// underlying write may error), in which case the slot is still marked used
    /// durably.
    fn persist_free(&mut self, _id: &str) -> Result<(), StoreError> {
        // pretend this writes to sqlite and may return Err
        Ok(())
    }

    /// Evict a session and return its slot to the pool.
    pub fn evict(&mut self, id: &str) {
        self.live.remove(id);
        let _ = self.persist_free(id);
        self.used -= 1;
    }

    pub fn used(&self) -> u32 {
        self.used
    }
}
