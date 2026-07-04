# SessionManager::dispatch

`SessionManager` keeps warm sessions in a `tokio::sync::Mutex<HashMap<..>>`
keyed by session id. `dispatch(id, prompt)` runs one agent turn for a session
and records the turn count.

Contract:
- Many sessions dispatch concurrently through one shared `SessionManager`.
- The sessions map lock protects only the map; it must NOT be held across the
  agent round-trip, so one slow turn cannot block every other session.

The change refactors `dispatch` to look up the session and record its turn
count "in place" instead of taking the map lock twice.
