# Attested Prefix Cut v3 Task P Implementation Notes

## Delegated decision: capability request lifecycle

`AcpBackend::connect_observed_after()` issues `_b2a/apc-prefix/capabilities` after the normal ACP initialize/authenticate sequence and before the backend is returned to the registry. The request is one-time per backend instance, not per session.

Rationale: this point has an established `ConnectionTo<Agent>`, is still before any node execution, and yields a capability declaration that can be stored on the resolved backend and remain stable for the backend lifetime. The spawn path remains responsible only for process/container setup; both spawned and in-process transports converge through `connect_observed_after()`.

A failed or mismatched wrapper capability response does not fail the backend. It records `Unsupported(protocol_downgrade)` so enabled callers resolve toward KEEP rather than trusting a downgraded or spoofed transport.

The documented `cargo build --bin a2a-bridge` path now builds the runtime wrapper entry point as a hidden mode inside `a2a-bridge` itself. The standalone `codex-acp-attested` binary remains available, but production resolver output invokes the current `a2a-bridge` executable with the hidden wrapper argv marker.

The wrapper child is resolved before wrapper spawn. The bridge canonicalizes an absolute `codex-acp` child path from `A2A_BRIDGE_CODEX_ACP_PATH` or from `PATH`, then passes that pinned path through `--codex-acp`; the wrapper itself has no PATH/env fallback and rejects non-absolute child commands.

## Delegated decision: beginTurn failure propagation

`configure_turn(TurnMeta)` remains the sole turn metadata delivery API. It stashes metadata in the existing `pending_turn_meta` map. `prompt_inner()` takes that stash at prompt entry, sends `_b2a/apc-prefix/beginTurn` only for a supported `CodexCommitMarkerV1` request, and surfaces send or acknowledgement failure as a prompt-open error before `session/prompt` is installed. The private beginTurn params include the ACP `session_id` so the wrapper can bind active turns and prompt buffers per session rather than process-globally.

Rationale: this follows the existing `pending_turn_meta` pattern: metadata is consumed exactly once at prompt entry, early setup failures cannot leave stale turn metadata for a later prompt, and no prompt is dispatched after a failed private begin-turn handshake.

## Turn ID wire format

Task P changes newly issued bridge turn IDs from the older `turn-<uuid>` display shape to `turn_<32-lower-hex>` so the private wrapper can validate a compact nonce-like identifier. Existing persisted/logged historical turn IDs are not rewritten; external queries that pattern-match the old hyphenated shape must account for both formats during mixed-version observation windows.

## Fix-round decision: default-OFF mode gate for the prompt contract

The prompt contract was initially injected for every capable turn, which violated §4.5 (whose first
condition is node mode `attested_prefix_v1`) and §15.1 acceptance criterion 16 (with sanitization
absent/OFF, Task P causes no semantic task-output change). The fix adds the §6 mode as an input:
`bridge_core::attestation::HarvestSanitizationMode { Off, AttestedPrefixV1 }` (serde wire names
`"off"` / `"attested_prefix_v1"`, default `Off`) and threads it through
`prefix_attestation_request_for_capability(mode, capability)`, the sole production constructor of an
enabled `PrefixAttestationRequest::CodexCommitMarkerV1`.

Task P ships no configuration surface for the mode. Every production call site (workflow executor
warm and cold paths, coordinator direct prompt, inbound warm dispatch) passes a literal
`HarvestSanitizationMode::Off`, so the enabled path is unreachable today: the request is always
`Disabled`, `append_prompt_contract` is a no-op, `AcpBackend::prompt_inner` sends no
`_b2a/apc-prefix/beginTurn` (it only sends one for a `CodexCommitMarkerV1` request), and the wrapper
never activates a turn, so it performs no marker recognition and rewrites no text. Task F's per-node
`harvest_sanitization` config becomes the only switch that can select `AttestedPrefixV1`; the enum is
placed in bridge-core so Task F can serialize exactly this type into `WorkflowNode` per §6.

The connect-time capability handshake (`_b2a/apc-prefix/capabilities`) intentionally remains
unconditional. §4.6 requires the declaration "immediately after backend resolution and private
negotiation, before node execution", stored on the resolved backend and stable for the run — it
cannot depend on any node's mode, because a single backend may serve a mix of off and (post-Task F)
enabled nodes. It is transport-internal: the wrapper consumes the private method and never forwards
it to the codex-acp child (§4.3), so nothing model-visible flows from it. Likewise `configure_turn`
still carries the bridge-issued `turn_id` with a `Disabled` request; that produces zero private wire
traffic for the turn.

## Scope note

Task P activates the wrapper prerequisite path for capable Codex ACP turns: the bridge generates a per-turn `turn_<32-lower-hex>` ID plus nonce, passes the request through `configure_turn`, sends private `beginTurn` at prompt entry, and appends the nonce-specific prompt contract for the same request.

Task P still does not add the Task F sanitizer, audit side-store, config enum, or fan-in output cutting. Until Task F enables sanitization decisions, harvested task output remains semantically kept even though capable wrapper turns can now surface typed prefix-attestation status.
