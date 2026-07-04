use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A fixed pool of warm-agent slots. `available` is the number of free slots.
pub struct Pool {
    available: AtomicUsize,
}

impl Pool {
    pub fn new(size: usize) -> Arc<Self> {
        Arc::new(Self {
            available: AtomicUsize::new(size),
        })
    }

    pub fn available(&self) -> usize {
        self.available.load(Ordering::SeqCst)
    }

    /// Take a slot. Returns a `Lease` that frees the slot when dropped.
    pub fn checkout(self: &Arc<Self>) -> Option<Lease> {
        if self.available.fetch_sub(1, Ordering::SeqCst) == 0 {
            self.available.fetch_add(1, Ordering::SeqCst);
            return None;
        }
        Some(Lease {
            pool: self.clone(),
        })
    }

    fn release(&self) {
        self.available.fetch_add(1, Ordering::SeqCst);
    }

    /// Called when a turn finishes to return its slot to the pool.
    pub fn complete(self: &Arc<Self>, lease: Lease) {
        self.release();
        drop(lease);
    }
}

/// Holds a slot; frees it back to the pool on drop (RAII).
pub struct Lease {
    pool: Arc<Pool>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.pool.release();
    }
}
