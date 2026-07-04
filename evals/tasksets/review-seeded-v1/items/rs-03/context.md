# BridgeError::client_message

`client_message()` maps an internal `BridgeError` to the text sent back to the
remote A2A client over the wire.

Contract:
- The returned string is client-facing and untrusted-reader-visible.
- It must expose only a stable, generic category -- never internal detail such
  as upstream URLs, host/topology, or the underlying source error. Internal
  detail is for logs (via `Display`/`Debug`), not the wire.

The change adds handling for the new `Upstream { url, source }` variant so that
upstream failures return a client message instead of a generic fallback.
