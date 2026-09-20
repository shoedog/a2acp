---
task-type: implement
---
# Implement ADR-0041 Slice 2B1 pure capsule contracts

## Description

Implement only child 2B1 from the approved Slice 2B plan. The authoritative approved-closure commit is
`56922bdd66a6f92522848f2167647e9576e74d8a`; its plan SHA-256 is
`fd865c357fad25980fd23bf16f7af2d9cff10dc429de6a55577d09ae6be5fa71`. The merged code predecessor is
`27a885f6d4af6a517c2a8899aa5bfe36605b8fb7`.

This child is provider-free and effect-free. It defines canonical records, validation, binding, and envelope-port
contracts only. Do not implement export, filesystem materialization, Git subprocesses, encryption/decryption,
restore, CLI/config/store wiring, remote custody, cleanup, or operator adoption.

Read the complete approved plan, both spec-review records, ADR-0041 §9, the Slice 2A handoff, and the existing
`custody_inventory`/`custody_seal` implementation and tests before editing. Treat the plan as authoritative. If an
API cannot satisfy it without inventing an effect boundary or weakening Slice 2A validation, stop and report the
exact gap rather than broadening scope.

## Acceptance Criteria

- Add `custody_capsule` and export it from `bridge-core` with private-field Serde v1 records, validating
  constructors, immutable accessors, `validate`, canonical JSON encode/decode, and SHA-256 content digests wherever
  the approved plan requires them. Unknown fields, wrong schemas, and non-canonical JSON must refuse through the
  same validation path.
- Implement the closed artifact roles and canonical artifact-role rows used by `CustodyCapsuleIndexV1`. Logical
  names remain lossless `LosslessPathV1` values and obey the exact reserved-name/role mapping in plan §3.1-§3.2.
- `CustodyCapsuleLayoutV1` accepts only a validated, sealable manifest and deterministically derives the full index:
  exactly three control artifacts, one payload for every captured non-object-database class, and exactly one Git
  pack iff `object_database` is captured with a non-empty inventory. It refuses unresolved coverage,
  `object_database=captured` with an empty inventory, empty inventory with a non-empty state, excluded object
  database coverage, and every `unknown` object kind. Its 3/17 totals are derived invariants, not a caller limit.
- `CustodyCapsuleBindingV1` is pure and post-effect-shaped: it validates all inputs, re-derives the expected layout,
  requires the supplied index to equal that layout, checks
  `manifest.content_digest() == index.manifest_digest() == seal.manifest_digest()`, and proves exact two-way
  index/seal artifact-name equality. Foreign manifest, foreign index digest, foreign seal digest, missing,
  duplicate, extra, or unmapped artifacts receive typed refusals.
- `CustodyRestorePolicyV1` accepts only the closed inert policy: hooks, executable config/includes, filters,
  network/lazy fetch, archived external-path activation, and workflow resume disabled; source/project-ref mutation
  forbidden. Any active/unknown value or schema version refuses.
- `CustodyEnvelopeFormatV1` binds non-empty capsule format, sealing tool, and sealing tool version.
  `CustodyEnvelopeContextV1` binds logical artifact name, manifest digest, capsule format, and a non-empty recipient
  set canonicalized exactly like `CustodySealV1` (empty entries refuse, duplicates collapse, UTF-8 byte order).
  Context encoded from unsorted/duplicate seal-time input must be byte-identical to context re-derived from the
  canonical seal.
- Define sealed `CustodyEnvelopeSealerV1` and `CustodyEnvelopeOpenerV1` contracts that accept the validated context
  and bounded opaque bytes/metadata without performing cryptography. Do not add a production or fixture adapter in
  2B1. Keep the sealing mechanism private so external crates cannot implement the traits.
- Add only the minimum read-only accessors to `custody_seal.rs`; do not weaken or reclassify any v1 validation.
- Use typed errors that distinguish invalid input, non-sealable manifest, three-way digest mismatch, derived-layout
  mismatch, missing/duplicate/extra/unmapped artifacts, coverage-state mismatch, unknown object kind, and
  unsupported restore behavior. Every reachable constructor error branch has a focused negative test.
- Do not add dependencies, features, filesystem APIs, subprocess APIs, provider calls, network access, source
  discovery, or allocation proportional to an unvalidated/unbounded input.

## RED-First and Mutation Evidence

Before production code, add the focused integration-test target/import and run it on the exact predecessor to
capture structural RED. Record that output as structural evidence only. Once the seam compiles, perform and restore
all seven behavioral mutations from plan §4.3. Each mutation is admissible only when the focused test flips in the
expected direction against compiling code; a compile error, invalid fixture, zero selected tests, or refusal by an
unrelated validator is no evidence.

Mutation 6 has three independent controls: foreign index digest, foreign seal digest, and a different valid
manifest while index and seal still agree on the old digest. Mutation 3 uses distinct canonical names and must
demonstrate that disabling the exact duplicate-class-ownership check makes the focused test pass.

Write the structural RED command/output, every behavioral mutation/control command and result, and restoration
confirmation into `docs/superpowers/reviews/2026-09-20-adr0041-slice2b1-implementation-handoff.md`.

## Verification

Run directly, without `tee` or another status-masking pipe, and report exact totals:

```text
cargo test --locked --offline -p bridge-core --test custody_capsule
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets
cargo test --locked --offline --workspace
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo deny check
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

The bridge's hermetic verification subset does not replace the direct full-suite gates. If the implementation
quarantine cannot run a listed host/system gate, run the largest valid subset, name the exact exclusion and
mechanism in the handoff, and do not claim that gate passed. The controller will repeat full direct host gates after
integration. Do not rebaseline or fix unrelated failures; use a same-environment exact-predecessor control before
attributing one to this change.

## Files

Owned implementation paths:

- `crates/bridge-core/src/custody_capsule.rs` (new)
- `crates/bridge-core/src/custody_seal.rs` (minimum immutable accessors only)
- `crates/bridge-core/src/lib.rs` (module export only)
- `crates/bridge-core/tests/custody_capsule.rs` (new)
- `docs/superpowers/reviews/2026-09-20-adr0041-slice2b1-implementation-handoff.md` (new evidence/handoff)
- `docs/reliability-execution-roadmap.md` and the Slice 2B planning handoff (status reconciliation only)

Do not edit `Cargo.toml`, `Cargo.lock`, adapter/runtime code, CLI/config/store code, container configuration, other
tests, or any 2B2/2B3 path.

## Spec Refs

- `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, especially §§3, 4, 7, and 8
- `docs/superpowers/reviews/2026-09-20-adr0041-slice2b-spec-review-round1.md`
- `docs/superpowers/reviews/2026-09-20-adr0041-slice2b-spec-review-round2.md`
- `docs/adr/0041-durable-custody-local-clone-lifecycle.md`, especially §§4, 8, 9, 14, and 17
- `docs/superpowers/reviews/2026-09-16-adr0041-slice2a-handoff.md`

## Review and Stop Conditions

The implementation review cap is two admitted hard-read-only rounds. Report WRONG before SMELL. Repair a closed
enumerable rejected population on the same quarantine artifact; park an open-class population. Do not restart the
implementation or silently extend the cap.

Stop without broadening scope if the traits require a real provider, a record needs effectful validation, the
implementation needs a new dependency/feature, the diff escapes owned paths, or any 2B2 filesystem/Git effect is
required. Do not push, merge, publish, clean worktrees, or mutate the running operator.

## Commit Message

`feat(bridge-core): add custody capsule contracts`
