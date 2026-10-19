# PR 1a split: parse/freeze, then workflow composition

Design only; baseline `897cfb2a4d0a23ce7d1b8e7eb352f46aefdcc108`, branch `plan/pr1a-skills-split`.
Authority: [binding design](../specs/2026-10-18-reviewer-prism-skill-design.md), §§1, 2, 6 and both fold tables.
This plan authorizes no implementation, provider turns or publication; the controller commits.
No builds/tests were run to prepare it. Lane ownership below is supplied coordination authority.

## Boundaries and source anchors

- `bin/a2a-bridge/src/config.rs:1137,1994,2324,2472`: agent TOML, string-only parse, config-relative
  workflow loading, then registry conversion. String-only parsing cannot infer the declaring file.
- `crates/bridge-core/src/run_spec.rs:59,293,415,489`: persisted delivery spec, workload hash,
  construction and validation. `crates/bridge-workflow/src/run_spec.rs:1` only re-exports it.
- `crates/bridge-core/src/session_fingerprint.rs:7` has public struct fields; its production literal
  at `crates/bridge-coordinator/src/session_manager.rs:940` belongs to G-A2.2.
- `crates/bridge-workflow/src/executor.rs:3601` is the owned route's template expansion seam.
  Do not mistake the dispatcher branch at line 2932 for a second production entrance.
- Keep PR 1b's direct/implement composition and PR 2's bundles, roles, policy selectors, replacement,
  eval opt-out and namespace resolution separate. Start with direct `[[agents.skills]]` only.

## 1a-i — independently reviewable parsing/freezing foundation

**Exact implementation ownership** (new files marked new; tests otherwise stay in owned modules):

| File | Allowed change |
|---|---|
| `bin/a2a-bridge/src/config.rs` | Skills declaration/module, admission API, preview/refusal plumbing and unit tests; no custody edits |
| `bin/a2a-bridge/src/config/skills.rs` (new) | Bounded loader, frontmatter validation, immutable frozen selection, metadata rendering, unit tests |
| `bin/a2a-bridge/tests/fixtures/skills/navigator.md` (new) | Generic valid frontmatter/body fixture |
| `bin/a2a-bridge/tests/fixtures/skills/literal-braces.md` (new) | Generic Unicode/braces/delimiter fixture |
| `bin/a2a-bridge/Cargo.toml` | Only a bounded YAML parser dependency if needed for frontmatter |
| `Cargo.lock` | Only that dependency's resolution; controller handles any required network resolution |

None is an exclusively reserved file in the supplied lanes. `config.rs` is explicitly section-shared:
Lane R retains `[custody]` and all custody validation/tests. Coordinate the manifest/lock reservation
before implementation; do not expand a dependency update into unrelated lockfile churn.
Declare the child module from `config.rs`, avoiding both bin `main.rs` and core `lib.rs`.
Keep frozen types local here; 1a-ii converts them into core's persisted representation.

**Contract:**

1. Parse `id`, `path`, `delivery="prompt"`; absent skills means an empty selection. Reject unknown
   delivery, empty IDs/paths and duplicate IDs within an agent's ordered selection. Separate agents
   may attach the same ID independently; no roles/bundles or implicit directory discovery yet.
2. Provide an explicit admission API taking the declaring config location and checkout context.
   Resolve relative paths against that config's directory, never process/session cwd. Preserve
   declaration order. No caller may turn a raw path into a supposedly frozen attachment.
3. Open without following symlinks and check the opened handle is a regular file; reject symlinked
   components as well as final links. Bound reads even if the file grows; avoid blocking on FIFOs.
   Use platform-specific safe opening, or explicit unsupported refusal, never a follow-link fallback.
4. Require UTF-8 and valid frontmatter with nonempty string `name` and `description`; reject malformed
   or duplicate required keys. Validation must not normalize the frozen source bytes (including CRLF).
   Limit the entire source to 32 KiB per skill and each effective per-agent selection to 64 KiB,
   measured in bytes. No asset retrieval, relative-reference resolution, copying or script execution.
5. Freeze ID, exact source bytes, SHA-256, byte count and delivery in an immutable in-memory value.
   No later file reread within that admission. Explicit re-admission may produce a new identity;
   editing/replacing/deleting a source cannot mutate an existing selection.
6. Preview exposes ID/digest/size/mode, `origin=checkout` where applicable, and delivery status;
   ordinary Debug/errors/previews omit source bytes and host paths. Explain config-equivalent trust
   and recommend operator-controlled sources outside untrusted checkouts. Preview is not delivery.
7. Supply metadata-only rendering for `not_delivered(path)` with path meaning the execution route:
   `direct`, `implement`, or `workflow_pending_composition`, never a filesystem path. This is a
   callable config-layer preview/report value, not a new CLI or run-summary wiring claim.
8. Until 1a-ii wires admission, existing registry conversion must explicitly refuse nonempty skills
   as unsupported by that entrance; it must not silently discard them. Empty configurations keep
   their current behavior. This staged foundation is not the completed PR 1a delivery feature.

**Fingerprint deferral:** do not edit `session_fingerprint.rs` in 1a-i. Adding its field requires the
reserved `session_manager.rs` literal to change; a default derive does not repair a struct literal.
No persisted structure changes in 1a-i: old config/registry behavior remains covered, and persisted
old-record gates move with the actual schema changes to 1a-ii/1b.

**Execution policy:** `crates/bridge-core/src/execution_policy.rs` is **not needed for 1a-i**.
No selection/binder/registry mutation is needed for the config-owned frozen value. For 1a-ii, keep
prompt identity in the run spec and reuse fingerprint without changing MCP effects. If G-A2.2 needs
that identity in its bound selection, the controller must first ask the bridge session for the exact
field/digest/validation seam; that file is not implicitly authorized by this plan.

**Acceptance tests:** fixtures plus generated missing/malformed/invalid-UTF-8/duplicate-ID cases;
frontmatter scalar, quoted and multiline descriptions; exact and one-byte-over limits, including
multibyte totals; directory/FIFO/final-link/parent-link rejection; config-relative resolution with a
different cwd; byte/digest stability after source replacement; ordering; safe metadata and checkout
origin; no cwd writes; empty-config compatibility and explicit unsupported-conversion refusal.
Use temporary fixtures, not operator files. No serialized old-record test is claimed for an unchanged schema.

## 1a-ii — workflow delivery, blocked on G-A2.2 integration

Do not dispatch until the bridge session releases the required production seams and reconciles Lane R's
review-history ownership in `main.rs`. Re-read the landed interfaces before accepting this file reservation.

**Exact proposed implementation ownership:**

| Files | Purpose |
|---|---|
| `bin/a2a-bridge/src/config.rs`, `bin/a2a-bridge/src/config/skills.rs` | Carry admitted values into workflow setup and omission reporting; retire staging refusal only where admission/reporting is wired |
| `bin/a2a-bridge/src/main.rs` | Local/served admission and public preview/run-summary output; exclude Lane R review-history hunks |
| `crates/bridge-core/src/run_spec.rs` | Frozen skill records, validated persisted identity, S5 hash and old-record compatibility |
| `crates/bridge-core/src/session_fingerprint.rs` | Optional attachment identity and mismatch comparison |
| `crates/bridge-workflow/src/executor.rs` | Owned-route composition after rendering, payload omission and per-turn delivery metadata |
| `crates/bridge-coordinator/src/coordinator.rs`, `crates/bridge-coordinator/src/dispatch.rs` | Served workflow admission/forwarding and direct omission summaries only |
| `crates/bridge-coordinator/src/session_manager.rs` | Workflow mint/reuse identity; direct attachment consumer remains 1b |
| `crates/bridge-a2a-inbound/src/server.rs` | Served workflow handoff/summary parity; no client-supplied skill paths |
| `crates/bridge-workflow/tests/pr1a_skills.rs` (new) | Fake-adapter composition, exclusions and replay cases |
| `bin/a2a-bridge/tests/pr1a_skills.rs` (new) | Local/served entrance and preview/summary integration cases |

Existing-module unit tests cover schema/fingerprint internals; no extra fixture files are reserved here.
G-A2.2's remaining reserved files stay **excluded**: bin `smoke.rs`, `compatibility.rs`; core
`translator.rs`; coordinator `detached.rs`, `batch.rs`, `compact.rs`; controller `turn.rs`, `resilient.rs`.
Also excluded throughout: G-A2.1's core `accounting.rs`, `accounting_backend.rs`, `ports.rs`,
`task_store.rs`, `lib.rs` and store `sqlite.rs`; Lane R's `custody_push.rs`, `evidence.rs`,
`implement_resume.rs` and custody config. Any newly necessary file requires a revised ownership agreement.

**Delivery and persistence acceptance:**

- Prepend exactly one leading attached-skills `Part` after task template expansion. Generate fresh
  per-prompt nonce delimiters; reject/regenerate collisions with composed content. Never expand skill
  braces or mutate stored input. Brief lint sees original input; findings and output contracts stay last.
  State ID/digest precedence over installed copies; include delivered MCP names, checkout and conventions
  beside unchanged skill bytes. Advisory instructions confer no extra execution or sandbox authority.
- Deliver the full attachment on every supported workflow task turn, including retries/continuations;
  record metadata and repeated-turn byte overhead, not source text/paths in normal reports.
  Preflight PONG, `/rename` and the warm seed remain byte-identical; task composition must not alter seed bytes.
- S1 decision: omit skills for **in-container Claude workflow targets**, reporting
  `not_delivered: payload_gate` in preview and run summary until PR 1b supplies separately authorized
  exact-adapter evidence or validated lane-specific limits. Preserve the original task payload.
  Test omission at maximal multibyte sizes; fake delivery never establishes real transport support.
- S4: direct/implement receive no skill body in 1a; emit `not_delivered(path)` in both preview and
  run summary. Their continuation consumer check waits for 1b; do not fingerprint undelivered skills
  as delivered instructions. Keep their current MCP sets and prior findings/contracts unchanged.
- S5: core run-spec fields carry frozen ID/bytes/digest/delivery and node selection, with `serde(default)`
  and `skip_serializing_if` when empty. Commit nonempty ordered content/delivery identity and effective
  omission decisions into `workload_fingerprint`; validation detects tampering. Preserve the legacy
  hash input when empty, not merely the JSON shape. Replay reads frozen records, never ambient files/policy.
- Old V2/V3 records decode attachment-free and re-encode byte-identically; test real frozen fixtures.
  Session fingerprint is currently in-memory: test empty legacy equivalence and changed delivered
  identity refusal before reuse. Test any newly serialized projection similarly. Implement checkpoints
  remain unchanged until 1b, which owns old-checkpoint/resume drift tests and explicit re-admission.
- Four fake adapters assert exact bodies on supported targets; cover local/served parity, braces,
  forged headers/closing delimiters, duplicate prevention, repeated turns, payload-gated Claude,
  unchanged no-config prompts/exclusions, no cwd writes, and resume after source deletion/change.

## Estimates, gates and review budget

1a-i: roughly 400–650 production lines plus 350–500 test/fixture lines (750–1,150; lockfile separate).
1a-ii: roughly 450–700 production lines plus 650–950 test lines (1,100–1,650).
These are planning ranges, not permission for unrelated cleanup; split again before dispatch if exceeded materially.
Declare **two review rounds per part** before dispatch. WRONG precedes SMELL; each WRONG needs an input,
incorrect result, mechanism and bounded regression. At cap, disclose a bounded extension only for converging
closed findings; park open-class issues for design. Never discard the artifact to restart the loop.

Future implementers follow [CONTRIBUTING](../../../CONTRIBUTING.md): same-environment pre-change red /
post-change green behavior tests, negative/edge cases per path, then `CARGO_INCREMENTAL=0 cargo test
--workspace --locked --no-fail-fast` with totals. Attribute unrelated failures only with a base control.
Run `cargo build --all-targets`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo deny check`, and `a2a-bridge validate --repo-hygiene` against the candidate build.
Coverage floors at this baseline: workspace 85%, core 86%, ACP 90%, API 90%, workflow 87%, coordinator 85%,
MCP 70%; use CI's `cargo llvm-cov --workspace/--package … --fail-under-lines …` gates.
Use `CARGO_INCREMENTAL=0` for aggregate verification. Core changes also require the Windows bridge-store
CI lane: Unix success cannot establish non-Unix compilation. Report every unavailable check and exclusion;
no live provider/container compatibility claim follows from deterministic tests.

## Open coordination questions

1. Controller: reserve the narrow parser dependency/lockfile edits alongside config ownership; select a
   bounded YAML implementation without narrowing valid name/description syntax silently.
2. Bridge session: when are 1a-ii's named seams released, and does G-A2.2 require skill identity in
   `execution_policy.rs`? No 1a-i dependency on that answer; no speculative edit to that file.
3. PR 1b owner: which separately authorized evidence or validated bounds will retire Claude's payload
   omission? Until then, omission remains explicit and tested; no provider turn is authorized here.
