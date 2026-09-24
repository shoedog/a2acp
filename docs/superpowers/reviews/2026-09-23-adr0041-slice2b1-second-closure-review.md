# ADR-0041 Slice 2B1 second-repair closure disposition

**Date:** 2026-09-23
**Review cap:** one of one, exhausted
**Base:** `80d4586828aca7d5959b3931f3d30d2cd593e662`
**Reviewed candidate:** `8d9d4c45d93ef6441aef52fea2fc35db4b2316b9` (tree `e8f153093ce5101abfbac6fb5ce29c957ca07658`)
**Run:** `exec-6fb6dc2200ad44563d487e3e2ee96408` / `attempt-0c3b24562fa60b0e6c9fd6dd1e850641`
**Reviewer:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only`; `codex-acp` 1.8.0; Codex 0.153.0
**Raw result:** `/private/tmp/a2a-bridge-slice2b1-closure2-20260923/result.md` (SHA-256 `574c9481eb6a9fe4a862b55d001b5fe57142f472577b38c5a3a044a7fa7c46bb`)
**Verdict:** **REJECT** — inherited `RESOLVED 2 / UNRESOLVED 1 / DEFERRED 0`; new `WRONG 0 / SMELL 4`; dispositions `BLOCKER 1 / DEFER 4`.

## WRONG

1. **WRONG / BLOCKER — R1 stream-to-seal provenance remains UNRESOLVED.**
   `CustodyEnvelopeSourceValidatorV1::finish` and `CustodyEnvelopeSinkValidatorV1::finish` both mint the
   same public `CustodyEnvelopeStreamReceiptV1`, and public `CustodyEnvelopeSealReceiptV1::new` accepts that
   undifferentiated type. A caller can hash plaintext or decoy bytes through the source validator, pass that
   receipt as ciphertext evidence, assemble `CustodyCapsuleSealProofV1`, and obtain an accepted capsule binding
   whose sealed digest does not describe emitted ciphertext. Private receipt fields prevent struct literals but
   do not prove sink provenance. This violates the plan's rule that no public constructor may label plaintext
   as encrypted or manufacture a production seal. The reviewer rated the future 2B2-adapter trigger plausible,
   impact high, confidence high, and repair cost medium/module-local.

   Bounded repair shape: separate the ciphertext-sink capability from general source receipts and prevent a
   public caller from directly manufacturing a seal receipt. Construction must be tied to the sealed in-crate
   sealer/sink path that observes successfully emitted ciphertext. A RED must show that a source/decoy receipt
   or bytes B cannot authorize a seal for ciphertext bytes A; the positive control must bind A exactly.

Controller re-read confirmed the mechanism: the two public `finish` methods return the same type and the
public seal-receipt constructor accepts it without a sink-origin marker. The prior R1 mutation demonstrated
only that the constructor copied its supplied receipt; it did not discriminate receipt provenance.

## SMELL

1. **DEFER:** no compile-fail proof that generic `CustodySealV1` cannot substitute for the private capsule
   proof, no direct empty-receipt-population test, and no signature-reversion mutation.
2. **DEFER:** allocation/exact-boundary coverage is narrow; the zero-allocation test covers the 65-recipient
   edge but not the other exact v1 count/byte boundaries. `CustodyEnvelopeFormatV1::from_seal` also retains
   allocating generic validation, though no current production caller makes that a demonstrated wrong output.
3. **DEFER:** open-side receipt/metadata binding remains intentionally deferred to the first in-crate adapter.
4. **DEFER:** the reviewed candidate's roadmap/handoffs still said amendment/review was pending. This docs-only
   custody fold reconciles that status without changing reviewed Rust bytes.

## Resolved mechanisms and boundary

- **R2 RESOLVED:** the private capsule proof derives the generic seal from a nonempty receipt population,
  enforces shared manifest/format/tool/version/recipient identity, and generic seals no longer satisfy binding
  or open-request signatures.
- **R3 RESOLVED:** capsule-wide limits precede borrowed canonical validation; the mutation produced 88 counted
  allocations versus zero and the restored malformed-order negative remains discriminating.
- The review node completed with clean terminal and cleanup evidence. Automatic LSP language detection skipped
  the multi-language root, so the reviewer used authorized read-only Git/search instead.
- No repair, rereview, push, merge, cleanup, 2B2/2B3 effect, publication, or running-operator mutation is
  authorized by this disposition. Preserve the reviewed code commit and this docs-only custody successor.

**Final verdict:** REJECT — R1 remains a production-contract blocker; the one-review cap is exhausted.
