# Pool::complete

`Pool` hands out a fixed number of warm-agent slots. `checkout` returns a
`Lease` which, by design, frees its slot back to the pool when it is dropped
(RAII -- see `impl Drop for Lease`). `available` gates admission.

Contract:
- Each checked-out slot is returned exactly ONCE.
- `Lease` already frees its slot on drop; nothing else should also free it.

The change adds `complete(lease)`, called when a turn finishes, to return the
turn's slot to the pool.
