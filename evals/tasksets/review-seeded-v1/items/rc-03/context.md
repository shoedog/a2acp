# Registry::register

`Registry` maps an agent id to its launch command. `register(id, cmd)` inserts
the entry and returns a borrowed handle to the stored command for the caller to
use immediately.

Contract:
- `register` returns a reference to the just-stored command.
- The method takes `&mut self`, so no concurrent mutation can occur between the
  insert and the read within the call.

The change adds `register`, which inserts the entry and returns the stored value.
