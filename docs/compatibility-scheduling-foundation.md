# Compatibility scheduling foundation

R3d0 is a default-off, non-billable contract layer for future owner-operated compatibility scheduling. It
adds checked-in policy, inventories, canonical identities, strict record schemas, and local validators. It
does not install or invoke a timer, read credentials, discover models, access a registry or container
runtime, call a provider, publish a GitHub check, issue private authority, or touch the long-lived production
operator.

## Checked-in inputs

- `compatibility/scheduling-policy.toml` is non-authoritative policy. It records the approved trigger/effect
  classes, profile maxima, price-ranking contract, and 10 GiB hot plus 25 GiB owner-iCloud cold-storage
  allocation.
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

Validation uses bounded, regular-file/no-follow reads; rejects any resolved child path outside the canonical
foundation root; applies strict TOML schemas and exact inventory coverage; checks config/row agreement; and
derives canonical semantic profile projections plus a stable profile-policy-bundle hash. It reuses the
existing strict production-manifest validator but does not change that manifest.

The profile-policy bundle deliberately excludes exact candidate bytes, test-merge/main targets, generated
run manifests, package versions, and image/config digests. Those are execution identities. Changing an
exact package or immutable-image pin therefore changes the future case-execution fingerprint without
invalidating an unchanged characterized profile or standing grant. Changing model, effort, capability,
environment/auth shape, prompt/template, policy/recipe constraint, artifact policy, or a maximum cap changes
the profile and requires characterization again.

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

- canonical identities: `case-execution-fingerprint`, `admission-attempt-fingerprint`;
- authority and controls: `characterization-authorization`, `provider-effect-grant`, `manual-admission`,
  `storage-consent`, `characterization`, `safety-hold`, `quarantine`, `failure-disposition`;
- classification and accounting: `impact`, `ledger`, `equivalent-work`, `consumption`;
- sealed sources and evidence: `scheduled-source`, `claimed-support-characterization-source`,
  `schedule-sidecar`, `publication-outbox`, `evidence-index`, `status`, `routing`.

All records are versioned, deny unknown fields, reject secret-shaped string material, and use bounded local
file reads. Reusable case-execution input contains the exact target/candidate/manifest/config/pin/resolution/
package/image/environment/cap bindings but structurally cannot contain a trigger or authority. The separate
admission-attempt input binds that execution to exactly one tagged authority plus request/window/attempt and
optional repeat identity. Optional security-relevant values use tagged `absent`/present forms instead of an
omitted field.

The sealed scheduled and claimed-support sources embed both canonical records and cross-check their source
row, identity, config, caps, authority, and trigger. The sidecar separately binds reservations, equivalent
work, consumption, controls, deadline/preflight/supervisor/freshness evidence, and publication state while
leaving the existing R3 aggregate schema byte-compatible.

Publication-outbox check, terminal, guard, and observation values use tagged absences. Terminal fields are
all absent before `prepared` and all present from `prepared` onward; remotely observed states additionally
require a bounded nonzero observation-attempt count. Partial or nullable encodings are invalid.

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
