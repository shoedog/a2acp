# Bridge reliability execution and handoff roadmap

- **Program status:** active P0
- **R2a implementation baseline:** `main` at `24aff09c` on 2026-07-11
- **Completed through:** R2a provenance diagnostics
- **Next action:** R2b0 contract clarifications
- **Design of record:**
  [`superpowers/specs/2026-07-11-bridge-reliability-r2-design.md`](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md)
- **Operating runbook:**
  [`../skills/a2a-bridge-operator/SKILL.md`](../skills/a2a-bridge-operator/SKILL.md)

This file is the durable program cursor. A new session should be able to start here, find the exact
active slice, open its implementation plan, and continue without reconstructing the July 2026 incident
history. The detailed design remains normative for R2; this roadmap owns sequencing, status, handoff,
and completion evidence.

## Dependency graph

```text
R2a provenance (MERGED)
  -> R2b0 contract clarifications
  -> R2b1 diagnostic types + rollback-safe persistence surface
  -> R2b2 ACP/Fable lifecycle evidence + no-replay/warm-session safety
  -> R2b3 API/provider mapping + remaining container/dispatch observation
  -> R2c explicit one-turn billable smoke
       -> R2d local non-billable fallback plan
       -> R3 compatibility manifest + pinned/floating canaries
            -> R4 reproducible dependency/image pins + release promotion gate

R2e authenticated in-process fallback is DEFERRED and off the critical path.
It requires R2d plus a separately approved authenticated-policy/attestation design.
```

M4 Slice 3b/3c remains parked until the reliability exit gates in
[`roadmap.md`](roadmap.md) are satisfied. Do not mix retention work into these slices.

## Program status table

| Slice | Status | Durable plan | Merge boundary |
|---|---|---|---|
| R0 — front door/baseline | **MERGED** | [`bridge-reliability.md`](bridge-reliability.md) | Docs index, compatibility matrix, priority reset. |
| R1 — Fable isolation | **MERGED** | [R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md) | Host and reader controls dispositioned. |
| R2a — doctor provenance | **MERGED** at `24aff09c` | [R2 design](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md) | Additive non-billable provenance rows. |
| R2b0–R2b3 — structured diagnostics | **NEXT / NOT STARTED** | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Four reviewed, independently mergeable PRs. |
| R2c — live smoke | **NOT STARTED** | [R2c implementation plan](superpowers/plans/2026-07-11-r2c-live-smoke.md) | One explicit, bounded, billable turn; no retry. |
| R2d — fallback plan | **NOT STARTED** | [R2d implementation plan](superpowers/plans/2026-07-11-r2d-local-fallback-plan.md) | Local recommendation only; never executes fallback. |
| R2e — in-process fallback | **DEFERRED / BLOCKED BY POLICY** | [R2e gated plan](superpowers/plans/2026-07-11-r2e-policy-authorized-fallback.md) | No implementation until authenticated attestation design is approved. |
| R3 — compatibility canaries | **NOT STARTED** | [R3 implementation plan](superpowers/plans/2026-07-11-r3-compatibility-canaries.md) | Local manifest/runner first; scheduling requires runner/credential owner. |
| R4 — reproducible release policy | **NOT STARTED** | [R4 implementation plan](superpowers/plans/2026-07-11-r4-reproducible-release-policy.md) | Full resolution pins, candidate smokes, promotion and rollback. |

Allowed status values are `NOT STARTED`, `IN PROGRESS`, `IN REVIEW`, `MERGED`, `BLOCKED`, and
`DEFERRED`. Update this table in the same PR that changes a slice status. Never mark `MERGED` from a
local commit or open PR.

## Resume protocol for a new session

1. Fetch and verify the live default branch:

   ```bash
   git fetch origin main
   git rev-parse origin/main
   git status --short --branch
   ```

2. Read, in order:

   - [`docs/README.md`](README.md)
   - this roadmap
   - the active slice plan from the status table
   - the R2 design of record for R2b–R2e, or the R3/R4 plan for later work
   - [`compatibility.md`](compatibility.md) before any live agent or release claim
   - the operator skill before any bridge-mediated review or smoke

3. Confirm that every prerequisite slice is actually present on `origin/main`; do not trust this table
   if live history disagrees.

4. Create an isolated worktree from current `origin/main`. For the next slice:

   ```bash
   git worktree add /private/tmp/a2a-bridge-r2b0 \
     -b agent/reliability-r2b0-contract origin/main
   ```

5. Before editing, run the active plan's named pre-change regression. Every new behavior needs a test
   that fails on the pre-change code plus a negative or edge case.

6. Keep scratch configs, prompts, model output, SQLite stores, and review artifacts under
   `/private/tmp`; repository hygiene forbids committing one-off workflow material.

7. Before merge, update this roadmap with completion evidence and the next single action.

## Universal slice rules

### Safety and compatibility

- The public A2A error category remains redacted. Diagnostic detail is operator evidence, never an
  untrusted wire payload.
- No prompt may be replayed after the conservative prompt-acceptance barrier.
- Provider-capacity handling and container degradation are separate. Neither automatically routes to
  another provider or execution tier.
- Tier 2 remains mandatory for untrusted reads; Tier 3 remains mandatory for all write-capable work.
- Caller metadata, workflow input, config labels, and `AlwaysGrant` cannot assert trusted-content
  authority.
- Raw advertised capabilities win. Aliases and semver ranges are not provenance.
- Raw opaque stderr is never persisted or traced. Best-effort-redacted stderr remains explicit opt-in.

### Implementation discipline

- One numbered sub-slice per branch/PR unless the active plan explicitly permits coalescing.
- Use additive defaults at public traits and serialized records whenever rollback compatibility is part
  of the contract.
- Match every new enum exhaustively in retry, warm-session, projection, metrics, and serialization
  code. Wildcard arms must not hide a new failure class or event variant.
- Run the repository's complete suite, not only focused tests. Report exact passed/failed/ignored totals
  and every unexercised live gate.
- A failure outside slice scope is reported, not re-baselined or silently fixed.

### Required merge gates

Every implementation slice runs:

```bash
git diff --check
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --bin a2a-bridge
cargo run -p a2a-bridge -- validate --repo-hygiene
```

Docs-only R2b0 still runs repository hygiene and the full workspace suite because it changes the design
contract that later code will follow.

### Review and dogfood gate

- R2b–R2e require a fresh adversarial full-branch review through the bridge before merge.
- Prefer Fable/xhigh when its usage window has headroom. If it is degraded or near its limit, an
  operator may select `gpt-5.6-sol`/`max` as a new, separately recorded attempt. Never auto-resume or
  auto-route the first attempt.
- Every finding is tagged `WRONG` or `SMELL`; a `WRONG` finding names the constructible state and
  incorrect result. Prior findings are adjudicated before new findings.
- R3/R4 additionally require a release/compatibility reviewer focused on credentials, cost bounds,
  workflow permissions, immutable pins, and rollback.

## Completion evidence template

Fold this block into the active plan and this roadmap when a slice merges:

```text
Status: MERGED
Branch:
Commit:
PR or direct-main record:
Review model/effort and verdict:
Focused regressions:
Full suite totals:
Build/hygiene gates:
Live/billable gates run:
Live/billable gates not run:
Compatibility rows changed:
Remaining findings/deferred items:
Next action:
```

## Current handoff

- `origin/main` contains R2a at `24aff09c`.
- R2a's last full suite was 1,607 passed / 0 failed / 12 ignored live-agent tests.
- A fresh bridge-mediated Fable/xhigh review returned `R2A: READY`, `V6 DESIGN: READY`, `MERGE`.
- The Podman bare image-id normalization and non-vacuous descendant survivor-marker regression were
  folded after that review and revalidated by the full local gate set.
- No R2b production code exists yet.
- **Start with R2b0 only:** clarify direct inbound/coordinator observer ownership and enumerate every
  raw ACP SDK log site in the design. Land that contract patch before adding diagnostic types.
