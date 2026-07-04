# DeltaBuffer

`DeltaBuffer` retains a turn's recently streamed deltas so a late-attaching SSE
subscriber can replay them. `push` is called on every delta from the agent;
`take` drains the buffer when a subscriber pulls.

Contract:
- Only the most recent deltas need retaining -- the buffer MUST be bounded (a
  ring of at most `CAP = 1024` deltas); older deltas are dropped.
- A detached run may have NO subscriber, so `take` is never called on that path;
  the bound is what keeps memory finite regardless of whether anyone drains.

The change simplifies the internal storage of `DeltaBuffer`.
