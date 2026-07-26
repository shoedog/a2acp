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

## Fix-round decision: no fence/Markdown consultation (§13, §17.16)

An early Task P revision tracked Markdown code fences in `resolve_marker_text` and treated a fenced
marker as literal (KEEP), following a task-spec line later identified as a spec-authorship error
(task spec correction of 2026-07-26). Per design §13 ("Fences have no authority") and §17 condition
16 (no implementation may choose or modify `k` based on fences or any Markdown context), that fence
machinery (`in_fence` state, `is_fence_delimiter_line`) was removed before the final Task P commit;
the marker resolver consults only byte-exact marker matching and backslash parity, with no lexical
or Markdown context of any kind.

The fix round locks both directions with tests: a unique UNESCAPED marker inside a backtick or tilde
fence STRIPs (`marker_inside_code_fence_attests_because_fences_have_no_authority`); an ESCAPED
marker anywhere, including inside fences, stays data and never blocks a genuine commit elsewhere
(`escaped_marker_is_data_everywhere_including_fences`). Escaping is the only quoting mechanism. The
escaping table is exercised for backslash runs 0 through 9 per the §15.1 test plan
(`backslash_runs_zero_through_nine_follow_parity_table`).

## Scope note

Task P builds the wrapper prerequisite machinery for capable Codex ACP turns — packaged wrapper with
marker grammar, private capability handshake at connect, bridge-issued `turn_<32-lower-hex>` IDs on
`configure_turn`, and the prompt-contract/beginTurn plumbing — but, per the default-OFF mode gate
above, none of the per-turn enabled path runs: with every production mode literal `Off`, no nonce
request is minted, no `beginTurn` is sent, no prompt contract is appended, and the wrapper acts as a
byte-preserving proxy. Task F's `harvest_sanitization` config is what will activate enabled turns.

Task P still does not add the Task F sanitizer, audit side-store, config wiring, or fan-in output
cutting. Until Task F lands, every harvest resolves to KEEP and task output is byte-identical to the
pre-Task-P bridge.


## Task F implementation note: audit commit seam and direct translator wrapper

Task F activates the per-node `harvest_sanitization` switch. The workflow executor now carries raw node output plus typed prefix-attestation metadata until the fan-in drain; immediately before `NodeFinished`, checkpointing, `outputs.insert(...)`, or downstream prompt rendering, it calls `commit_harvested_completion(...)` and inserts the committed effective body. The best-effort `HarvestSanitizationDecision` journal event is emitted only after the authoritative audit commit returns.

The legacy `Translator::run()` wrapper remains source-compatible by using compile-time null defaults: a synthetic direct `TurnContext`, mode `Off`, and `NoopHarvestAuditStore`. Production observed call sites use `run_observed(...)` with an explicit `TurnContext` and `Arc<dyn HarvestAuditStore>`; this keeps direct/non-workflow translation byte-compatible while workflow nodes get durable audit storage through the task-store adapter.

The Task P capability handshake location remains unchanged: `AcpBackend::connect_observed_after()` probes the private wrapper capability once per resolved backend, before node execution, and `configure_turn` supplies per-turn begin metadata only when the Task F node mode is `attested_prefix_v1`.

Task F's `#[serde(deny_unknown_fields)]` on `[[workflows.nodes]]` is a breaking config migration: any pre-Task-F node table with unrecognized keys now fails startup and must remove or rename those keys before upgrading.

Fix round (MAJOR 1): terminal-status classification is now causally dependent on the turn's control-frame drain, never on a wall clock. `record_control` runs only under the update-registry lock while the turn's route is live; the prompt driver removes the route under that same lock and then closes the drain (`PrefixTurnState::close_control`) before classifying. Because the SDK dispatch loop is serial and FIFO ("the dispatch loop waits for each handler to complete before processing the next message"), every §4.4-conforming control frame — on the wire before the prompt response — is recorded before `prompt_fut` can resolve, so at the production classification point the drain is already closed and `BackendProtocolViolation` is claimed only when no valid in-flight frame can still arrive. A frame refused at the barrier arrived on the wire after the terminal response and is dropped as post-terminal (warn-logged). The old fixed 10 ms quiescence window is gone; a 250 ms operational backstop remains solely so a mis-sequenced caller (classification demanded while the drain is still open) cannot hang — its expiry is audited as the new distinct `NoAttestationReason::ControlDrainTimeout` (`control_drain_timeout`), an operational failure, never as a protocol violation. §5's absence-reason list is worded non-exhaustively ("include"); `control_drain_timeout` is a fix-round addition under that clause.

The standalone `run-workflow` CLI now supplies a retaining audit store whenever the selected graph enables `harvest_sanitization = "attested_prefix_v1"`. If `[store].path` is configured, audit bundles are written to that config-relative SQLite task store; otherwise the CLI creates a per-run SQLite audit artifact at `.git/a2a-bridge/run-workflow/<run-id>/harvest-audit.sqlite` when a git artifact surface exists, falling back to the OS temp directory. The CLI logs the audit-store path and task id on stderr so operators can inspect the rows after the run.

Invalid attestation records intentionally persist the bridge-constructed `Rejected` status in `HarvestRawRecordV1.prefix_attestation`, not the original untrusted claim. This follows the byte-blind bridge design but means forensic inspection cannot recover a malicious issuer/hash from the audit row unless the adapter captured the wire frame elsewhere.

`HarvestAuditStore::list_by_task_id` defines missing cursors as an empty page. The SQLite implementation uses a cursor lookup plus keyset query; the in-memory twin keeps the simpler full in-memory scan because it is test/local state only.
