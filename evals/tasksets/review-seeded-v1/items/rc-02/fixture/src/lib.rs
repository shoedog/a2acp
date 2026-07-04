use std::collections::HashMap;
use tokio::sync::Mutex;

pub struct Agent;

impl Agent {
    /// One ACP round-trip; slow (seconds) in the real bridge.
    pub async fn run_turn(&self, _prompt: &str) -> String {
        String::from("ok")
    }
}

pub struct Manager {
    prompts: Mutex<HashMap<String, String>>,
    agent: Agent,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            prompts: Mutex::new(HashMap::new()),
            agent: Agent,
        }
    }

    pub async fn set_prompt(&self, id: &str, prompt: &str) {
        self.prompts
            .lock()
            .await
            .insert(id.to_string(), prompt.to_string());
    }

    /// Run the session's stored prompt against the agent.
    pub async fn dispatch(&self, id: &str) -> Option<String> {
        // Clone the prompt out and let the map lock drop at the end of this
        // block, BEFORE the slow agent round-trip, so other sessions are not
        // serialized behind the map lock while this turn runs.
        let prompt = {
            let prompts = self.prompts.lock().await;
            prompts.get(id)?.clone()
        };
        Some(self.agent.run_turn(&prompt).await)
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}
