# ADR-0041 Slice 2B — local capsule and inert restoration plan

**Status:** approved at the two-round review cap; 2B1 is implementation-ready

**Exact base:** `27a885f6d4af6a517c2a8899aa5bfe36605b8fb7` (`origin/main`, PR #104 merge)

**Authority and limits:** The owner authorized planning, independent review, and implementation orchestration of
the pure/effect-free 2B1 child after spec approval. No 2B2/2B3 filesystem or Git effect, source mutation, remote
promotion, project publication, merge, cleanup, quarantine, reap, deletion, restore into a user-selected path, or
running-operator mutation is authorized. ADR-0041 and the Slice 2A handoff require a separately reviewed child spec
before any later Slice 2B effect is implemented.

## 1. Outcome

Slice 2B will prove, locally and without providers, that one sealable `custody-manifest.v1` can become a
self-contained capsule and can be reconstructed into a new inert materialization without the source repository,
its alternates, sibling clones, caches, credentials, or network.

The completed parent slice must establish all of the following:

1. every `captured` coverage class maps to exactly one sealed class artifact: a coverage payload, except
   `object_database`, whose non-empty inventory maps to the single Git pack;
2. every sealed artifact has one declared role, and no manifest coverage row or artifact is silently omitted;
3. the capsule's Git object pack contains exactly the manifest's object inventory and is transitively closed;
4. closure is verified from a fresh isolated object database with no alternates or lazy fetch;
5. an isolated restore reproduces captured logical payload bytes exactly; inert hidden-state evidence may be placed
   outside its original active location, while ordinary working state is reconstructed byte-for-byte where safe;
6. archived hooks and behavior-affecting Git configuration are retained as evidence but are never executed or
   activated by export, verification, or restore;
7. restored linked-worktree, nested-repository, in-progress-operation, and workflow-resume state is inert until a
   later, separately authorized activation design exists;
8. every write is confined to a caller-supplied new scratch or restore root, uses no-follow descriptor-relative
   operations, and refuses overwrite, mount escape, identity drift, or budget exhaustion;
9. failure leaves the source untouched and reports a truthful partial local outcome; it never creates a custody,
   deletion, or remote-durability claim.

This is ADR-0041 rollout stage 2 only. Remote promotion and independent verification remain stage 3; merge
retention, quarantine, reap, and deletion remain stage 4.

## 2. Why Slice 2B is three ordered children

A single exporter + Git closure verifier + restore engine would combine record design, hostile-input parsing,
subprocess isolation, descriptor-relative writes, crash cleanup, and behavioral restoration in one review. That is
too broad to converge within the required cap. Slice 2B is therefore one parent outcome delivered by three serial,
independently reviewable children:

| Child | Deliverable | Effects | Entry gate |
|---|---|---|---|
| **2B1** | Canonical capsule index, artifact-role map, encryption-port contract, and inert restore-policy records | None | This plan approved |
| **2B2** | Local exporter plus isolated Git-object closure proof | New scratch-root writes and hardened local Git subprocesses only | 2B1 approved and merged into the Slice 2B branch |
| **2B3** | New-root restore plus exact hidden-state fixture verification | New restore-root writes and hardened local Git subprocesses only | 2B2 approved and its capsule fixture sealed |

No child may begin on a merely plausible predecessor. Each binds the exact approved predecessor commit and reruns
its focused tests. A conflict or schema gap returns to the owning child; it does not get patched opportunistically
in a later child.

This parent plan makes 2B1 implementation-ready. Before 2B2 or 2B3 begins, that child receives its own reviewed
task fixing its byte framing, numeric bounds, exact Git argv/environment, owned paths, crash points, and test
fixtures. The later-child sections below are acceptance boundaries, not permission to invent those details during
implementation.

## 3. Shared capsule model

### 3.1 Physical layout

The v1 capsule exterior is a directory of independently sealed opaque artifacts, not a tar/zip stream. This avoids
adding an exterior archive parser or exterior path-extraction policy, compression nondeterminism, and
aggregate-memory risk to the first local proof. A later reviewed 2B2 child may define bounded interior framing for
a coverage payload; it must not turn artifact names into extraction paths. `custody-seal.v1` remains the exterior
descriptor and is not listed as an artifact inside itself.

Reserved logical artifact names are portable slash-delimited names already admitted by
`CustodySealedArtifactV1`:

```text
control/manifest.json.enc
control/capsule-index.json.enc
control/restore-policy.json.enc
git/objects.pack.enc
payload/<coverage-class>.bin.enc
```

Names are logical identifiers, not paths accepted directly from an untrusted archive. Exterior materialization
walks their validated components beneath retained destination descriptors. The v1 capsule exterior has no symlink,
hardlink, device, FIFO, socket, absolute-path, `..`, platform-prefix, or case-fold alias entry type. Interior
coverage framing and its separate path/mode policy remain a reviewed 2B2 decision.

### 3.2 Artifact roles and total mapping

`custody-capsule-index.v1` binds the manifest digest and contains one row for every sealed artifact name. Each row
has exactly one closed role:

- `manifest`;
- `capsule_index`;
- `restore_policy`;
- `git_object_pack`;
- `coverage_payload(<CustodyCoverageClassV1>)`.

The index must contain exactly one manifest, one index, and one restore-policy role. It contains exactly one Git
pack when the manifest object inventory is non-empty and none when that inventory is empty. Every seal artifact
name appears exactly once in the index, and every index artifact name appears exactly once in the seal. Each
captured class owns exactly one payload artifact in v1, except `object_database`, which owns the one Git pack. The
closed 14-class vocabulary therefore caps a v1 capsule at 17 sealed artifacts (three control artifacts plus at most
14 class artifacts); there is no caller-selected artifact-count limit or chunk fan-out.

For each manifest coverage row:

- `captured` requires exactly one corresponding coverage payload, except `object_database`, whose non-empty
  payload is exactly the Git-pack role;
- `empty` requires no corresponding payload;
- `excluded_reproducible` requires no corresponding payload and preserves the exact manifest exclusion binding;
- `unresolved` refuses capsule planning before any write.

An artifact cannot cover two classes. Cross-class deduplication and multi-artifact chunking are deliberately
excluded from v1 because they make completeness, bounds, and restore ownership ambiguous. Zero-byte payloads
remain valid artifacts.

The object-database row and inventory must agree bidirectionally: a non-empty original-object inventory requires
`captured`; `object_database` `captured` requires a non-empty inventory; and an empty inventory requires state
`empty`. `excluded_reproducible` is not accepted for this self-contained v1 object database. An `unknown` object
kind refuses layout construction because exact kind equality cannot otherwise be proved.

### 3.3 Encryption boundary

Production code sees a narrow opaque envelope port with separate `seal` and `open` methods. Both accept the same
validated `CustodyEnvelopeContextV1` canonical encoding as authenticated context. That record binds the logical
artifact name, manifest digest, capsule format, and the canonical non-empty recipient set exactly as
`CustodySealV1` stores it: empty recipients refuse, duplicates collapse, and remaining recipient strings sort by
their UTF-8 bytes. Seal-time context uses the exact values the exterior seal will carry; open-time context is
re-derived from that seal and must encode byte-identically. The port returns or consumes bounded byte streams and
an exact non-empty capsule-format, sealing-tool, and sealing-tool-version identity; it cannot claim success without
producing ciphertext bytes and recipient metadata compatible with `CustodySealV1`.

Slice 2B ships no real key discovery, key storage, password prompt, recipient resolver, or crypto provider. A
deterministic test-only envelope implementation is allowed only under fixture/test support and must identify itself
as non-production. No public constructor may label plaintext as encrypted or manufacture a production seal from
the fixture adapter.

### 3.4 Inert restore policy

`custody-restore-policy.v1` has closed, non-optional v1 values:

- hooks: disabled;
- executable Git config/includes: disabled;
- clean/smudge/process filters: disabled;
- network and lazy object fetch: disabled;
- archived external-path activation: disabled;
- workflow/agent resume: disabled;
- source and project-ref mutation: forbidden.

Unknown values or schema versions refuse. The raw originals remain encrypted evidence; the active restored Git
config is synthesized from the safe closed policy rather than copied from the archived config.

## 4. Child 2B1 — pure capsule contracts (next implementation slice)

### 4.1 Owned paths

- `crates/bridge-core/src/custody_capsule.rs` (new)
- `crates/bridge-core/src/custody_seal.rs` (read-only accessors only; no v1 validation weakening)
- `crates/bridge-core/src/lib.rs` (module export only)
- `crates/bridge-core/tests/custody_capsule.rs` (new)
- ADR-0041's schema table, this plan, the Slice 2B handoff, and the reliability roadmap for reconciliation only

### 4.2 Required API

Implement private-field Serde records with validating constructors, accessors, `validate`, canonical encode/decode,
and content digests:

- `CustodyCapsuleIndexV1` with literal schema, manifest digest, and canonical artifact-role rows;
- `CustodyCapsuleArtifactRoleV1` with the closed roles in §3.2;
- `CustodyCapsuleLayoutV1`, constructed from a validated, sealable manifest before effects and enforcing the exact
  reserved names and state/inventory rules in §3.2; because the manifest vocabulary is closed, the constructor
  asserts the derived invariant of exactly 17 artifacts when all 14 classes are captured and exactly three when
  every class is empty rather than exposing an unreachable caller-selected cap branch;
- `CustodyCapsuleBindingV1`, constructed only after effects from a validated manifest, index, restore policy, and
  seal; it re-derives the expected layout from the manifest, requires the supplied index to equal that layout,
  enforces three-way digest agreement (`manifest.content_digest() == index.manifest_digest ==
  seal.manifest_digest`), and enforces exact two-way index/seal artifact equality;
- `CustodyRestorePolicyV1`, accepting only the inert v1 policy in §3.4;
- `CustodyEnvelopeFormatV1`, which records opaque capsule-format, sealing-tool, and sealing-tool-version identity
  but performs no cryptography;
- `CustodyEnvelopeContextV1`, which validates and canonically encodes the logical artifact name, manifest digest,
  capsule format, and canonical recipient set specified in §3.3;
- sealed `CustodyEnvelopeSealerV1` and `CustodyEnvelopeOpenerV1` port traits whose production implementations must
  accept that context and supply bounded opaque bytes and exact format/recipient identity; 2B1 supplies no
  implementation. The later deterministic fixture adapter must live inside `bridge-core` behind test/fixture
  support and be covered by in-crate unit tests; an external integration-test crate does not implement the sealed
  traits;
- typed refusal reasons that distinguish invalid input, non-sealable manifest, three-way manifest-digest mismatch,
  derived-layout mismatch, missing/duplicate/unmapped artifacts, coverage-state mismatch, unknown object kind, and
  unsupported restore behavior.

Add only the minimum immutable accessors needed to compare existing private Slice 2A records. All direct Serde
deserialization must route through the same validation path; unknown fields and non-canonical JSON refuse.

2B1 performs no filesystem access, Git invocation, encryption, decryption, allocation proportional to unbounded
input, CLI/config/store wiring, or source discovery.

### 4.3 RED-first requirements

Capture the current-base structural RED before production code. Then run and record at least these behavioral
mutations against compiling code, restoring each mutation:

1. omit the `worktree` coverage row from the mapping and prove the layout would otherwise accept it;
2. allow one seal artifact to remain unmapped and prove the post-effect binding would otherwise accept it;
3. temporarily disable the exact duplicate-class-ownership check and give two distinct canonical artifact names
   the same `coverage_payload(worktree)` role while retaining all required control roles; count the mutation only
   if the focused test changes from failing to passing, otherwise repair the fixture/branch before using it;
4. treat `unresolved` coverage as empty and prove the planner refuses before any later effect;
5. allow an active hook or executable-config restore value and prove the closed restore policy rejects it;
6. independently change the index manifest digest and the seal manifest digest while retaining the same artifact
   map, proving each foreign-generation substitution is refused; then substitute a different valid manifest while
   index and seal still agree on the old digest, proving the binding actually computes the manifest leg;
7. accept `unknown` as a proved object kind and prove layout construction refuses it.

Tests also cover seal-time/open-time envelope-context byte equality under unsorted and duplicate recipient input;
empty-object manifests; mandatory/non-mandatory Git-pack role cardinality; exact 3- and 17-artifact invariants;
`object_database=captured` with an empty inventory; zero-byte artifacts;
non-UTF-8 logical names, order-independent canonicalization, duplicate rows, wrong schemas, unknown fields,
non-canonical encodings, every constructor error branch, and digest sensitivity for every bound field.

### 4.4 2B1 done gate

2B1 is done only when all tests/gates in §8 pass, mutation evidence is durable, an independent hard-read-only review
approves within the cap, and its handoff binds the exact implementation commit. Approval authorizes neither 2B2
effects nor publication.

## 5. Child 2B2 — isolated export and Git closure

### 5.1 Inputs and authority

The exporter accepts an already validated capsule layout plus an explicitly constructed, generation-bound capture
capability. It must not accept a bare source path, rediscover bridge roots, or infer quiescence from repeated scans.
The capability owns retained source/scratch descriptors and can be constructed only after a supported snapshot or
exclusive managed-writer quiescence decision. The initial implementation may expose only a fixture constructor if
the production quiescence capability is not yet available; it must not replace that missing gate with a boolean.

Writes are limited to one preflighted, owner-private, empty scratch root. Publication is create-new/no-replace.
Each artifact is length- and total-budget bounded before and during streaming; the parent 10 GiB staging cap from
ADR-0041 is an upper policy ceiling, not an implicit default allocation or deletion grant.

### 5.2 Git export and closure proof

Use the exact manifest object identities as roots and export a pack that includes unreachable/orphan objects named
by the manifest as well as transitive dependencies. Source Git subprocesses must disable optional locks, hooks,
fsmonitor, replacement-object rewriting, prompts, protocols, lazy fetch, and ambient system/global/repository
config effects that are not explicitly required for object access. Shallow/grafted history and an unresolved
promisor boundary refuse export. The emitted pack must be self-contained, never thin, and must not retain an
external delta base.

Verify the result from a new isolated object database that has:

- no alternates file and no alternate-object environment;
- no remotes, credentials, network protocols, source path, sibling path, object cache, or promisor fallback;
- temporary internal refs only as needed to make every object root visible to strict connectivity checking;
- exact object-id and kind enumeration equal to the manifest object inventory, not merely a subset check;
- strict pack/index verification and a full connectivity check.

Any missing object, extra object not represented by the manifest inventory, kind mismatch, corrupt/truncated pack,
alternate dependency, lazy-fetch attempt, or subprocess ambiguity refuses the seal. Exit status alone is not proof:
the implementation parses and validates the produced object inventory and verification output.

### 5.3 Non-Git artifacts

Coverage bytes are read only through the capture capability and written through the envelope port. The exporter
hashes the final opaque bytes, constructs `CustodySealedArtifactV1` values from measured lengths/digests, constructs
the completed `CustodyCapsuleBindingV1` against those exact values, and publishes the exterior seal last. Source bytes and
identity are rechecked after reads; drift yields a typed incomplete/ambiguous outcome, never a successful seal.

### 5.4 2B2 RED and completion controls

Required behavioral controls include: missing reflog-only object; orphan blob absent from the pack; source alternate
removed after planning; injected extra packed object; corrupt/truncated pack; attempted lazy fetch; source identity
swap; destination symlink/path-prefix attack; duplicate/case-fold-colliding destination name; byte/entry/total cap
exhaustion; write/sync/publication fault at each boundary; and a no-source-mutation comparison.

The primary closure control renames the source and every alternate/cache out of reach before verifying the staged
capsule. The same fixture without one transitive object must fail in the same environment.

## 6. Child 2B3 — inert new-root restore and hidden-state proof

### 6.1 Restore boundary

Restore accepts a verified exterior seal, the matching capsule artifacts, an envelope-open capability, and one
preflighted new empty destination. It refuses existing/non-empty destinations and never overwrites, follows a
symlink, crosses an undeclared mount, touches the source, updates project refs, or contacts a remote.

Before materialization it verifies schema, manifest digest, artifact mapping, every encrypted artifact's exact
length/digest, the inert policy, and Git closure. Plaintext is bounded and written only beneath retained
destination descriptors. Partial restore remains a named partial local outcome and does not imply custody.

### 6.2 Two-plane reconstruction

Preserve raw behavior-affecting material as encrypted/read-only evidence, but create the usable restored repository
from a synthesized safe configuration. Do not run checkout, hook, filter, include, fsmonitor, credential, remote,
submodule, LFS, or workflow commands against archived configuration.

The active restored materialization must have hooks, executable config/includes, filters, remote protocols, lazy
fetch, and workflow resume disabled. External linked-worktree locations, nested repositories/submodules,
in-progress Git operations, and external evidence are restored as inert evidence/metadata unless a safe v1 active
form is explicitly specified and tested. No old absolute path becomes active by copying bytes.

Emit canonical `custody-restore.v1` binding the source generation, seal/manifest digests, new materialization
identity/path, exact verification results, and every deliberately disabled behavior. It is a local restoration
record, not `custody-verification.v1` and not deletion eligibility.

### 6.3 Required fixture

One real Git fixture must jointly contain: attached and detached ref evidence, a staged change, unstaged change,
tracked deletion, ignored and untracked files, symlink payload, executable mode, conflict-stage index entries,
assume-unchanged and skip-worktree flags, stash, reflog-only commit, orphan blob, annotated tag, replace ref,
in-progress-operation metadata, linked-worktree metadata, nested repository/submodule metadata, alternates, hooks,
config include, fsmonitor/filter commands, bridge evidence, and one excluded-reproducible output with its declared
dependency.

The test restores after the source, alternates, sibling, cache, HOME config, and credentials are unavailable. It
compares exact manifest-covered bytes/modes/index/object/ref/reflog state and proves planted hooks, filters,
fsmonitor, includes, remotes, and workflow-resume markers were not executed. Unsupported platform metadata parks
that fixture row; it is not silently normalized.

Negative fixtures cover one absent object, one missing external artifact, one undecryptable artifact, one altered
ciphertext byte, destination replacement, and attempted activation of an archived external path.

## 7. Explicitly excluded from all of Slice 2B

- provider/API/network calls, remote Git escrow, archive upload, retention claims, verifier signatures, or
  `custody-verification.v1`;
- project-repository publication, source-ref changes, merge behavior, branch cleanup, or worktree cleanup;
- automatic source discovery, historical migration, standing capture authority, scheduler/operator wiring, or a
  background service;
- quarantine, reap, deletion, tombstones, destructive journals, rollback of removal, or any deletion eligibility;
- production key discovery/storage, password-manager access, recipient policy, real encryption provider selection,
  or claims that test envelopes provide confidentiality;
- activation of restored hooks, config includes, filters, remotes, LFS/submodule fetches, linked-worktree paths,
  nested repositories, in-progress operations, or workflow execution;
- tar/zip extraction, compression, content deduplication, incremental capsules, multi-generation garbage
  collection, or overwrite/resume-in-place;
- running-operator restart/adoption or compatibility-matrix/live-agent spend.

## 8. Verification and review gates

Every child directly invokes its focused target, `bridge-core`, and the full locked/offline workspace, reporting
exact totals. Completion-gate commands are not piped through `tee` or another process that can mask their status:

```text
cargo test --locked --offline -p bridge-core --test <child-target>
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets
cargo test --locked --offline --workspace
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo deny check
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

2B2/2B3 additionally run their real-Git fixtures on host macOS and a native Linux filesystem such as ext4; a
container overlayfs result does not substitute for the native-filesystem identity-drift control. Platform-specific
exclusions are named with exact test counts and mechanisms. A fixture/setup/subprocess failure, zero-test selection,
or invalid flag is inadmissible and must be repaired before it informs the result. The exact pre-change artifact
must run in the same environment before attributing a new failure to the child.

Declare a **two-admitted-round review cap per child** before dispatch. Reviews are hard-read-only and report WRONG
before SMELL. A WRONG must name a constructible state/input, wrong result, mechanism, bounded repair, and realistic
failing regression. Closed enumerable findings are repaired on the same artifact. Open-class findings park the
child for design. At the cap, classify convergence before any extension; never restart and discard a partially
reviewed artifact.

The parent Slice 2B is complete only after all three children are approved, the aggregate fixture passes without
the source or alternates, the combined diff receives one bounded hard-read-only review, and a durable handoff
records exact commits, evidence, totals, exclusions, and deferred work. Local approval does not authorize push,
merge, provider effects, remote custody, cleanup, deletion, or operator adoption.

## 9. Stop conditions

Stop and return to the owner/spec when any of these occurs:

- a coherent snapshot or exclusive managed-writer quiescence cannot be represented without a boolean assertion;
- the object inventory cannot be proved equal to the isolated pack inventory;
- a required hidden-state class can only be restored by executing archived behavior;
- a source, alternate, sibling, cache, credential, or network dependency survives the isolation control;
- an effect requires a bare path instead of retained descriptor identity;
- a new dependency or archive/crypto format materially changes the threat model;
- any design needs remote authority, source/project mutation, overwrite, cleanup, quarantine, reap, or deletion;
- the diff grows beyond one child's owned seams, the finding population is open-class, or the review cap is
  exhausted without convergence.

## 10. Next action

Independently review this parent plan and the detailed 2B1 contract before implementation. If approved or repaired
within the cap, implement only 2B1 from exact base `27a885f6`; do not begin 2B2 filesystem/Git effects in the same
implementation dispatch.
