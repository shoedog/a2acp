# ADR-0041 Slice 2A — local seal contracts

**Status:** implementation task, stacked on PR #104 head `4e00b76fdcc86627dd1f328d7087b546f619255b`

**Authority:** `docs/adr/0041-durable-custody-local-clone-lifecycle.md`. PR #104 later advanced for an independent
rustls lock repair; this dirty Slice 2A delta retains `4e00b76f` as its immutable comparison base until integration.

## Objective

Add the provider-free, effect-free record layer that binds one complete local custody manifest to the exact
artifacts of one local seal. This is the first bounded increment of ADR-0041 rollout stage 2. It creates no
capsule, performs no encryption or restore, reads no filesystem or Git state, and grants no capture, promotion,
restore, quarantine, reap, or deletion authority.

The slice must make these facts mechanically representable and canonical:

- a `custody-manifest.v1` identifies exactly one unit, run, and materialization generation;
- every required state-coverage class is present exactly once and classified as captured, empty,
  excluded-reproducible, or unresolved;
- original refs, original objects, dependencies, and exclusions are deterministic, duplicate-free inventories;
- a manifest with any unresolved coverage or dependency is not sealable;
- a `custody-seal.v1` binds the manifest content digest, exact artifact names/lengths/SHA-256 digests,
  encryption recipients, and format/tool versions;
- canonical digests are independent of caller input order but change when any bound value changes;
- schema constants are validated, and deserialization cannot turn invalid public-field combinations into accepted
  records.

## Owned paths

- `crates/bridge-core/src/custody_seal.rs` (new)
- `crates/bridge-core/src/lib.rs` (module export only)
- `crates/bridge-core/tests/custody_seal.rs` (new integration tests)
- this task and the Slice 2A handoff/review artifacts

Do not change Slice 1A/1B inventory behavior or its fixtures. Reuse `LosslessPathV1`,
`CustodyStateClassV1`, and `Sha256HexV1`; do not duplicate those primitives.

## Required records

Implement private-field records with constructors, accessors where tests/consumers need them, `validate`, canonical
encoding, canonical decoding, and content digest methods. `decode_canonical` must reject non-canonical JSON and
every invariant violation rather than relying on constructors having been used. Any public Serde deserialization
implementation must also route through the same constructors; direct deserialization must not manufacture an
invalid public record that bypasses `validate`.

1. `CustodyManifestV1`
   - literal schema `custody-manifest.v1`;
   - non-empty `unit_id`, `run_id`, and `materialization_id`;
   - one non-empty `generation_id` that distinguishes later mutations of the same materialization;
   - `coverage`: exactly one entry for each closed `CustodyCoverageClassV1` variant;
   - deterministic inventories of original refs, original Git objects, dependencies, and exclusions.
2. `CustodyCoverageEntryV1`
   - closed class vocabulary covering ADR-0041 §4: refs/HEAD, object database, index, worktree, stash/reflogs,
     in-progress Git operations, linked worktrees, nested repositories/submodules, LFS/external payloads,
     alternates/shared stores, Git configuration/hooks, bridge evidence, external evidence, reproducible outputs;
   - one `CustodyStateClassV1` value;
   - unresolved requires at least one sorted/deduplicated ADR reason code; resolved states forbid blocking reasons;
   - excluded-reproducible requires a non-empty exclusion id that resolves to exactly one manifest exclusion;
     other states forbid an exclusion id, and every manifest exclusion must be referenced by coverage.
3. `CustodyOriginalRefV1`
   - non-empty lossless ref name plus a typed target: direct Git object identity, non-empty lossless symbolic-ref
     name, or `unborn`; sort/deduplicate by the complete record and reject conflicting duplicate names;
   - every direct target must exactly match an entry in the manifest's original-object inventory.
4. `CustodyOriginalObjectV1`
   - closed Git object format (`sha1` or `sha256`), lowercase hex object id of the exact format length, and closed
     kind (`commit`, `tree`, `blob`, `tag`, `unknown`); sort/deduplicate by complete identity and reject the same
     format/object id paired with conflicting kinds.
5. `CustodyDependencyV1`
   - non-empty dependency id and kind, a binding digest, one `CustodyStateClassV1`, and blocking reasons under the
     same unresolved/resolved rules as coverage; unique by id; an unresolved dependency makes the manifest
     non-sealable;
   - every exclusion reconstruction-dependency id must resolve to exactly one manifest dependency.
6. `CustodyExclusionV1`
   - non-empty exclusion id, content class, policy version, and at least one reconstruction dependency id; sort and
     deduplicate dependency ids; unique by exclusion id.
7. `CustodySealV1`
   - literal schema `custody-seal.v1`;
   - manifest digest;
   - at least one `CustodySealedArtifactV1`, each with a lossless relative name, byte length (zero is valid), and
     SHA-256;
   - at least one non-empty encryption recipient identity;
   - non-empty capsule format and sealing tool/version strings;
   - use a platform-neutral, slash-delimited relative namespace; reject leading `/`, Windows drive prefixes,
     backslashes, NUL bytes, empty components, `.`/`..` components, duplicate names, and every slash-delimited
     ancestor conflict such as `pack` plus `pack/data`, even when another name sorts between them;
   - artifact and recipient order is canonicalized.

All string identifiers are opaque in this slice; validate only the explicit non-empty/closed-format rules above.
Do not invent provider identities, authorization tokens, signatures, timestamps, paths to source units, or remote
retention assertions.

## Sealability

`CustodyManifestV1::sealability()` returns a typed result that is sealable only when all coverage rows and all
dependencies are resolved. It reports deterministic blocking reason codes for non-sealable manifests. Constructing
or decoding a valid unresolved manifest is allowed; constructing a `CustodySealV1` does not accept a manifest and
therefore conveys no claim that the digest names a sealable manifest. Later effectful code must evaluate sealability
before writing.

## RED-first evidence

Before production implementation, add the integration-test target and capture the exact current-base failure.
The initial structural RED may be an unresolved module import, but it is seam evidence only. Before completion,
perform and record at least these behavioral mutation controls against compiling code, restoring each mutation:

1. remove one coverage-class completeness check: a manifest missing the worktree row must be accepted by the
   mutant and rejected after restoration;
2. make unresolved coverage sealable: the sealability test must fail under the mutant and pass restored;
3. omit artifact byte length from the seal digest input: two otherwise identical seals with different lengths must
   collide under the mutant and differ after restoration;
4. disable artifact prefix-conflict rejection: `pack` plus `pack/data` must be accepted by the mutant and rejected
   after restoration.

Tests must also cover duplicate/conflicting refs, wrong-length or uppercase Git object ids, exclusion reconstruction
requirements, wrong schema on decode, non-canonical JSON, order-independent digests, and one negative/edge case per
constructor branch.

## Verification and review gates

Run:

```text
cargo test --locked --offline -p bridge-core --test custody_seal
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

Report exact totals. Preserve any unrelated baseline failure rather than re-baselining it. Independent review cap:
two admitted rounds. Findings must be tagged WRONG or SMELL, WRONG first. Closed enumerable WRONG findings are fixed
on this artifact; open-class findings park the slice for design.

## Explicitly excluded

- filesystem/Git enumeration, consistent snapshots, quiescence, locks, journals, staging directories, or writes;
- archive serialization beyond canonical JSON records;
- pack/bundle creation, closure proof, LFS/alternates capture, encryption/decryption, keys, recipient resolution;
- restore planning/execution or hook/config disabling;
- CLI/config/store wiring;
- remote promotion, verification receipts, providers, network calls, project refs, merge, quarantine, reap, deletion,
  migration, or running-operator changes.

## Done claim

Done means the pure contract and its tests pass all gates, the behavioral RED controls are recorded, an independent
review approves within the cap, and a durable handoff binds the stacked base, changed paths, evidence, review result,
and deferred Slice 2B work. It does not mean PR #104 or this slice is merged, deployed, adopted, or remotely durable.
