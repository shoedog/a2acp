# run_turn (warm turn driver)

`run_turn` accumulates an agent's streamed deltas until the stream ends, while
also watching a cancel channel. It is one warm producer in the bridge's turn
loop.

Contract:
- Cancellation MUST take effect immediately (abort-first): when a cancel is
  pending, the turn must stop before draining another delta.
- The delta stream ending (`recv() == None`) completes the turn normally.

The change adds `run_turn` using a `tokio::select!` over the cancel and delta
channels.
