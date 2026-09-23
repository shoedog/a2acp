# ADR-0041 Slice 2B1 provenance repair review

**Date:** 2026-09-23
**Review cycle:** 1 of at most 2; approved, so cycle 2 is not opened
**Base:** `80d4586828aca7d5959b3931f3d30d2cd593e662`
**Reviewed code/docs candidate:** `88013eb4408d5afecb0b9101ef43c9c398226ef5` (tree `6159fa2f95e95d5976f7738e5a5e49d7ecf4f48a`)
**Run:** `exec-e7ee36345656679aec8923c4be9d25b6` / `attempt-86036c5afb97c003d9f62bb2fc0c9163`
**Reviewer:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only`; `codex-acp` 1.8.0; Codex 0.153.0
**Raw result:** `/private/tmp/a2a-bridge-slice2b1-provenance-review1-20260923/result.md` (SHA-256 `ae10a401186fc9933a959a17ff6da1c8d5727f0e2ee79440ab63306663fb2eb0`)
**Verdict:** **APPROVE** — inherited `RESOLVED 3 / UNRESOLVED 0 / DEFERRED 0`; new `WRONG 0 / SMELL 2`; dispositions `BLOCKER 0 / DEFER 5`.

## WRONG

None.

## Resolved inherited mechanisms

- **R1 RESOLVED:** source completion remains `CustodyEnvelopeStreamReceiptV1`; sink completion is the distinct,
  private-field `CustodyEnvelopeCiphertextReceiptV1`; the only production seal-receipt constructor is crate-private
  and consumes the sink receipt. No alternate constructor, trait implementation, adapter, production caller, or
  served projection exists. Exact accepted sink bytes feed the incremental length and SHA-256.
- **R2 RESOLVED:** the private capsule proof remains nonempty and receipt-derived, enforces shared
  manifest/format/tool/version/recipient identity, and remains required by binding/open APIs.
- **R3 RESOLVED:** capsule-wide limits still precede borrowed canonical validation; the provenance repair did not
  alter this path.

Both API controls discriminated independently: reconnecting source completion to the ciphertext receipt made only
the source-substitution compile-fail doctest fail, and making the seal constructor public made only the
constructor-privacy compile-fail doctest fail. Each mutation produced 4 passed / 1 intended failure; the restored
doctests passed 5/5. The reviewer correctly treated the container's offline `arc-swap` failures as inadmissible.

## SMELL

All are non-blocking and deferred:

1. **New:** test relocation dropped public negatives for oversized capsule format, oversized selected-artifact
   length, duplicate index names, and an empty index. Restore these before or with the first dependent 2B2 change.
2. **New/recurrent:** candidate status documents described committed candidate `88013eb4` as pending commit/review.
   This docs-only successor repairs that custody state before publication.
3. **Retained:** add compile-fail generic-seal substitution, a direct empty-receipt test, and a signature-reversion
   mutation before API widening.
4. **Retained:** exact-boundary/allocation coverage remains narrow; add max/max+1 tables and allocation controls as
   the effect adapter approaches.
5. **Retained:** opener receipt/metadata binding belongs to the first 2B3 adapter and remains unreachable now.

The review node completed with clean terminal and cleanup evidence. It did not rerun tests or modify the candidate.
The external planning-worktree contract was not read because the reviewer kept its repository read boundary; the
task embedded the governing repair requirements, and the reviewer read the in-repository parent plan, prior
disposition, full base-to-candidate diff, and changed files.

Approval authorizes the already owner-approved push and pull-request creation only. It does not authorize merge,
2B2/2B3 effects, cleanup, release, deployment, or running-operator mutation.

**Final verdict:** APPROVE — R1, R2, and R3 are resolved with no blocker; 0 WRONG and 5 deferred SMELLs remain.
