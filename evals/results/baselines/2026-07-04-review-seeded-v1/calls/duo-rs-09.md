**Merged Review: rs-09 — `frame::encode`**

Both lenses (correctness/Codex, architecture/Claude) succeeded and agree on the core defect: the `as u16` cast silently truncates. They differ only on severity — Codex rated it MAJOR, Claude rated it BLOCKER. **Claude is right: this is a BLOCKER.** The failure is provably WRONG with a concrete, catastrophic outcome (permanent silent stream desync), which clears the bar for a blocker, not merely a risk.

---

**BLOCKER — `src/lib.rs:20` (with root cause at `src/lib.rs:4`): the length header cannot represent valid payloads, and the cast truncates them silently (WRONG).**
The contract admits payloads "up to 1 MiB," but `pub len: u16` / the 2-byte big-endian header maxes out at 65,535 bytes. `let len = payload.len() as u16` reduces the length mod 65536, so it is lossy for exactly the admitted range: a 65,536-byte payload yields `len = 0`, and a 1 MiB payload also yields `len = 0`. The header then violates the stated invariant "header MUST equal `payload.len()`" — the reader consumes 0 bytes and parses the payload's first two bytes as the *next* frame's header, desyncing the stream permanently and corrupting every subsequent message.
This is one defect with a symptom and a structural root cause that **must be fixed together**: a checked cast alone would merely reject all valid payloads ≥ 64 KiB, because the header field itself is too narrow. **Fix:** widen `len` to `u32`, emit a 4-byte header in `to_bytes`, and update the reader's header parse in lockstep. This is a wire-format change — version/negotiate it if the reader is already deployed.

**MAJOR — `src/lib.rs:19`: `encode` has no fallible seam, so oversize is unrepresentable rather than handled (design gap).**
The signature returns `Frame` unconditionally, so "payload exceeds what the header can hold" has nowhere to surface — the failure is pushed onto the wire instead of back to the caller. Even after widening to `u32`, the "up to 1 MiB" upper bound stays unenforced: nothing rejects a 2 MiB payload at the sender, so it becomes a desync discovered only at the receiver. **Fix:** make encoding fallible (`Result<Frame, EncodeError>`) and enforce the ≤ max-payload cap here, rejecting at the boundary.

**MINOR — `src/lib.rs:4-5`: `pub len` + `pub payload` let callers construct a Frame whose header lies (invariant unenforced by construction).**
With both fields public, the `len == payload.len()` invariant lives only in prose; any code can build `Frame { len: 5, payload: /* 100 bytes */ }`. **Fix:** make the fields private and force construction through the fallible `encode` (or a validating constructor) so the invariant is guaranteed by the type, not convention.

**MINOR — no tests for `encode` boundary behavior.**
Missing coverage lets the 65,536-byte and 1 MiB contract cases regress unnoticed. **Fix:** add tests for exact small payloads, `u16::MAX`, `u16::MAX + 1`, 1 MiB, and over-limit rejection.

---

**Verdict: REJECT — ship only after fixing the BLOCKER (widen the header to `u32` *and* remove the truncating cast, as one change) and adding the fallible/bounded encode seam (MAJOR); the two MINORs are worth folding in while touching this code.**