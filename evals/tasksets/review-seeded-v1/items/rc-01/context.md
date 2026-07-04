# run_turn (warm turn driver)

`run_turn` accumulates an agent's streamed deltas until the stream ends, while
also watching a cancel channel. It is one warm producer in the bridge's turn
loop.

Contract:
- Cancellation MUST take effect immediately (abort-first): when an actual
  cancel value (`Some(())`) is pending, the turn stops before draining a delta.
- Only a real cancel value is a cancellation. The cancel SENDER being dropped
  (`cancel.recv() == None`) is NOT a cancel -- it means no cancel can arrive, so
  the turn keeps draining deltas to normal completion.
- The delta stream ending (`deltas.recv() == None`) completes the turn normally.

The change adds `run_turn` using a `tokio::select!` over the cancel and delta
channels.
