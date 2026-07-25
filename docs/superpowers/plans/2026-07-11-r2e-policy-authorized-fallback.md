# R2e — Policy-authorized in-process fallback gated plan

- **Status:** DEFERRED — NOT IMPLEMENTATION-READY
- **Prerequisites:** R2b and R2d merged, plus an approved authenticated-policy milestone
- **Source design:**
  [`../specs/2026-07-11-bridge-reliability-r2-design.md`](../specs/2026-07-11-bridge-reliability-r2-design.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Branch:** none until the gate below is satisfied

R2e is intentionally off the reliability critical path. This file prevents a future session from
mistaking a reserved capability for approved work. `AlwaysGrant`, caller metadata, repository paths, git
remotes, workflow names, and agent configuration are not trust authorities.

## Current blockers

The current auth surface is insufficient:

- `AuthContext` carries only a caller id;
- `bridge-policy::AlwaysGrant` returns anonymous for both anonymous and tokened requests;
- no policy-issued content/action attestation exists;
- R2d has no executor by design;
- no durable two-attempt audit/cost contract has been reviewed.

Do not create an `auto_fallback`, `content_trust`, or equivalent config/request field as a shortcut.

## Gate to begin R2e0 design

All of the following must be true:

1. R2b's prompt-acceptance barrier is live across ACP, API, workflow, and warm paths.
2. R2d's local plan format and eligibility matrix are merged and security-reviewed.
3. An authentication design identifies a real caller and records authentication provenance in
   `AuthContext`; `AlwaysGrant` remains explicitly non-authoritative.
4. A policy layer can issue a non-caller-forgeable, non-serializable
   `TrustedOwnRepoReadOnly` attestation bound to that exact `AuthContext`, content scope, action class,
   repository identity, and expiration.
5. Attempt/cost/audit persistence can represent the primary and fallback as two distinct attempts.
6. An owner approves the threat model, audit semantics, cost bound, and rollback behavior.

Until then, status stays `DEFERRED`; ordinary difficulty or demand does not clear the gate.

## Future slice shape after approval

### R2e0 — threat model and contract only

- Define attacker capabilities: anonymous caller, token theft/replay, forged A2A metadata, malicious
  repository, symlink/path swap, confused deputy, stale attestation, and concurrent policy change.
- Define the attestation issuer, verifier, binding fields, expiry/revocation, and unforgeable Rust type
  boundary.
- Specify audit ordering, two attempt ids, two cost records, cancellation, crash recovery, and exact
  behavior when the fallback cannot start.
- Require independent security and data-safety approval before code.

### R2e1 — authenticated policy and attestation

- Extend auth without weakening anonymous compatibility paths.
- Add policy issuance/verification ports; callers cannot deserialize or construct the attestation.
- Bind to canonical repository identity plus read-only action class; paths/remotes alone are
  insufficient.
- Persist only bounded audit evidence, never bearer tokens or secret policy inputs.

### R2e2 — two-attempt executor

- Consume a validated R2d eligibility result and live attestation.
- Persist the audit decision before starting the second attempt.
- Allocate a distinct attempt/task id and provenance/cost record.
- Enforce a hard two-attempt maximum and no replay after either prompt barrier.
- Never change Tier 2 for untrusted reads or Tier 3 for writes.

### R2e3 — adversarial and live gates

- spoofed/missing/expired/wrong-caller/wrong-repo/wrong-action attestations fail closed;
- `AlwaysGrant`, config, workflow input, and arbitrary A2A metadata cannot authorize;
- primary possibly accepted means no fallback;
- audit failure prevents the second attempt;
- crash between audit and spawn is recoverable without duplicate execution;
- cancellation and concurrent policy revocation have deterministic behavior;
- costs and usage stay distinct and attributable;
- one explicitly approved trusted-own-repo read-only live exercise proves the full path.

## Non-goals

- automatic provider/model routing;
- host fallback for untrusted reads or any write-capable task;
- inferring trust from ownership, path, branch, remote, prompt text, or model choice;
- resuming the original attempt;
- more than one fallback attempt;
- weakening R2d's local-plan-only behavior before this plan is approved.

## Completion

R2e is complete only after all four future slices merge under a separately approved design. It is not a
prerequisite for R3, R4, or resuming M4 once the other reliability exit gates pass.
