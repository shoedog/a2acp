// task.rs — Task lifecycle typestate machine (spec §5.2).
// Illegal state transitions (e.g. resume on Terminal) fail to compile, not at runtime.

use crate::ids::TaskId;

/// Runtime-inspectable task state.
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    AuthRequired,
    Completed,
    Failed,
    Canceled,
}

// Phantom typestate markers — zero-size, never instantiated directly by callers.
pub struct Submitted;
pub struct Working;
pub struct InputRequired;
pub struct AuthRequired;
/// Terminal marker — deliberately has NO resume/transition methods.
pub struct Terminal;

/// A task in state `S`. `S` is a phantom type encoding the lifecycle position.
pub struct Task<S> {
    id: TaskId,
    runtime_state: TaskState,
    _s: std::marker::PhantomData<S>,
}

impl<S> Task<S> {
    pub fn state(&self) -> &TaskState {
        &self.runtime_state
    }

    pub fn id(&self) -> &TaskId {
        &self.id
    }
}

fn mk<S>(id: TaskId, st: TaskState) -> Task<S> {
    Task {
        id,
        runtime_state: st,
        _s: std::marker::PhantomData,
    }
}

impl Task<Submitted> {
    pub fn submitted(id: TaskId) -> Self {
        mk(id, TaskState::Submitted)
    }

    pub fn start(self) -> Task<Working> {
        mk(self.id, TaskState::Working)
    }
}

impl Task<Working> {
    pub fn suspend_input(self, _request_id: String) -> Task<InputRequired> {
        mk(self.id, TaskState::InputRequired)
    }

    pub fn suspend_auth(self, _request_id: String) -> Task<AuthRequired> {
        mk(self.id, TaskState::AuthRequired)
    }

    pub fn complete(self) -> Task<Terminal> {
        mk(self.id, TaskState::Completed)
    }

    pub fn fail(self) -> Task<Terminal> {
        mk(self.id, TaskState::Failed)
    }

    pub fn cancel(self) -> Task<Terminal> {
        mk(self.id, TaskState::Canceled)
    }
}

impl Task<InputRequired> {
    pub fn resume(self) -> Task<Working> {
        mk(self.id, TaskState::Working)
    }
}

impl Task<AuthRequired> {
    pub fn resume(self) -> Task<Working> {
        mk(self.id, TaskState::Working)
    }
}

// NOTE: Task<Terminal> intentionally has NO methods.
// Attempting to call .resume() on it is a compile error — proven by trybuild tests.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TaskId;

    #[test]
    fn input_required_resumes_to_working() {
        let t = Task::submitted(TaskId::parse("t").unwrap()).start(); // Working
        let suspended = t.suspend_input("r".into()); // InputRequired
        let resumed = suspended.resume(); // -> Working
        assert!(matches!(resumed.state(), TaskState::Working));
    }

    #[test]
    fn auth_required_resumes_to_working() {
        let t = Task::submitted(TaskId::parse("t").unwrap()).start();
        let resumed = t.suspend_auth("r".into()).resume();
        assert!(matches!(resumed.state(), TaskState::Working));
    }

    #[test]
    fn completed_is_terminal() {
        let done = Task::submitted(TaskId::parse("t").unwrap())
            .start()
            .complete();
        assert!(matches!(done.state(), TaskState::Completed));
    }

    #[test]
    fn id_is_preserved_through_transitions() {
        let t = Task::submitted(TaskId::parse("abc").unwrap()).start();
        assert_eq!(t.id().as_str(), "abc");
    }

    #[test]
    fn fail_and_cancel_transitions() {
        let t = Task::submitted(TaskId::parse("t").unwrap()).start();
        let failed = t.fail();
        assert!(matches!(failed.state(), TaskState::Failed));

        let t2 = Task::submitted(TaskId::parse("t2").unwrap()).start();
        let cancelled = t2.cancel();
        assert!(matches!(cancelled.state(), TaskState::Canceled));
    }
}
