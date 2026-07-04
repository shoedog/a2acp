#![allow(async_fn_in_trait)]

#[derive(Debug)]
pub struct StoreError;

/// A durable checkpoint store. Each write is an independent await (a separate
/// round-trip to the backing database).
pub trait Checkpoints {
    /// Mark `node` complete so a resume will SKIP it.
    async fn mark_complete(&self, node: &str) -> Result<(), StoreError>;
    /// Persist the node's output for downstream nodes to read on resume.
    async fn put_output(&self, node: &str, body: &str) -> Result<(), StoreError>;
}

pub struct Runner<S: Checkpoints> {
    pub store: S,
}

impl<S: Checkpoints> Runner<S> {
    /// Checkpoint a finished node: record its output and mark it done so a
    /// crash-resume skips it and downstream nodes can read the output.
    pub async fn checkpoint(&self, node: &str, body: &str) -> Result<(), StoreError> {
        self.store.mark_complete(node).await?;
        self.store.put_output(node, body).await?;
        Ok(())
    }
}
