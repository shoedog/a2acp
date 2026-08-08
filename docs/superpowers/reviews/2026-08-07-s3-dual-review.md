# S3 build-target reaper — dual-lens review record

Date: 2026-08-07. Artifact: `feat/s3-target-reaper` @ `1019ac2` (base `82c551b`). Two parallel lenses,
one declared round: Opus/high senior-lead posture (owner-directed: engineer protecting the project, not a
contract lawyer; hard scrutiny reserved for the destructive path) and gpt-5.6-sol/high via the bridge's
own `code-review` workflow (dogfood + second opinion).

## Verdicts

- **Opus senior-lead: REVISE** — one required finding, six DEFER smells, explicit endorsements (the D-4
  resume exemption "is the right call"; the ReapEnv seam, volume-identity-first refusal, truthful
  Partial/Unknown, and post-removal downgrade all credited; "nothing here reads as over-engineered").
- **Sol/high: REJECT** — six blocker-graded WRONGs, one deferred smell.

## Orchestrator adjudication (primary-evidence tie-break per house rule)

**Converged, fixed (R1/R2):** the live-run hole — the operation lock is inert against a live initial
`implement` (it holds only a run lease; `.operation-locks` is resume/merge-only), `run_lease` is
hardwired `Unknown` for implement items, and host `lsof` cannot see container-side consumers. Fix:
pid-alive park (pid embedded in the run dir name, fail-closed), affirmative-runtime-answer required for
the container axis, honest wording (the invariant is "idle run / no live consumer", not "completed run").
Checkpoint-phase gating was rejected: it would permanently strand crashed-`InLoop` runs — the exact
population the reaper exists to clean — and adds nothing over pid-alive for the live-consumer invariant.
Also converged: name-only `DependencyCache` classification → cheap provenance markers required
(`package.json` sibling, `pyvenv.cfg` inside `.venv`).

**Sol-only, upheld (R3/R4, both small):** signal-terminated `lsof` with empty output read as `Free` →
typed exit-status handling, park on abnormal termination; no crash-durable evidence boundary → fsync'd
intent record before first removal + outcome receipt written before the lock guard drops + receipt-write
failure fails the command.

**Sol-only, overruled/narrowed (owner-concurred):** descriptor-relative recursive deletion (a 2–4 day
`openat`/`unlinkat` rewrite) against an intermediate-swap race requiring a hostile concurrent host-level
actor during an operator-invoked command — the existing canonical-path + dev/ino rechecks and the
post-CVE-2022-21658-hardened `remove_dir_all` bound the realistic surface; **deferred to A4**, where
`fs_custody` grows the right primitives and the reaper adopts them.

**Opus ergonomics batch, fixed (R5):** dry-run banner names its one state-visible effect (lock-namespace
creation); gate evidence rendered in text mode under `--dry-run`; one `lsof` probe per run directory plus
per-item progress (a silent O(tree) walk reads as a hang); comment/wording nits.

## Carried to the ledger

Descriptor-relative deletion (A4); D-4 admission CLI fixture test; `ReportItem` needs a discriminated
source/kind field so destructive code stops inferring volume-vs-path (S4); worktree payloads have no
operation-lock boundary — S4 needs a lease-based one; `is_cargo_target` marker plantability remains
narrowed-not-closed; `storage report`'s consumer line is never reap-eligibility — every consumer of the
classifier must probe for itself at its own boundary.
