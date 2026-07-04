# Manager::dispatch

`Manager` stores per-session prompts in a `tokio::sync::Mutex<HashMap>`.
`dispatch(id)` runs the session's stored prompt against the agent.

Contract:
- Many sessions dispatch concurrently through one shared `Manager`.
- The prompts map lock must NOT be held across the agent round-trip, so one slow
  turn cannot block every other session's access to the map.

The change adds `dispatch`, which reads the stored prompt and runs the turn.
