use std::collections::HashMap;
use tokio::sync::Mutex;

pub struct Session {
    pub id: String,
    pub turns: u32,
}

pub struct Agent;

impl Agent {
    /// One ACP round-trip against the backing agent process. In the real bridge
    /// this can take many seconds.
    pub async fn run_turn(&self, _session_id: &str, _prompt: &str) -> String {
        String::from("reply")
    }
}

pub struct SessionManager {
    sessions: Mutex<HashMap<String, Session>>,
    agent: Agent,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            agent: Agent,
        }
    }

    pub async fn open(&self, id: &str) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            id.to_string(),
            Session {
                id: id.to_string(),
                turns: 0,
            },
        );
    }

    /// Dispatch one turn for `id`: look up the session, run the turn, record it.
    pub async fn dispatch(&self, id: &str, prompt: &str) -> Option<String> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions.get_mut(id)?;
        // Run the turn while we hold the session so we can record it in place.
        let reply = self.agent.run_turn(id, prompt).await;
        session.turns += 1;
        Some(reply)
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
