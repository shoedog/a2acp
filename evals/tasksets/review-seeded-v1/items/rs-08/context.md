# Runner::checkpoint

A detached workflow persists each finished node so a crash-resume re-runs only
the still-pending nodes. `checkpoint` writes two durable records via two
independent awaits: the node's output, and a "complete" marker.

Contract:
- Resume treats a node as done iff its "complete" marker is set, and downstream
  nodes then read its stored output.
- A turn can be CANCELLED at any await point (its future is dropped). The two
  writes must be ordered so a drop between them can never leave a node marked
  complete without its output present.

The change adds `checkpoint`, called when a node finishes.
