# EventLog::events_since

`EventLog` stores a task's ordered events with 1-based monotonic `seq` numbers.
When an SSE subscriber reconnects it sends its `Last-Event-ID` (the seq of the
last event it already received) and the server replays what it missed.

Contract:
- `events_since(cursor)` returns exactly the events the subscriber has NOT seen:
  those with `seq` strictly greater than `cursor`.
- The event whose `seq == cursor` was already delivered and must NOT be replayed
  (exactly-once delivery across a reconnect).

The change adds `events_since` for the reattach path.
