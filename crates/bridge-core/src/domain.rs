// domain.rs — minimal shared domain value types (spec §5.2/§5.3).
// Task 4 will extend this file with richer payload types.

#[derive(Debug, Default, Clone)]
pub struct Part;

#[derive(Debug, Default, Clone)]
pub struct Artifact;

#[derive(Debug, Default, Clone)]
pub struct PromptOutcome;

#[derive(Debug, Default, Clone)]
pub struct TaskMeta;
