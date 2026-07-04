use std::collections::HashMap;
use std::sync::Mutex;

pub struct Session {
    pub id: String,
}

pub struct Manager {
    sessions: Mutex<HashMap<String, Session>>,
    spawned: Mutex<u32>,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            spawned: Mutex::new(0),
        }
    }

    /// Spawn a fresh agent-backed session (expensive; also bumps a counter).
    fn spawn(&self, id: &str) -> Session {
        *self.spawned.lock().unwrap() += 1;
        Session { id: id.to_string() }
    }

    /// Return the session for `id`, spawning it on first use.
    pub fn get_or_spawn(&self, id: &str) {
        if !self.sessions.lock().unwrap().contains_key(id) {
            let session = self.spawn(id);
            self.sessions.lock().unwrap().insert(id.to_string(), session);
        }
    }

    pub fn spawn_count(&self) -> u32 {
        *self.spawned.lock().unwrap()
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}
