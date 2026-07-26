I’ll preserve the verified v2 guarantees, replace the impossible channel-role design with the owner-decided wrapper boundary, and make the repository contracts and task charters concrete enough to implement directly.# Attested Prefix Cut v3 — Complete Design Specification

## 0. Decision summary

Adopt the owner-decided **B1 adapter-boundary sentinel** design.

A packaged thin wrapper, `codex-acp-attested`, is spawned by the bridge instead of bare `codex-acp`. The wrapper proxies ACP stdio, recognizes a per-turn reserved commit marker in agent text, removes that framing marker before the bridge receives text, and emits a wrapper-private ACP metadata record. The bridge ACP adapter converts that record into typed `PrefixAttestationStatus`.

The bridge core never searches prose or marker bytes. It validates typed metadata, applies an exact UTF-8 prefix cut, persists the raw bridge-visible body and decision, and then emits the effective body.

Bare `codex-acp`, Claude, Kiro, generic ACP, and arbitrary user-configured commands are incapable in v1. With sanitization enabled, they KEEP and diagnose.

This replaces v2’s unimplementable `commentary`/`final` channel premise. The locked ACP schema has no such roles; v3 does not depend on them.

---

## 1. Problem invariants

1. **Text alone cannot safely identify disposable narration.** Identical text can be process narration or the requested deliverable. Words, headings, Markdown, tense, tool names, and model judgment cannot authorize deletion while preserving a zero-false-strip guarantee.

2. **Uncertainty means KEEP.** Missing, malformed, unsupported, stale, ambiguous, suspicious, or untrusted evidence never authorizes a cut.

3. **Absent configuration means OFF.** OFF returns the original bridge-visible `String` without trimming, normalization, reconstruction, or marker parsing in the core.

4. **Only one leading prefix can be removed.** Once the committed suffix starts, all later bytes are retained.

5. **Sanitization occurs once per completed node attempt, before checkpointing, fan-in, hand-off, or terminal output composition.**

6. **The core decision is deterministic and non-semantic.** It uses typed status, exact UTF-8 byte offsets, identities, hashes, and fixed validation rules.

7. **Raw evidence precedes completion.** The pre-cut bridge-visible body and decision must commit durably before an artifact, checkpointed completion, or fan-in value is released.

8. **Only the packaged wrapper can issue a trusted v1 attestation.** A marker appearing through bare ACP, another backend, request metadata, configuration, or ordinary bridge-visible text has no authority.

9. **Marker ambiguity fails toward KEEP.** Zero markers, multiple unescaped markers, malformed escaping, an empty post-marker suffix, wrapper protocol disagreement, or incomplete wrapper metadata produce no attestation.

10. **A suspicious boundary does not cut.** A valid prefix greater than 90% of the body is kept.

11. **Bridge-synthesized completion text is never cut-eligible.** Stream-failure markers, missing-`Done` text, cancellation text, empty-final fallbacks, twin-death summaries, and a `stop_reason` used in place of text all force KEEP.

12. **Legacy behavior is intentionally conservative.** Bare narration is unchanged until produced through the capable wrapper with a valid marker.

13. **The translator cut applies only to `saw_text_delta == true`.** The `saw_text_delta == false` branch emits `stop_reason`; that body is audited as synthetic and kept, never submitted as a cut candidate.

14. **Fan-in uses the committed effective body.** The executor’s live `Update::Text` accumulator is not an authoritative fan-in value.

15. **The raw audit body begins at the wrapper boundary.** It excludes a successfully recognized protocol marker and reflects escape decoding. The underlying codex-acp wire transcript is not the audit body because the wrapper strips framing before the bridge sees text.

### Scoped impossibility result

The following text can be either narration or the entire requested transcript:

```text
I'm reading the file...
```

Likewise, a sentence preceding `# Findings` may be disposable setup or the report’s primary conclusion. Therefore, a prose classifier cannot both strip such examples and guarantee no false stripping.

The commit marker does not refute that result. It changes the evidence source: the capable producer explicitly declares a boundary under a wrapper and prompt contract.

---

## 2. Architecture

The v3 path is:

```text
workflow prompt renderer
    → injects per-turn marker contract
    → bridge invokes packaged codex-acp-attested wrapper
    → wrapper privately configures the turn
    → wrapper proxies pinned codex-acp over stdio
    → agent emits process text, reserved marker, deliverable
    → wrapper strips valid marker and escape-decodes literal markers
    → wrapper emits ordinary ACP text plus private terminal metadata
    → acp_backend.rs consumes metadata before normal text mapping
    → Update::Done carries typed PrefixAttestationStatus
    → common completion finalizer validates and decides
    → raw + decision side-store transaction commits
    → effective artifact/checkpoint/output is released
    → executor inserts effective body into fan-in outputs
```

The packaged wrapper is the capable backend. The nested bare `codex-acp` child is not independently capable.

The private metadata uses ACP’s `_meta` bag only as a bilateral wrapper-to-bridge extension. The bridge makes no assumptions about that key for generic ACP implementations. An identical `_meta` object from bare codex-acp is untrusted and cannot cut.

No ACP `commentary`, `final`, `ContentChannel`, `TextRole`, or equivalent schema field is required.

---

## 3. Marker and body contracts

### 3.1 UTF-8 contract

The bridge-visible body is a Rust `String`. All lengths and hashes operate on `body.as_bytes()`.

Consequences:

- invalid UTF-8 is out of scope;
- offsets are UTF-8 byte offsets;
- SHA-256 covers exact UTF-8 bytes;
- cuts require `body.is_char_boundary(k)`;
- slicing uses `&body[k..]`;
- the suffix is never decoded and re-encoded;
- grapheme boundaries are not inferred;
- the bridge inserts no bytes between `Update::Text` chunks.

### 3.2 Per-turn marker format

For each enabled capable turn, the bridge generates a cryptographically random 16-byte nonce. Its canonical representation is 32 lowercase hexadecimal characters.

The exact marker is:

```text
<|b2a_apc_commit_v1:{nonce_hex}|>
```

For example:

```text
<|b2a_apc_commit_v1:6f0d8e2b7c9145a1b3d74f26e8c0aa59|>
```

Normative properties:

- ASCII only;
- no leading or trailing whitespace;
- no newline is part of the marker;
- nonce is exactly 32 lowercase hex characters;
- nonce is never reused by the bridge;
- matching is byte-exact and spans ACP chunk boundaries;
- a marker with the wrong nonce is ordinary text;
- the marker is not a secret after prompt delivery;
- unpredictability prevents accidental collision with pre-existing user data but is not the issuer trust mechanism.

Failure to obtain a secure nonce fails the turn before backend invocation.

### 3.3 Escaping literal markers

A deliverable may legitimately contain the exact per-turn marker. Backslash parity provides an unambiguous, reversible escape grammar.

Let `M` be the exact marker for the current turn. For every occurrence of `M`, inspect the maximal consecutive run of ASCII backslashes immediately preceding it. Let its length be `s`.

The wrapper’s first-pass interpretation is:

- if `s` is odd, emit `(s - 1) / 2` backslashes followed by literal `M`; this is data, not a boundary;
- if `s` is even, emit `s / 2` backslashes and register an unescaped commit candidate at the current output byte length.

Backslashes not immediately followed by exact `M` are unchanged.

Thus:

| Intended logical data/control | Required wire form |
|---|---|
| Literal `M` | `\M` |
| One literal `\` followed by literal `M` | `\\\M` |
| Commit marker with no preceding data backslash | `M` |
| One literal `\` immediately before the commit | `\\M` |

More generally:

- `r` literal backslashes plus literal `M` use `2r + 1` wire backslashes;
- `r` literal backslashes followed by the commit marker use `2r` wire backslashes.

This grammar can represent every sequence of literal backslashes and literal marker bytes.

### 3.4 Candidate resolution and KEEP direction

The first unescaped marker is the only possible boundary. A later marker never replaces it.

A commit is accepted only when:

1. exactly one unescaped candidate exists;
2. the decoded suffix after it contains at least one byte;
3. the wrapper turn handshake is valid;
4. the underlying turn terminates normally enough to emit wrapper metadata.

Outcomes:

| Candidate state | Wrapper output | Status |
|---|---|---|
| Exactly one, non-empty suffix | Omit that marker; decode escaped literals | `AttestedV1` |
| No unescaped candidate | Preserve all text, decoding escaped literals | `UnavailableV1(turn_missing_deliverable_boundary)` |
| More than one candidate | Restore every candidate as literal text; decode escaped literals | `UnavailableV1(multiple_commit_markers)` |
| Candidate but empty decoded suffix | Restore candidate as literal text | `UnavailableV1(turn_ended_without_deliverable)` |
| Malformed/incomplete wrapper state | Do not authorize marker removal; fail turn or KEEP | `Rejected` or no emitted completion |

“KEEP” is relative to the decoded bridge-visible body. When candidate resolution fails, no unescaped candidate is silently deleted.

### 3.5 Streaming and buffering

A wrapper cannot stream the post-marker suffix immediately and later restore an earlier marker if a second marker creates ambiguity. Therefore:

- before the first unescaped candidate, the wrapper may stream with enough look-behind to recognize a split marker and preceding backslashes;
- after the first candidate, it buffers the complete ordered ACP update sequence through the underlying terminal event;
- if the candidate is unique and has a non-empty suffix, the buffered sequence is replayed with the marker removed;
- otherwise it is replayed with all candidates restored;
- non-text events after the candidate are buffered as well, preserving transport order.

The buffer uses bounded memory and a private `0600` runtime spool file above that bound. Spool creation, write, read, or integrity failure aborts the turn. It must not release a partially transformed completion.

This deliberately delays deliverable streaming after the marker. Process text before the marker can remain live.

### 3.6 Attested prefix length

After escape decoding and valid marker removal:

- `k` is the number of UTF-8 bytes the wrapper emitted before the marker;
- `n` is the total number of UTF-8 bytes in all ordinary wrapper-emitted ACP text for the turn;
- `body_sha256` is computed over those exact `n` bytes in event order;
- no separator is added between chunks;
- non-text events contribute zero bytes.

The bridge’s `artifact_text` must reproduce exactly the same concatenation.

---

## 4. Wrapper, prompt, and backend contracts

### 4.1 Repository placement

Task P adds:

```text
<bridge-crate>/src/bin/codex-acp-attested.rs
<bridge-crate>/src/acp/attested_wrapper.rs
```

The crate manifest declares the binary name:

```text
codex-acp-attested
```

The backend resolver changes the capable Codex command from bare `codex-acp` to the packaged sibling binary. The wrapper spawns the repository-pinned `codex-acp` executable with the resolved arguments and environment.

User-supplied commands, path overrides, and a binary merely named `codex-acp-attested` do not automatically become trusted. `SupportedV1` is returned only by the resolver variant that selects the bridge-packaged wrapper and completes its private version handshake.

### 4.2 Stdio proxy behavior

The wrapper:

- proxies ACP JSON-RPC stdin/stdout;
- passes child stderr through as stderr;
- preserves ordinary ACP frames byte-for-byte except targeted assistant text transformation and reserved metadata filtering;
- strips any child-supplied `dev.b2a.attested_prefix` `_meta` key;
- scans only text that `acp_backend.rs` would otherwise map to `Update::Text`;
- ignores marker-like bytes in tool arguments, plans, thoughts, JSON fields, stop reasons, and other non-body fields;
- emits exactly one wrapper control chunk before forwarding the underlying terminal event.

### 4.3 Private wrapper control methods

The wrapper and bridge use two private JSON-RPC methods over the wrapper’s stdio. The wrapper consumes them and never forwards them to codex-acp.

Capability request:

```json
{
  "jsonrpc": "2.0",
  "id": "<bridge request id>",
  "method": "_b2a/apc-prefix/capabilities",
  "params": {}
}
```

Required result:

```json
{
  "protocol_version": 1,
  "issuer_id": "bridge.acp.codex.commit-wrapper.v1"
}
```

Per-turn request:

```json
{
  "jsonrpc": "2.0",
  "id": "<bridge request id>",
  "method": "_b2a/apc-prefix/beginTurn",
  "params": {
    "schema_version": 1,
    "turn_id": "turn_<32 lowercase hex>",
    "enabled": true,
    "marker_nonce": "<32 lowercase hex>"
  }
}
```

The wrapper returns an exact acknowledgement before the bridge sends the ACP prompt. With `enabled: false`, it performs no marker recognition and returns an unavailable status at completion.

Missing acknowledgement, version mismatch, overlapping active turns, turn-ID mismatch, duplicate begin-turn, or nonce mismatch is a wrapper protocol violation and cannot attest.

### 4.4 Wrapper-to-bridge metadata

Immediately before the child’s terminal event, the wrapper emits a synthetic zero-length `AgentMessageChunk`:

```text
message_id = "_b2a_apc_control/<turn_id>"
content text = ""
```

Its `_meta` contains exactly one reserved object.

Attested form:

```json
{
  "dev.b2a.attested_prefix": {
    "schema_version": 1,
    "kind": "attested",
    "issuer_id": "bridge.acp.codex.commit-wrapper.v1",
    "turn_id": "turn_<32 lowercase hex>",
    "marker_nonce": "<32 lowercase hex>",
    "process_prefix_bytes": "<canonical u64 decimal>",
    "body_len_bytes": "<canonical u64 decimal>",
    "body_sha256": "<64 lowercase hex>"
  }
}
```

Unavailable form:

```json
{
  "dev.b2a.attested_prefix": {
    "schema_version": 1,
    "kind": "unavailable",
    "issuer_id": "bridge.acp.codex.commit-wrapper.v1",
    "turn_id": "turn_<32 lowercase hex>",
    "marker_nonce": "<32 lowercase hex>",
    "reason": "turn_missing_deliverable_boundary"
  }
}
```

Lengths are canonical decimal strings to avoid JSON number-width ambiguity. Leading zeros are forbidden except for `"0"`.

At `acp_backend.rs:4458`, `map_session_update` must check for this control chunk before ordinary `AgentMessageChunk` mapping. It consumes the chunk without producing `Update::Text`.

It accepts the metadata only when:

- the resolved backend capability is the packaged wrapper’s `SupportedV1`;
- the private capability handshake succeeded;
- message ID, empty content, issuer, turn ID, nonce, version, and field encodings are exact;
- exactly one control chunk occurs for the turn.

A malformed object becomes an internal rejected-attestation status. Missing control metadata becomes `UnavailableV1(backend_protocol_violation)`. A duplicate becomes rejected metadata. The bridge does not fall back to parsing text.

The ACP specification’s generic `_meta` rule remains respected: this key is interpreted only under the separately authenticated local-wrapper contract.

### 4.5 Prompt-contract injection

When and only when all of these hold:

- node mode is `attested_prefix_v1`;
- resolved capability is the packaged wrapper’s `SupportedV1`;
- a turn ID and nonce were successfully issued;

the workflow prompt renderer appends a bridge-owned protocol block after ordinary workflow-template substitution. The block is not user-substitutable and cannot be supplied by request metadata.

Every built-in workflow prompt template and custom-template path must use the same final injection helper.

The normative instruction block is:

```text
[B2A ATTESTED PREFIX PROTOCOL — REQUIRED]

You may produce process or status text before the deliverable.

Immediately before the first byte of the non-empty deliverable, emit the following
ASCII control marker exactly once:

<|b2a_apc_commit_v1:{nonce_hex}|>

Do not quote it, put it in a code fence, add whitespace to it, or add a newline
between the marker and the first deliverable byte. The marker itself is not part
of the deliverable.

Do not emit the marker until a non-empty deliverable is ready. After emitting it,
treat every later text byte as part of the deliverable.

If your intended response must literally contain the exact marker, escape it with
backslash parity: a literal marker uses one backslash immediately before it; a
literal backslash plus a literal marker uses three. A commit marker preceded by
one literal backslash uses two.

If you cannot determine a valid non-empty deliverable boundary, do not emit the
marker.
```

An implementation may wrap the block in its native high-priority prompt representation, but it must preserve every rule and the exact per-turn marker.

If the agent never emits the marker, the wrapper returns no attestation and the body is kept. There is no heuristic fallback.

### 4.6 Capability API

At `ports.rs:88-170`, `AgentBackend` gains:

```rust
fn prefix_attestation_capability(&self) -> PrefixAttestationCapability;
```

```rust
enum PrefixAttestationCapability {
    SupportedV1 {
        issuer_id: &'static str,
        boundary_scheme: PrefixBoundaryScheme,
    },
    Unsupported {
        reason: CapabilityUnavailableReason,
    },
}

enum PrefixBoundaryScheme {
    CodexCommitMarkerV1,
}
```

The declaration is queried immediately after backend resolution and private negotiation, before node execution. It is stored in the resolved backend and cannot change during the workflow run.

`TurnMeta` is extended rather than adding a second turn setter:

```rust
struct TurnMeta {
    // existing fields remain
    context_id: ContextId,
    generation: u64,
    op: TurnOperation,

    turn_id: TurnId,
    prefix_attestation_request: PrefixAttestationRequest,
}

enum PrefixAttestationRequest {
    Disabled,
    CodexCommitMarkerV1 {
        marker_nonce: [u8; 16],
    },
}
```

`TurnId` is bridge-generated:

```text
"turn_" + 32 lowercase hexadecimal characters
```

It uses 16 cryptographically random bytes and is generated once per backend turn before `configure_turn`.

The existing `configure_turn(TurnMeta)` call is the sole delivery mechanism. The wrapper-backed ACP implementation translates its fields into `_b2a/apc-prefix/beginTurn`.

### 4.7 `run_observed` binding

At `translator.rs:152-161`, preserve the existing seven parameters in their current order and append:

```rust
turn_context: &'a TurnContext,
harvest_audit_store: &'a dyn HarvestAuditStore,
```

Thus `run_observed` changes from seven to nine parameters.

`TurnContext` must expose:

```rust
struct TurnContext {
    task_id: String,
    run_id: String,
    node_id: String,
    attempt: u32,
    turn_id: TurnId,
}
```

For audit purposes:

```text
attempt_id = TurnContext.attempt
```

Its concrete Rust type is `u32`; no `AttemptId` newtype is introduced.

### 4.8 Capability matrix

| Resolved backend | Capability | Enabled-node behavior |
|---|---|---|
| Packaged `codex-acp-attested` wrapper with v1 handshake | `SupportedV1` | Prompt contract, wrapper parsing, typed status |
| Bare `codex-acp` | `Unsupported` | KEEP + `backend_declared_incapable` warning |
| Claude ACP | `Unsupported` | KEEP + warning |
| Kiro ACP | `Unsupported` | KEEP + warning |
| Generic/unknown ACP | `Unsupported` | KEEP + warning |
| User-configured arbitrary command | `Unsupported` | KEEP + warning |
| Packaged wrapper with failed/mismatched handshake | `Unsupported(protocol_downgrade)` | KEEP + warning |

---

## 5. Typed attestation status

`Update::Done` becomes:

```rust
Update::Done {
    stop_reason: String,
    prefix_attestation: PrefixAttestationStatus,
}
```

```rust
enum PrefixAttestationStatus {
    AttestedV1(AttestedPrefixV1),
    UnavailableV1(NoAttestationV1),
    Rejected(RejectedAttestation),
}
```

```rust
struct AttestedPrefixV1 {
    issuer_id: String,
    producer_id: String,
    turn_id: String,
    body_len_bytes: u64,
    body_sha256: [u8; 32],
    process_prefix_bytes: u64,
}
```

```rust
struct NoAttestationV1 {
    producer_id: String,
    turn_id: String,
    reason: NoAttestationReason,
}
```

```rust
struct RejectedAttestation {
    producer_id: String,
    turn_id: String,
    reason: InvalidAttestationReason,
}
```

`Rejected` is an internal bridge representation. It resolves the v2 hole where unsupported versions and malformed metadata could not be represented after transport decoding.

`acp_backend.rs` catches malformed or unknown wrapper metadata before the sanitizer. It creates `Rejected` using the expected current producer and bridge-issued turn ID; it does not preserve untrusted identity fields as authoritative.

Absence reasons include:

- `backend_declared_incapable`
- `protocol_downgrade`
- `sanitization_not_requested`
- `turn_missing_deliverable_boundary`
- `turn_ended_without_deliverable`
- `multiple_commit_markers`
- `backend_protocol_violation`
- `bridge_synthetic_stream_error`
- `bridge_synthetic_missing_done`
- `bridge_synthetic_cancellation`
- `bridge_synthetic_empty_final`
- `bridge_synthetic_twin_death`
- `bridge_stop_reason_without_text`

Invalid reasons include:

- `unsupported_version`
- `malformed_metadata`
- `duplicate_control_metadata`
- `backend_capability_mismatch`
- `untrusted_issuer`
- `producer_mismatch`
- `turn_mismatch`
- `nonce_mismatch`
- `length_mismatch`
- `digest_mismatch`
- `offset_overflow`
- `offset_out_of_bounds`
- `empty_deliverable`
- `offset_not_utf8_boundary`

---

## 6. Configuration and snapshot behavior

Configuration remains per node:

```toml
[workflow.nodes.reviewer]
harvest_sanitization = "attested_prefix_v1"
```

Allowed values are exactly:

```text
"off"
"attested_prefix_v1"
```

Rules:

- absent TOML means `off`;
- unknown values are hard errors naming the complete field path;
- root-level or misplaced `harvest_sanitization` is an unknown-field error;
- no graph-shape, environment, request-metadata, or workflow-wide override exists;
- resume uses the snapshotted value;
- completed outputs are not sanitized again.

At `config.rs:380`, `WorkflowNodeToml` gains the field and `#[serde(deny_unknown_fields)]`.

This is intentionally a breaking validation change. Existing configurations with unknown node keys must remove them, correct their spelling, or move supported extension data into an explicitly modeled extension map before upgrade. There is no compatibility flag that silently discards unknown node fields. The release note and configuration-validation command must call this out.

At `graph.rs:49`, `WorkflowNode` gains:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub harvest_sanitization: Option<HarvestSanitizationMode>,
```

This exact attribute pair is required for old serialized workflow graphs to resume.

At the `WorkflowNodeToml → WorkflowNode` builder at `config.rs:1345`, the field is normalized:

```text
None from new TOML → Some(Off)
explicit off       → Some(Off)
explicit enabled   → Some(AttestedPrefixV1)
```

Therefore:

- new snapshots serialize an explicit mode, including `off`;
- old snapshots deserialize missing data as `None`;
- resume normalization maps old `None` to `Off`;
- an unknown serialized enum value is a hard resume error.

---

## 7. Sanitization algorithm

Inputs are:

```text
mode
body_origin
resolved_backend_capability
expected producer_id
expected turn_id
artifact_text: String
prefix_attestation
```

Let:

```text
B = artifact_text.as_bytes()
n = B.len()
h = SHA256(B)
```

### 7.1 OFF

If mode is `off`:

- return the original `String`;
- record `kept_off`;
- do not validate the boundary;
- do not alter or reconstruct the text.

The wrapper prompt contract is not enabled in this mode.

### 7.2 Synthetic origin

If `body_origin` is any bridge-synthetic classification:

- ignore any previously observed attestation;
- replace status with the matching `UnavailableV1` reason;
- return the whole body;
- record `kept_no_attestation`.

This check precedes attestation validation.

### 7.3 Unavailable or rejected evidence

For `UnavailableV1`:

- return the original body;
- record `kept_no_attestation` and the reason.

For `Rejected`:

- return the original body;
- record `kept_invalid_attestation` and the reason.

### 7.4 `AttestedV1` validation

An attestation authorizes further consideration only if all checks pass in order:

1. version is v1;
2. backend capability is `SupportedV1(CodexCommitMarkerV1)`;
3. issuer exactly equals `bridge.acp.codex.commit-wrapper.v1`;
4. producer equals the resolved backend instance;
5. turn ID equals the bridge-issued current turn ID;
6. body length converts to and equals `n`;
7. body SHA-256 equals `h`;
8. prefix offset converts to `usize`;
9. `k < n`;
10. `artifact_text.is_char_boundary(k)`.

Any failure returns the original body with `kept_invalid_attestation`.

### 7.5 Suspicious-attestation guard

The check is:

```rust
(k as u128) * 100 > (n as u128) * 90
```

Both operands must be widened to `u128` before multiplication. Widening only the multiplication result or only one operand is non-conforming.

Consequences:

- exactly 90% is allowed;
- greater than 90% is kept;
- there is no small-body exception.

A suspicious boundary records:

```text
decision = kept_suspicious_attestation
reason = process_prefix_exceeds_90_percent
```

### 7.6 Authorized result

If validation succeeds and the boundary is not suspicious:

- `k == 0`: return the original body and record `kept_zero_prefix`;
- `k > 0`: return exactly `&body[k..]` and record `cut_attested`.

No trimming, BOM removal, CRLF normalization, newline insertion, or other mutation is permitted.

### 7.7 Safety proof shape

There are two gates.

**Wrapper gate:** An ambiguous marker stream cannot produce an attestation. Multiple candidates and an empty suffix restore the candidate bytes to the bridge-visible body. Missing markers preserve the body. Thus parser ambiguity authorizes no semantic deletion.

**Core gate:** The core cuts only a trusted-wrapper record bound to the current producer, turn, exact body length, exact body hash, and valid UTF-8 boundary. The result is either the complete body or one exact suffix.

Under a semantically correct marker, no deliverable byte is removed. Under an incorrect but structurally valid single marker, the possible loss is limited to one node prefix of at most 90% of its bridge-visible body.

A single unescaped marker emitted at the wrong semantic position is not structurally ambiguous. That remains an explicit trusted-producer failure mode.

---

## 8. Trust boundary

The trusted issuer allowlist contains exactly:

```text
bridge.acp.codex.commit-wrapper.v1
```

Trust requires all of:

- bridge-packaged wrapper resolver variant;
- exact private capability handshake;
- statically compiled issuer constant;
- matching per-turn private handshake;
- exact wrapper control metadata;
- body length and digest agreement.

The wrapper must not copy an issuer from child output, workflow configuration, request metadata, or body text. It strips the reserved metadata key from child ACP frames before proxying them.

The marker alone is not authority. Bare codex-acp or another backend may emit identical bytes, but the bridge sees those bytes as ordinary text because no trusted wrapper metadata exists.

This preserves the v2 core trust boundary: the core trusts a statically registered local adapter, not prose. The semantic assertion is now derived by that adapter from a prompt-governed agent marker rather than a nonexistent ACP channel role. This is weaker than an authenticated typed role against a misbehaving model, and the residual risk is stated rather than hidden.

Adding another capable wrapper requires:

- a new static issuer;
- a distinct boundary scheme;
- protocol and prompt-contract review;
- collision and ambiguity tests;
- backend integration tests;
- an allowlist code change.

Configuration cannot add issuers.

---

## 9. Durable raw and decision store

### 9.1 Records

```rust
struct HarvestRawRecordV1 {
    schema_version: u16, // 1
    audit_id: String,
    task_id: String,
    run_id: String,
    node_id: String,
    attempt_id: u32,
    turn_id: String,
    backend_id: String,
    producer_id: String,
    declared_capability: PrefixAttestationCapability,
    raw_body: String,
    raw_len_bytes: u64,
    raw_body_sha256: [u8; 32],
    prefix_attestation: PrefixAttestationStatus,
    provenance_sha256: [u8; 32],
}
```

```rust
struct HarvestSanitizationDecisionV1 {
    schema_version: u16, // 1
    audit_id: String,
    mode: HarvestSanitizationMode,
    decision: HarvestDecision,
    reason: Option<String>,
    node_id: String,
    producer_id: String,
    issuer_id: Option<String>,
    raw_body_sha256: [u8; 32],
    effective_body_sha256: [u8; 32],
    raw_len_bytes: u64,
    effective_len_bytes: u64,
    cut_byte_offset: Option<u64>,
    provenance_sha256: [u8; 32],
    suspicious_threshold_percent: u8, // 90
}
```

Decisions are:

- `kept_off`
- `kept_no_attestation`
- `kept_invalid_attestation`
- `kept_suspicious_attestation`
- `kept_zero_prefix`
- `cut_attested`

`cut_byte_offset` exists only for `kept_zero_prefix` and `cut_attested`.

### 9.2 Audit-ID rule

Define:

```text
LP(s) = u32 big-endian UTF-8 byte length || UTF-8 bytes
```

Then:

```text
audit_id =
  "apc1_" ||
  lowercase_hex(
    SHA256(
      ASCII "APC-AUDIT-ID-V1" ||
      LP(run_id) ||
      LP(node_id) ||
      u32_be(attempt_id) ||
      LP(turn_id)
    )
  )
```

Strings exceeding `u32::MAX` bytes are rejected before persistence.

The audit ID is deterministic for the idempotency key:

```text
(run_id, node_id, attempt_id, turn_id)
```

### 9.3 Store trait

Add at the store port layer:

```rust
#[async_trait]
pub trait HarvestAuditStore: Send + Sync {
    async fn commit_bundle(
        &self,
        raw: HarvestRawRecordV1,
        decision: HarvestSanitizationDecisionV1,
    ) -> Result<HarvestAuditCommit, HarvestAuditStoreError>;

    async fn get_by_audit_id(
        &self,
        audit_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError>;

    async fn get_by_attempt_key(
        &self,
        run_id: &str,
        node_id: &str,
        attempt_id: u32,
        turn_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError>;

    async fn list_by_task_id(
        &self,
        task_id: &str,
        after_audit_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<HarvestAuditBundleV1>, HarvestAuditStoreError>;
}
```

```rust
enum HarvestAuditCommit {
    Inserted,
    AlreadyPresentIdentical,
}
```

`commit_bundle` is one transaction. Raw and decision audit IDs must match.

If the idempotency key already exists:

- exact structural equality, excluding store-generated timestamps, returns `AlreadyPresentIdentical`;
- any differing field returns `IntegrityConflict`.

### 9.4 SQLite DDL and migration

Add the next repository migration after the existing task tables in `sqlite.rs:138-170`:

```sql
CREATE TABLE harvest_raw_records_v1 (
    audit_id               TEXT PRIMARY KEY NOT NULL,
    task_id                TEXT NOT NULL,
    run_id                 TEXT NOT NULL,
    node_id                TEXT NOT NULL,
    attempt_id             INTEGER NOT NULL
                               CHECK (attempt_id >= 0 AND attempt_id <= 4294967295),
    turn_id                TEXT NOT NULL,
    backend_id             TEXT NOT NULL,
    producer_id            TEXT NOT NULL,
    declared_capability_json TEXT NOT NULL,
    raw_body               TEXT NOT NULL,
    raw_len_bytes          INTEGER NOT NULL CHECK (raw_len_bytes >= 0),
    raw_body_sha256        BLOB NOT NULL CHECK (length(raw_body_sha256) = 32),
    prefix_attestation_json TEXT NOT NULL,
    provenance_sha256      BLOB NOT NULL CHECK (length(provenance_sha256) = 32),
    created_at_ms          INTEGER NOT NULL,
    UNIQUE (run_id, node_id, attempt_id, turn_id)
);

CREATE TABLE harvest_sanitization_decisions_v1 (
    audit_id                 TEXT PRIMARY KEY NOT NULL,
    mode                     TEXT NOT NULL
                                 CHECK (mode IN ('off', 'attested_prefix_v1')),
    decision                 TEXT NOT NULL
                                 CHECK (decision IN (
                                     'kept_off',
                                     'kept_no_attestation',
                                     'kept_invalid_attestation',
                                     'kept_suspicious_attestation',
                                     'kept_zero_prefix',
                                     'cut_attested'
                                 )),
    reason                   TEXT,
    node_id                  TEXT NOT NULL,
    producer_id              TEXT NOT NULL,
    issuer_id                TEXT,
    raw_body_sha256          BLOB NOT NULL
                                 CHECK (length(raw_body_sha256) = 32),
    effective_body_sha256    BLOB NOT NULL
                                 CHECK (length(effective_body_sha256) = 32),
    raw_len_bytes            INTEGER NOT NULL CHECK (raw_len_bytes >= 0),
    effective_len_bytes      INTEGER NOT NULL CHECK (effective_len_bytes >= 0),
    cut_byte_offset          INTEGER CHECK (cut_byte_offset >= 0),
    provenance_sha256        BLOB NOT NULL
                                 CHECK (length(provenance_sha256) = 32),
    suspicious_threshold_percent INTEGER NOT NULL
                                 CHECK (suspicious_threshold_percent = 90),
    created_at_ms            INTEGER NOT NULL,
    FOREIGN KEY (audit_id)
        REFERENCES harvest_raw_records_v1(audit_id)
        ON DELETE CASCADE
);

CREATE INDEX harvest_raw_records_v1_task_idx
    ON harvest_raw_records_v1(task_id, created_at_ms, audit_id);

CREATE INDEX harvest_raw_records_v1_attempt_idx
    ON harvest_raw_records_v1(run_id, node_id, attempt_id, turn_id);
```

Rust must perform checked `u64 → i64` conversion before SQLite insertion. Failure is a persistence error; values must not wrap.

The migration is additive:

- no old checkpoint is backfilled;
- old completed nodes remain unaudited and are not re-sanitized;
- new emitted completions require an audit bundle;
- rollback may leave the new tables unused but must not reinterpret their rows.

### 9.5 Memory store

At `task_store.rs:1327-1381`, the memory implementation adds:

- `HashMap<AuditId, HarvestAuditBundleV1>`;
- `HashMap<(run_id, node_id, u32, turn_id), AuditId>`;
- atomic insertion under the store’s existing mutex;
- identical retry as a no-op;
- differing retry as `IntegrityConflict`;
- cloned values for lookup;
- stable sorting by `(created_at_ms, audit_id)` for task listing.

It has process-lifetime durability only, matching the existing memory store’s semantics.

### 9.6 Ordering and errors

Before releasing a completion:

1. assemble the bridge-visible raw body;
2. compute the decision and effective body;
3. begin the side-store transaction;
4. insert the raw row;
5. insert the decision row;
6. commit;
7. attempt the operational event;
8. emit the artifact or write the completed-node checkpoint;
9. make the effective body available to fan-in.

Add:

```rust
enum HarvestAuditStoreError {
    Persistence(BoxError),
    IntegrityConflict { audit_id: String },
    Encoding(String),
    Lookup(BoxError),
}
```

The completion-facing error is:

```rust
CompletionCommitError::HarvestAuditPersistFailed {
    audit_id: String,
    source: HarvestAuditStoreError,
}
```

Its external/node reason code is exactly:

```text
harvest_audit_persist_failed
```

Translator and executor error mappings must use that same code. Audit failure releases no artifact, completed checkpoint, `outputs` entry, or fan-in value.

### 9.7 Resume and `run_id`

The current run naming remains intentional:

- fresh submission: `run_id = task_id`;
- resume attempt `N`: `run_id = "{task_id}-resume-{N}"`.

Therefore, audit bundles from different resumes have different idempotency keys and audit IDs. `task_id` remains stable and `list_by_task_id` is the normative cross-resume lookup.

Operators must not assume one `run_id` covers the task’s entire lifetime.

### 9.8 Operational event sink

At `orch.rs:150-190`, add:

```rust
OrchEventKind::HarvestSanitizationDecision {
    audit_id: String,
    run_id: String,
    node_id: String,
    attempt_id: u32,
    producer_id: String,
    mode: HarvestSanitizationMode,
    decision: HarvestDecision,
    reason: Option<String>,
}
```

It contains no body.

The store bundle is authoritative. Journal emission is attempted after the store commit and is best-effort:

- event failure is warning-logged with the audit ID;
- it does not fail completion;
- the durable side-store row remains queryable.

Configuration-time incapable-backend warnings continue through `DiagnosticObserver`, also best-effort. This separates durable evidence from operational notification.

---

## 10. Translator, executor, fan-in, and synthetic text

### 10.1 Common finalizer

Task F introduces one shared function:

```rust
async fn commit_harvested_completion(
    context: &TurnContext,
    mode: HarvestSanitizationMode,
    capability: &PrefixAttestationCapability,
    producer_id: &str,
    origin: CompletionBodyOrigin,
    raw_body: String,
    status: PrefixAttestationStatus,
    store: &dyn HarvestAuditStore,
) -> Result<CommittedHarvest, CompletionCommitError>;
```

```rust
struct CommittedHarvest {
    audit_id: String,
    effective_body: String,
    decision: HarvestSanitizationDecisionV1,
}
```

Direct observed/A2A completion and workflow-node completion use this same finalizer. If a runtime path causes both call sites to observe the same completion, the deterministic audit ID and identical-commit rule make the second call a no-op; differing inputs fail integrity checking.

### 10.2 Translator branch guard

At `translator.rs:229-234`:

```text
if saw_text_delta {
    artifact_text is eligible for normal attestation validation
} else {
    payload is stop_reason
    origin is BridgeSyntheticStopReasonWithoutText
    payload is audited and kept
    no cut validation is performed
}
```

The `saw_text_delta == false` branch must not treat `stop_reason` as a model deliverable or infer a marker boundary.

### 10.3 Fan-in delivery path

At `executor.rs:1693-1740`, the local `text` buffer remains useful for live accumulation and for constructing the raw completion candidate. It is not inserted directly into fan-in `outputs`.

At the current insertion seam around `executor.rs:1750-1752`, replace:

```text
outputs.insert(node_id, text)
```

with the equivalent of:

```text
committed = commit_harvested_completion(..., raw_body = text, ...)
write completed-node checkpoint using committed.effective_body
outputs.insert(node_id, committed.effective_body)
```

The checkpoint write and `outputs` insertion happen only after audit commit.

The fan-in renderer at `executor.rs:2427-2507` remains unchanged. It performs variable substitution from `outputs`, which now contains the committed effective body.

Therefore:

- no sanitizer is added to prompt rendering;
- fan-in does not reread live `Update::Text`;
- no second semantic or marker pass occurs;
- resume restores the already effective checkpointed body;
- hand-off consumers using `outputs` receive the same effective value.

This executor change is required; translator-only mutation would not satisfy fan-in.

### 10.4 Synthetic bridge text

If any bridge-generated bytes are incorporated into a completed body, the entire combined body becomes synthetic-origin and any model attestation is ignored.

| Synthetic case | Code evidence | Status | Cut eligibility | Audit behavior |
|---|---|---|---|---|
| Stream/node-failure marker | `executor.rs:1742-1750` | `UnavailableV1(bridge_synthetic_stream_error)` | **KEEP** | Bundle required if terminal output/checkpoint is emitted |
| Missing-`Done` marker | `executor.rs:1752-1761` | `UnavailableV1(bridge_synthetic_missing_done)` | **KEEP** | Bundle required if emitted |
| Cancellation text | `executor.rs:1764-1768`, `1818-1844` | `UnavailableV1(bridge_synthetic_cancellation)` | **KEEP** | Required if cancellation produces recorded output; N/A if execution aborts without output |
| Empty-final fallback | `executor.rs:1861-1894` | `UnavailableV1(bridge_synthetic_empty_final)` | **KEEP** | Bundle required if emitted |
| Twin-death summary | twin terminal-summary path | `UnavailableV1(bridge_synthetic_twin_death)` | **KEEP** | Bundle required if emitted |
| `stop_reason` used because no text delta | `translator.rs:229-234`, false branch | `UnavailableV1(bridge_stop_reason_without_text)` | **KEEP** | Bundle required before artifact emission |

Synthetic text can never produce `AttestedV1`.

### 10.5 Consumer matrix

| Consumer | Body received |
|---|---|
| Wrapper wire proxy before marker resolution | Underlying ACP frames |
| Bridge `Update::Text` stream | Wrapper-decoded text; valid framing marker absent |
| Raw audit record | Complete pre-cut bridge-visible body |
| A2A final artifact | Effective body |
| Live pre-marker progress | Raw incremental process text |
| Buffered post-marker updates | Released after marker resolution |
| Completed-node checkpoint | Effective body |
| Executor `outputs` map | Effective body |
| Fan-in prompt variables | Effective body |
| Hand-off payload | Effective body |
| `--out` and task artifact files | Effective body |
| Terminal result | Effective body |
| Synthetic terminal error text | Raw/kept body |
| `NodeFinished::output` | Effective or synthetic-kept body |
| Resume short-circuit output | Previously stored effective body |
| Operational diagnostic | Metadata and audit ID only |

---

## 11. Candidate comparison

The prior channel-role design is not implementable against the locked ACP schema. A prose heuristic remains unsafe. The chosen wrapper is the smallest implementable authoritative boundary available under the owner’s constraints.

| Concern | Heading/prose heuristic | Nonexistent ACP channel roles | Wrapper commit marker |
|---|---|---|---|
| Implementable on locked schema | Yes | No | Yes |
| Core parses prose | Yes | No | No |
| Boundary source | Semantic guess | Typed role | Prompt-governed reserved control |
| Accidental body collision | Material | N/A | Per-turn nonce + escaping |
| Ambiguity direction | Often heuristic | N/A | KEEP |
| Bare backend support | Risky guess | Unsupported | Unsupported |
| Exact byte binding | Optional | Possible | Required |
| Legacy narration | May false-strip | Kept | Kept |
| Main residual risk | Classifier error | Unimplementable | Agent emits one marker at wrong semantic position |

The wrapper design is weaker than a genuine authenticated channel-role protocol but stronger and more decidable than prose classification.

---

## 12. Evidence-corpus walkthrough

In the table, `M` means the exact per-turn marker. STRIP fixtures include a non-empty suffix and satisfy `k/n <= 90%`.

| Evidence case | Capable wrapper placement | Enabled outcome | Incapable/no-marker outcome | Reason |
|---|---|---:|---:|---|
| `I'm reading the file...` followed by deliverable | Process text, `M`, deliverable | **STRIP** | **KEEP** | Unique wrapper boundary authorizes prefix |
| `I'm going to...` followed by deliverable | Process text, `M`, deliverable | **STRIP** | **KEEP** | Grammar is irrelevant |
| Resume dropped the config flag | Old snapshot lacks field | **N/A** | **N/A** | Serde default restores `None`; resume normalizes to OFF |
| `I'll call out one correctness issue...` before `# Findings` | `M` precedes the opening sentence; `k = 0` | **KEEP** | **KEEP** | Sentence is part of deliverable |
| `I'm using rg to inspect the fan-in seam.` followed by deliverable | Process text, `M`, deliverable | **STRIP** | **KEEP** | Tool vocabulary has no authority |
| `I'm going to be blunt: resume still drops the configured policy.` | `M` before sentence; `k = 0` | **KEEP** | **KEEP** | Future tense may be deliverable |
| `I'm using the diff algorithm to measure edit distance` | `M` before sentence; `k = 0` | **KEEP** | **KEEP** | Machinery vocabulary is irrelevant |
| Connective deliverable sentence containing `I'll` | Sentence occurs after `M` | **KEEP** | **KEEP** | Every post-commit byte survives |
| Agent never emits marker | No candidate | **KEEP** | **KEEP** | No attestation |
| Agent emits marker but no suffix | Candidate restored | **KEEP** | **KEEP** | Empty deliverable cannot attest |
| Two unescaped markers | Both restored | **KEEP** | **KEEP** | Ambiguous boundary |
| Deliverable quotes exact marker using `\M` | Literal decoded; real `M` remains unique | **STRIP/KEEP as bounded** | **KEEP** | Quoted marker does not create a second candidate |
| Bare codex emits the exact marker | No trusted wrapper metadata | **KEEP** | **KEEP** | In-band text is not an issuer |
| Fan-in uses two completed nodes | Each completion committed before `outputs` insertion | **N/A** | **N/A** | Renderer receives two independently effective bodies |
| Stream failure after apparent marker | Synthetic-origin override | **KEEP** | **KEEP** | Bridge-generated completion is not cut-eligible |
| `saw_text_delta == false` | Payload is `stop_reason` | **KEEP** | **KEEP** | Guarded synthetic branch |

If a corpus demands STRIP for a bare unannotated string, it contradicts this safety model.

---

## 13. Novel adversarial cases

| Novel case | Direction | Outcome | Mechanism result |
|---|---|---:|---|
| User data contains the marker family with a different nonce | False-strip | **KEEP** | Exact per-turn match fails |
| Pre-existing document guesses the current 128-bit nonce | False-strip | **KEEP**, except negligible successful guess | Rarity reduces accidental equality |
| Deliverable needs exact current marker and uses `\M` | False-strip | **KEEP literal marker** | Odd backslash parity decodes it as data |
| Literal backslash plus marker uses `\\\M` | Corruption | **KEEP exact logical data** | Decodes to `\M` |
| Process sentence ends in one backslash immediately before `M` | False-keep | **KEEP** | Marker is escaped; no boundary |
| Process sentence doubles that backslash before `M` | Correctness | **STRIP** | One logical backslash remains and marker commits |
| Prompt injection causes marker before deliverable, then normal contract emits another | False-strip | **KEEP** | Multiple candidates restored |
| Prompt injection causes exactly one marker inside deliverable and no intended marker | False-strip | **STRIP if valid and ≤90%** | Unavoidable single-marker semantic failure |
| Agent encloses `M` in a code fence unescaped | False-strip | **STRIP if it is the unique marker** | Fences have no authority; prompt violation remains semantic risk |
| Child forges reserved `_meta` without marker | False-strip | **KEEP** | Wrapper strips child-owned reserved key |
| Bare ACP forges an exact wrapper control chunk | False-strip | **KEEP** | Capability and handshake mismatch |
| Wrapper record replayed from previous turn | False-strip | **KEEP** | Turn ID and nonce mismatch |
| Wrapper record has correct turn but stale body digest | False-strip | **KEEP** | Digest mismatch |
| Marker spans three ACP chunks | Parser confusion | **STRIP/KEEP normally** | Logical-stream matcher spans chunks |
| Marker-like bytes occur in tool JSON | False-strip | **KEEP** | Non-body fields are not scanned |
| Boundary lands before an emoji | Correctness | **STRIP** | Exact UTF-8 boundary |
| Forged offset lands inside emoji bytes | Corruption | **KEEP** | `is_char_boundary` fails |
| Correct process prefix is 91% | False-keep | **KEEP** | Suspicious guard |
| Incorrect process prefix is exactly 90% | False-strip | **STRIP** | Threshold is strictly greater than 90% |
| Narration occurs after the marker | False-keep | **KEEP narration** | Suffix is opaque and never recut |
| Agent produces escaped literal marker but no commit | False-keep | **KEEP** | Zero candidates |
| Wrapper emits two control metadata chunks | False-strip | **KEEP** | Internal `Rejected(duplicate_control_metadata)` |
| Wrapper spool fails after candidate | Availability | **N/A** | Turn aborts; no partial completion |
| Synthetic cancellation follows valid model metadata | False-strip | **KEEP entire combined body** | Synthetic-origin override |
| Audit commits, event emission fails | Observability | **N/A** | Artifact may proceed; store remains authoritative |
| Audit commits, artifact emission crashes | Consistency | **N/A** | Retry is idempotent; orphan audit row is harmless |
| Same audit key is reused with different raw text | Integrity | **N/A** | Hard `IntegrityConflict`; no artifact |

---

## 14. Remaining failure modes and blast radius

| Failure mode | Uneliminated consequence | Blast radius |
|---|---|---|
| Agent emits exactly one unescaped marker at the wrong semantic position at or below 90% | Real deliverable bytes can be stripped | One node attempt directly; downstream branches consuming its effective body |
| Adversarial instructions override the marker prompt contract | Wrong boundary or no boundary | One enabled wrapper turn; wrong single marker may propagate downstream |
| Legitimate prefix exceeds 90% | Process narration remains | One node and downstream consumers |
| Narration occurs after commitment | Narration remains | One node and downstream consumers |
| Agent mishandles backslash parity | Usually no marker and KEEP; in a unique-unescaped case, wrong cut is possible | One turn |
| Wrapper buffers after the marker | Final-stream latency and temporary storage increase | Enabled capable turns |
| Wrapper spool fails | Completion unavailable | Affected turn only; no silent partial output |
| Bare or legacy backend is incapable | Entire body remains | Nodes using that backend |
| Live process text was already displayed | Final cut cannot retract it | Live observers only |
| Side-store unavailable | No completion can commit | Affected node attempts |
| Raw side-store duplicates sensitive text | Storage/disclosure surface increases | Audit readers for the task tenant |
| Local wrapper is compromised | It can forge boundaries and hashes | All enabled nodes using that packaged wrapper |
| Best-effort operational event fails | Alerting may miss a decision | One event; durable lookup remains available |
| Operator restores raw output after downstream execution | Earlier downstream effects are not undone | Already-executed branches |
| SHA-256 collision | False binding | Cryptographically negligible |
| Audit row commits before permanent artifact failure | Orphan audit record | Storage only |
| Task resumes multiple times | Audit history spans multiple `run_id` values | Operational query complexity; `task_id` lookup contains it |

Default-OFF and per-node configuration limit exposure. The 90% guard caps, but does not eliminate, trusted-producer semantic mistakes.

---

## 15. Implementation charters

### 15.1 Task P — Wrapper, marker, prompt contract, and typed prerequisite

#### Scope and change sites

- Add `codex-acp-attested` binary and wrapper module.
- Add private capability and begin-turn JSON-RPC handling.
- Spawn pinned bare codex-acp as the wrapper child.
- Implement exact marker format, nonce handling, backslash-parity decoder, chunk-spanning recognition, candidate restoration, buffering, and spool failure behavior.
- Strip child-supplied reserved metadata.
- Emit wrapper control metadata through zero-length `ContentChunk`.
- At `ports.rs:88-170`, add capability API and extend `TurnMeta`.
- Extend `Update::Done` with `PrefixAttestationStatus`, including `Rejected`.
- At `acp_backend.rs:4458`, consume trusted control chunks before ordinary message mapping.
- Add the bridge-owned prompt-contract injection helper to every workflow prompt-template path.
- Change capable Codex resolution to the packaged wrapper.
- Declare bare codex, Claude, Kiro, generic ACP, and arbitrary commands incapable.
- Update all `Update::Done` pattern matches without activating cuts.
- Add configuration-time incapable warnings.

#### Acceptance criteria

1. The wrapper capability is declared before the first turn and remains stable.
2. Only the packaged, successfully handshaken wrapper declares `SupportedV1`.
3. A per-turn bridge-issued turn ID and nonce reach the wrapper through `configure_turn`.
4. The prompt contains the exact nonce-specific marker contract only for enabled capable turns.
5. A unique marker with a non-empty suffix is absent from bridge text.
6. Wrapper `k`, length, and hash match exact emitted `Update::Text` bytes.
7. A quoted marker using the parity grammar survives as exact logical data.
8. Zero markers produce unavailable status and KEEP.
9. Multiple candidates are restored and produce unavailable status.
10. A marker without a suffix is restored and produces unavailable status.
11. Marker matching works across every split position.
12. Child-forged reserved metadata cannot produce an attestation.
13. Missing, malformed, duplicate, wrong-version, wrong-turn, or wrong-nonce control metadata is unavailable or rejected, never trusted.
14. Tool and non-body fields contribute no bytes.
15. Bare codex, Claude, Kiro, and generic ACP never attest.
16. With sanitization absent/OFF, Task P causes no semantic task-output change.
17. No bridge-core function searches text for the marker.

#### Test plan

- Marker at every byte position and every ACP chunk split.
- Zero, one, two, and three unescaped markers.
- Empty suffix and one-byte suffix.
- Wrong nonce and near-match markers.
- Backslash runs of length 0 through at least 9.
- Literal marker before and after the true boundary.
- Multibyte text around the marker.
- Child metadata forgery and duplicated wrapper metadata.
- Capability handshake downgrade.
- Missing begin-turn and overlapping turn requests.
- Wrapper buffer and spool failure injection.
- Exact length/hash comparison against bridge concatenation.
- Per-backend incapable behavior.
- Prompt golden test showing exact injected block and nonce.

### 15.2 Task F — Configuration, sanitizer, audit, executor delivery

#### Scope and change sites

- At `graph.rs:49`, add `WorkflowNode.harvest_sanitization` with exact `serde(default, skip_serializing_if = "Option::is_none")`.
- At `config.rs:380`, add the TOML field and `deny_unknown_fields`; document the breaking config migration.
- At `config.rs:1345`, copy and normalize the field into `WorkflowNode`.
- Update snapshot/resume normalization so old absence means OFF and new snapshots serialize explicit modes.
- At `translator.rs:152-161`, append `TurnContext` and `HarvestAuditStore` to `run_observed`.
- Implement the pure validation and sanitization algorithm.
- At `translator.rs:229-234`, apply cut validation only under `saw_text_delta == true`; audit-and-keep `stop_reason` under the false branch.
- Implement `commit_harvested_completion`.
- Add the `HarvestAuditStore` trait, SQLite tables/migration, and memory implementation.
- Add deterministic audit IDs and integrity-conflict behavior.
- Add `CompletionCommitError::HarvestAuditPersistFailed` and external reason code.
- At `orch.rs:150-190`, add `HarvestSanitizationDecision`.
- At `executor.rs:1693-1752`, replace raw `text` insertion with committed effective-body checkpointing and insertion.
- Leave fan-in substitution at `executor.rs:2427-2507` unchanged.
- Classify all synthetic paths at the named executor sites.
- Verify hand-off, journal, result, files, checkpoints, and resume consume effective bodies.

#### Acceptance criteria

1. Every Section 7 algorithm branch produces the exact body, decision, and reason specified.
2. Both operands of the 90% calculation are widened to `u128` before multiplication.
3. OFF and every fallback return byte-identical bridge-visible input.
4. A successful cut returns exactly `&body[k..]`.
5. Malformed and unsupported metadata are representable as `Rejected`.
6. Synthetic text is never cut-eligible.
7. The `saw_text_delta == false` branch keeps and audits `stop_reason`.
8. Raw and decision rows commit atomically before artifact or checkpoint release.
9. Store failure releases no artifact, completed checkpoint, `outputs` value, or fan-in input.
10. Identical retry is a no-op; conflicting retry is a hard integrity error.
11. Memory and SQLite stores implement identical observable semantics.
12. Audit lookup works by ID, attempt key, and stable task ID across resumes.
13. Executor `outputs` receives the committed effective body, not its live accumulator.
14. Fan-in and hand-off consume independently committed node bodies.
15. New snapshots serialize explicit OFF/ON; old absence resumes as OFF.
16. Unknown TOML and snapshot values fail loudly.
17. Completed resume outputs are not sanitized again.
18. No downstream type gains segment-provenance metadata.
19. Event emission failure does not invalidate an already committed audit bundle.
20. All evidence and adversarial test tables pass.

#### Test plan

Pure sanitizer:

- every decision and invalid reason;
- arbitrary Rust `String` OFF identity;
- output is always full input or one exact suffix;
- `k = 0`, `k = 1`, exactly 90%, and immediately above 90%;
- empty body, `k = len`, overflow, out-of-range, and UTF-8 interior offsets;
- CRLF, BOM, NUL, combining marks, emoji, and non-ASCII;
- wrong issuer, producer, turn, length, hash, and capability;
- `Rejected` version/malformed cases;
- narration after commitment.

Persistence:

- raw/decision atomicity;
- foreign-key and uniqueness constraints;
- checked SQLite integer conversion;
- identical and conflicting retries;
- audit-ID golden vectors;
- lookup APIs and pagination;
- process-lifetime memory behavior;
- store failure before emission;
- event failure after commit;
- crash after commit before artifact.

Configuration and resume:

- absent, OFF, and enabled TOML;
- misspelled and misplaced keys;
- breaking unknown-node-key behavior;
- graph serde old-field absence;
- new explicit snapshot round trips;
- resume `run_id` changes with stable `task_id`;
- no re-sanitization of completed nodes.

End to end:

- translator `saw_text_delta` true and false branches;
- every named synthetic executor path;
- executor output source replacement;
- two independently sanitized fan-in inputs;
- hand-off, checkpoint, journal, result, `--out`, and artifact files;
- live pre-marker progress remains visible;
- capable and incapable backend diagnostics.

---

## 16. Round-2 findings resolution

| Finding | Resolution |
|---|---|
| **B1 — ACP channel roles absent** | Refuted the channel-role premise. Sections 2–4 specify the owner-decided packaged wrapper, exact marker, escaping, private handshake, `_meta` record, prompt contract, and incapability of bare codex/Claude/Kiro. |
| **B2 — fan-in delivery path unspecified** | Section 10.3 names the exact code path: `executor.rs:1693-1740` remains raw staging; `1750-1752` changes to checkpoint and insert `CommittedHarvest.effective_body`; substitution at `2427-2507` remains unchanged. |
| **B3 — capability and turn-ID API absent** | Sections 4.6–4.7 add `AgentBackend::prefix_attestation_capability`, extend `TurnMeta`, define bridge-issued `TurnId`, bind `configure_turn`, and append `TurnContext` plus `HarvestAuditStore` to `run_observed`. |
| **B4 — side-store interface absent** | Section 9 defines the trait, methods, audit-ID rule, SQLite DDL/migration, memory semantics, idempotency, lookup API, transaction order, and `HarvestAuditPersistFailed`. Named change sites include `sqlite.rs` and `task_store.rs`. |
| **B5 — synthetic text unclassified** | Section 10.4 classifies stream failure, missing Done, cancellation, empty-final, twin-death, and no-text stop reason. All are unavailable, never cut, and audited when an output is emitted. |
| **M1 — no `AttemptId` type** | Section 4.7 defines `attempt_id = TurnContext.attempt` with concrete type `u32`; no newtype is introduced. |
| **M2 — `deny_unknown_fields` breaking** | Section 6 explicitly declares the breaking change and requires operators to remove, correct, or relocate unknown node keys. There is no silent migration flag. |
| **M3 — arithmetic widening non-normative** | Section 7.5 mandates `(k as u128) * 100 > (n as u128) * 90`; both operands are widened before multiplication. |
| **M4 — malformed status unrepresentable** | Section 5 adds `PrefixAttestationStatus::Rejected`; transport decode maps malformed and unsupported wrapper metadata into it before sanitization. |
| **M5 — diagnostic sink absent** | Section 9.8 adds `OrchEventKind::HarvestSanitizationDecision` at `orch.rs:150-190` and specifies best-effort emission after authoritative audit commit. |
| **m1 — graph serde resume compatibility** | Section 6 requires `#[serde(default, skip_serializing_if = "Option::is_none")]` on `WorkflowNode` at `graph.rs:49`. |
| **m2 — run IDs across resume** | Section 9.7 states fresh/resume naming, distinct audit keys, stable `task_id`, and the cross-resume lookup rule. |
| **m3 — Task F missing change sites** | Task F explicitly names `graph.rs:49`, `config.rs:380`, and the builder at `config.rs:1345`, including the serde compatibility attributes. |
| **m4 — translator false branch** | Sections 1, 10.2, and Task F normatively restrict cut validation to `saw_text_delta == true`; the false branch audits and keeps `stop_reason`. |

---

## 17. Decidable acceptance principle

Given configuration `C`, capability `A`, wrapper wire text `W`, issued marker `M`, bridge-visible body `B`, origin `G`, attestation status `P`, output `O`, and audit bundle `D`, an implementation is conforming if and only if all applicable rules below hold:

1. Only the packaged, successfully handshaken wrapper can declare `SupportedV1`.

2. Marker recognition uses the exact nonce-specific bytes and backslash-parity grammar in Section 3.

3. Exactly one unescaped marker with a non-empty decoded suffix produces a candidate boundary. Zero, multiple, or empty-suffix cases produce no attestation and preserve all candidate bytes in `B`.

4. A marker from any incapable backend is ordinary text and cannot authorize deletion.

5. If `C` is absent or OFF, then `O == B` and the decision is `kept_off`.

6. If `G` is bridge-synthetic, then `O == B`, status is unavailable with the matching synthetic reason, and no cut validation occurs.

7. If `P` is unavailable, then `O == B` and the decision is `kept_no_attestation`.

8. If `P` is rejected or any validation rule fails, then `O == B` and the decision is `kept_invalid_attestation`.

9. If attestation is otherwise valid but `(k as u128) * 100 > (n as u128) * 90`, then `O == B` and the decision is `kept_suspicious_attestation`.

10. If attestation is valid, non-suspicious, and `k == 0`, then `O == B` and the decision is `kept_zero_prefix`.

11. If attestation is valid, non-suspicious, and `k > 0`, then:

```text
O.as_bytes() == B.as_bytes()[k..]
```

and the decision is `cut_attested`.

12. No artifact, completed checkpoint, executor `outputs` value, or fan-in value may be released before the matching raw and decision bundle commits.

13. The executor value used at `executor.rs:2427-2507` must equal the committed effective body, not the live `Update::Text` accumulator.

14. Every emitted completion’s audit ID must resolve to the exact raw body, status, identities, hashes, mode, decision, effective hash, and offset used.

15. The `saw_text_delta == false` translator branch is acceptable only when `stop_reason` is kept and audited as synthetic.

16. No implementation may choose or modify `k` based on words, headings, Markdown, fences, stop reasons, graph topology, or model-based semantic scoring.

These conditions are mechanical. A reviewer or automated judge needs no judgment about whether prose “looks like” narration.
## 18. Round-3 panel amendments (normative errata — override cited sections)

1. **§10.3 fan-in seam correction (was wrong at two levels):** the insertion seam is
   `outputs.insert(node_id.as_str().to_string(), (text, ok, usage))` at **executor.rs:2552**,
   inside the `while let Some((node_id, text, ok, usage, disposition)) = inflight.next().await`
   fan-in drain loop; the preceding `WorkflowEvent::NodeFinished` yield is at line 2549. The
   ranges 1750-1752 and 2503-2507 cited earlier are NOT the seam (text-accumulation arm /
   wrong drain body). Instrumenting those produces tests that pass single-node and silently
   fail multi-node.
2. **§4.6a (new) — capability handshake call site:** the implementer must resolve ONCE before
   coding: which `AcpBackend` lifecycle method issues `_b2a/apc-prefix/capabilities`
   (candidates: `connect()` vs the spawn path), and whether it is one-time-per-backend or
   one-time-per-session; the declaration must be stable before node execution (§4.6).
   Record the decision in the implementation's design note.
3. **§4.4 — `beginTurn` failure propagation:** use the stash-and-surface-at-prompt-entry
   pattern, citing the existing `pending_turn_meta` field as precedent.
4. **Task F acceptance criterion 7 addition:** the audit bundle is required at
   `outputs.insert` regardless of the `ok` flag (failed nodes still produce completion
   bodies and still audit).
5. **Task F scope addition:** name the `run()` wrapper change choice (thread-through vs
   compile-time null defaults for tests) and include the ~fourteen `translator.rs` test call
   sites (462-870) in the blast radius.
6. **§4.4 citation correction:** the request-send seam is `ConnectionTo::send_request` at
   `jsonrpc.rs:2341`.
