// session.rs — Session lifecycle typestate machine (spec §5.3).
// send_prompt exists ONLY on Session<Ready>; attempts on other states are compile errors.

use crate::domain::{Part, PromptOutcome};
use crate::ids::SessionId;

// Phantom typestate markers.
pub struct Spawned;
pub struct Initialized;
pub struct Ready;

/// A session in state `S`. `S` is a phantom type encoding the lifecycle position.
pub struct Session<S> {
    id: SessionId,
    _s: std::marker::PhantomData<S>,
}

impl<S> Session<S> {
    pub fn id(&self) -> &SessionId {
        &self.id
    }
}

fn mk<S>(id: SessionId) -> Session<S> {
    Session {
        id,
        _s: std::marker::PhantomData,
    }
}

impl Session<Spawned> {
    pub fn spawned(id: SessionId) -> Self {
        mk(id)
    }

    pub fn initialize(self) -> Session<Initialized> {
        mk(self.id)
    }
}

impl Session<Initialized> {
    pub fn ready(self) -> Session<Ready> {
        mk(self.id)
    }
}

impl Session<Ready> {
    /// Send a prompt — only available on Session<Ready>.
    /// Returns a `PromptOutcome` placeholder and the session back (resumable).
    /// The streaming translator (Task 11) drives the real flow.
    pub fn send_prompt(self, _parts: Vec<Part>) -> (PromptOutcome, Session<Ready>) {
        (PromptOutcome, mk(self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SessionId;

    #[test]
    fn ready_session_can_prompt() {
        let s = Session::spawned(SessionId::parse("s").unwrap())
            .initialize()
            .ready();
        let (_outcome, _back) = s.send_prompt(vec![]);
        // compiles only because send_prompt exists on Session<Ready>
    }

    #[test]
    fn session_id_accessor() {
        let s = Session::spawned(SessionId::parse("my-session").unwrap());
        assert_eq!(s.id().as_str(), "my-session");
    }
}
