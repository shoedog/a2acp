# frame::encode

The bridge's stdio MCP transport wraps each message in a length-prefixed frame:
a 2-byte big-endian length header, then that many payload bytes. The reader
trusts the header to know how many payload bytes to consume.

Contract:
- The header MUST equal `payload.len()` so the reader consumes exactly the
  payload and stays aligned for the next frame.
- Payloads may be up to 1 MiB (well beyond 64 KiB).

The change adds `encode`, which frames a payload for the wire.
