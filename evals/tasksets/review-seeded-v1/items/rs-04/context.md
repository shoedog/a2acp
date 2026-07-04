# SlotTable::evict

`SlotTable` tracks warm-session slot usage. `used` gates admission, and it must
stay consistent with a durable ledger (`persist_free`) that survives restarts.

Contract:
- The in-memory `used` count and the durable ledger must not diverge.
- `persist_free` is fallible; if it fails, the slot has NOT been freed durably,
  so `used` must not be decremented as if it had (otherwise the two accountings
  drift and capacity is silently lost/leaked).

The change adds `evict(id)` to remove a session and return its slot to the pool.
