# Compatibility scheduling foundation

R3d0 is a default-off, non-billable contract layer for future owner-operated compatibility scheduling. It
adds checked-in policy, inventories, canonical identities, strict record schemas, and local validators. It
does not install or invoke a timer, read credentials, discover models, access a registry or container
runtime, call a provider, publish a GitHub check, issue private authority, or touch the long-lived production
operator.

## Checked-in inputs

- `compatibility/scheduling-policy.toml` is non-authoritative policy. It records the approved trigger/effect
  classes, exact owner/environment identity, the `/Users/wesleyjinks/code` trusted repository-cwd root,
  profile maxima, price-ranking contract, and 10 GiB hot plus 25 GiB owner-iCloud cold-storage allocation.
- `compatibility/scheduled-cases.toml` contains six provider-minimal advisory rows. Every row starts at
  `characterization_required`, is classified `canary`, and lives outside the production support manifest.
- `compatibility/characterization-profiles.toml` is the complete initial inventory: the six advisory
  profiles plus the four exact D7-reachable production support profiles.
- `compatibility/configs/{codex-luna-*,claude-haiku-*,claude-sonnet-low-host,ollama-local}.toml` are proposed
  characterization configs. They grant no execution authority.

Validate the complete set locally:

```bash
a2a-bridge compatibility validate --schedule-foundation compatibility
```

Validation pins the foundation-root directory identity, uses bounded regular-file/no-follow reads, rejects
any resolved child path outside that root, and retains each file's descriptor object/change identity as well
as its path and digest. The final pass compares every capture, including repeated canonical paths, and
requires all four to remain unchanged; same-byte atomic replacement and mixed file-object generations fail
closed. Every scheduled-advisory and claimed-support `session_cwd` must be absolute, traversal-free, and at
or below the exact trusted repository root. When that owner root is mounted, validation canonicalizes the
root and each cwd, requires a real directory whose resolved object remains inside the root, and binds the
resolved path into profile identity; an in-root symlink to an outside directory fails before inventory
comparison. A non-owner/offline validator on which the exact owner root is absent retains only the static
absolute, traversal-free path identity so deterministic no-effect CI remains possible. That offline result is
not execution proof: R3d2 must repeat real-directory/object containment immediately before admitting any
effect. Validation raw-scans comments and values before parsing, recognizes structured credential-key
delimiters, never invokes the runtime config parser or expands environment variables, applies strict TOML/
recipe schemas and exact inventory coverage, and checks provider, adapter/command, auth/pre-auth/API-key
environment, resolution recipe, endpoint, arguments, server, mount, egress, network, proxy, credential
volume, config-template, and effect agreement. Scheduled `required_env` uses the same credential-shaped-name
classifier as the production manifest and cannot repeat `credential_env`; ordinary non-secret prerequisites
remain valid. Claimed-support config
bytes must match their exact production-manifest pin and cannot bypass those semantic constraints by updating
the pin. The result is a canonical semantic profile-policy bundle: comments and set/row ordering do not
affect it, while material policy, recipe, or template changes do. Every canonical hash includes an explicit
versioned identity-kind domain.

The profile-policy bundle deliberately excludes exact candidate bytes, test-merge/main targets, generated
run manifests, package versions, and image/config digests. Those are execution identities. Changing an
exact package or immutable-image pin therefore changes the future case-execution fingerprint without
invalidating an unchanged characterized profile or standing grant. Changing model, effort, capability,
expected status, evidence path/probe, allowed effects, environment/auth shape, prompt/template, semantic
policy/recipe constraint, artifact policy, or a maximum cap changes the profile and requires
characterization again.

Claude's `haiku` and `sonnet` values are the raw selector ids advertised by the ACP adapter at this design
baseline, even though they look like friendly aliases. Characterization must record and check the observed
provider-effective mapping. A catalog or mapping change is drift to evaluate; it is never an implicit model
substitution or automatic pin update.

## Strict JSON records

Validate one owner-side record without performing its action:

```bash
a2a-bridge compatibility validate --schedule-record <kind> /absolute/path/to/record.json
```

Supported kinds are:

- supervision: `deadline-derivation`, `supervisor`;
- canonical identities: `case-execution-fingerprint`, `admission-attempt-fingerprint`;
- authority and controls: `characterization-authorization`, `provider-effect-grant`, `manual-admission`,
  `storage-consent`, `characterization`, `safety-hold`, `quarantine`, `failure-disposition`;
- classification and accounting: `impact`, `ledger`, `equivalent-work`, `consumption`;
- sealed sources and evidence: `scheduled-source`, `claimed-support-characterization-source`,
  `schedule-sidecar`, `publication-outbox`, `evidence-index`, `status`, `routing`.

The R3d1 deadline record binds one checked sum of metadata, build, preflight, resolution/materialization,
per-selected-case, publication, cold-handoff, cleanup-grace, and fixed-margin maxima to the elapsed time since
the process-entry monotonic origin. Runtime derivation rounds elapsed time up and never gives the executable
deadline more time than the serialized record. Each phase runs under the earlier of its local maximum or the hard
deadline after reserving every later phase, cleanup grace, and fixed margin; an exhausted phase refuses before
polling even an immediately-ready effect. The record refuses overflow, a consumed deadline, duplicate/zero case
timeouts, or a schedule/grant/accounting window shorter than the remaining hard bound.

The R3d1 supervisor record binds exact PID/start/parent/group/session identities, retained-anchor lifecycle,
journaled TERM/KILL ordering and cause, the no-later-group-signal mark, safety holds, exact container run labels,
and the child artifact's run/window/hash join. Numeric PIDs are unique across scheduler, anchors, and workloads;
the runner must be one exact workload. `Prepared` owns at least one retained empty anchor, operational and terminal
states require coherent runner/group topology, every non-hold group stays in the exact scheduler/runner session,
and every hold retains at least one group. A session, ancestry, liveness, or identity-observation failure after descendant-anchor
acquisition appends that exact group to the durable hold before disabling later signals; an escaped or
observation-ambiguous group may therefore be recorded only in `SafetyHold`, never in an operational phase. KILL
terminates as exactly `killed_after_deadline` or
`killed_after_cancellation` according to its write-once cause. The standalone `supervisor` validator checks one
snapshot. The runtime journal additionally requires a prepared generation 1, a contiguous hash chain, strictly
advancing record time, an explicit monotonic phase graph, immutable run/deadline/scheduler/container identity,
append-only exact groups, one-way anchor lifecycle, and write-once signal/artifact/outcome fields both on append and
startup reopen. Reopen reads each named generation through the retained directory descriptor with no-follow and
verifies descriptor identity before and after its bounded read.

The retained, unreaped anchor child is the process-group capability: its PID/PGID cannot recycle before reap, so a
late liveness-observation error cannot suppress necessary cleanup. Descendant registration revalidates every exact
runner, workload, and anchor identity before trusting numeric parent links. Prepared recovery resumes only when its
retained group is still exact and contains no possible workload; any observed member or ambiguity becomes a durable
hold. Before success, the supervisor descriptor-pins and hashes the actual child join and optional aggregate bytes,
checks their run/window/hash bindings, parses and validates the unchanged aggregate, and releases anchors only after
that private verified-artifact capability exists. These schemas and journal checks do not themselves authorize or
execute work.

All records are versioned, deny unknown fields, raw-scan every decoded object key and string for secret-shaped
material, and use bounded local file reads. Git object identities are non-null tagged SHA-1/SHA-256 object IDs rather than
content SHA-256 digests, so current 40-character GitHub `merge_commit_sha` values are representable; every
object ID in one repository target must use the same repository object algorithm. Reusable
case-execution input contains the exact target/candidate/manifest/config/pin/resolution/
package/image/environment/cap bindings but structurally cannot contain a trigger or authority. The separate
admission-attempt input binds that execution to exactly one tagged authority plus request/window/attempt and
optional repeat identity. Optional security-relevant values use tagged `absent`/present forms instead of an
omitted field.

Provider grants require one exact label/plist binding for each daily or test-merge launchd trigger, and the
component-wise sum of scheduled, test-merge, and manual-unallocated pools must fit both UTC-day and rolling
ceilings. Generic manual authority is a closed direct-local-CLI `compatibility_run` record and cannot request
characterization. A reviewed completed characterization may later satisfy advisory work only through its
tagged record identity, freshness observation/bucket, review identity, and coherent terminal/review/
consumption times. Holds locally bind a canonical opening hash and canonical clearance-action identity.
R3d0 quarantine closure validates only the shape of its `opening_sha256`; R3d3 storage, under the owner lock,
must dereference the immutable opening record and verify that hash before accepting a close. Until that
cross-record check exists, a locally valid closed-quarantine record is not sufficient clearance. A second
untyped failure remains conservatively retained rather than becoming suppressible.

Impact records enforce the initial claimed-support due-case matrix: documentation/tests-only changes make no
provider case due, container changes bind both reader profiles, ACP/core changes bind the full four-profile
set, and new providers remain `characterization_required`. Evidence-index paths are ASCII-portable,
case-folded for uniqueness, root-relative components bound to explicit hot/cold root identities; duplicate
hot or cold ownership is invalid. Status enforces `last_window <= generated_at < next_window`, preserves
revocation as a durable cause even after expiry, requires consent-consistent cold state, and requires an
active provider grant plus a next window for active scheduled/gate cases. Routing records accept only the
exact checked-in task matrix.

The sealed scheduled and claimed-support sources embed both canonical records and cross-check their source
row, identity, config, caps, authority, and trigger. The sidecar separately binds reservations, equivalent
work, consumption, controls, deadline/preflight/supervisor/freshness evidence, and publication state while
leaving the existing R3 aggregate schema byte-compatible.

Publication-outbox check, terminal, guard, and observation values use tagged absences. The stable outbox id
is derived from a domain-separated fingerprint over repository, PR, test-merge object, context, App, and
external id. Every post-intent record carries its predecessor hash, and a learned check-run id has its own
binding to that immutable identity. Terminal fields are all absent before `prepared` and all present from
`prepared` onward; remotely observed states additionally require a bounded nonzero observation-attempt
count. Partial, nullable, unchained, or identity-mutated encodings are invalid.

Passing a validator proves only that bytes satisfy the R3d0 contract. It does not prove current provider
compatibility, authorize an effect, consume a nonce, reserve a budget, characterize a profile, or enable a
required check.

## Later slices

- R3d1 implements provider-free one-shot supervision and signal parity with fake-process proof.
- R3d2 implements private authority, admission, preflights, equivalent-work, and durable accounting.
- R3d3 implements evidence storage, retention, status, and crash-consistent publication state.
- R3d4 implements disabled-by-default launchd and trusted test-merge/main triggers.
- R3d5 separately characterizes every inventory row under single-use owner authority before any staged
  enablement.

The approved design of record remains
[`superpowers/plans/2026-07-11-r3-compatibility-canaries.md`](superpowers/plans/2026-07-11-r3-compatibility-canaries.md).
