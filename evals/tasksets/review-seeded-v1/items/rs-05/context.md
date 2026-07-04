# Manager::get_or_spawn

`Manager` lazily spawns one agent-backed `Session` per id, cached in a
`Mutex<HashMap>`. `get_or_spawn(id)` is called concurrently from many request
handlers sharing one `Manager`.

Contract:
- At most ONE session is ever spawned per id (spawning is expensive and starts
  a real agent process).
- Concurrent `get_or_spawn(id)` calls for the same id must collapse to a single
  spawn; the cached session is authoritative.

The change adds `get_or_spawn`, which checks the cache and spawns on a miss.
