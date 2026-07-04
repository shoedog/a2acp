use std::collections::HashMap;

/// A runtime-mutable registry of agent commands, keyed by agent id.
pub struct Registry {
    agents: HashMap<String, String>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register `id` with its launch command and return a handle to the stored
    /// command string.
    pub fn register(&mut self, id: &str, cmd: &str) -> &str {
        self.agents.insert(id.to_string(), cmd.to_string());
        // SAFETY/INVARIANT: we inserted `id` on the line above and hold `&mut
        // self`, so no other code could have removed it before this lookup; the
        // get is infallible here.
        self.agents.get(id).map(String::as_str).unwrap()
    }

    pub fn get(&self, id: &str) -> Option<&str> {
        self.agents.get(id).map(String::as_str)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
