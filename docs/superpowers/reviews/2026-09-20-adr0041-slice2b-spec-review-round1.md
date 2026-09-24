# ADR-0041 Slice 2B spec review — round 1

**Date:** 2026-09-20  
**Reviewer lane:** Claude ACP `opus[1m]`, effort `high`, mode `plan`, hard read-only  
**Execution:** `exec-4206c5c65ea2dd11959c4c1b34546e2b` / `attempt-b299fb54e0b360d808021749f8d3291c`  
**Reviewed commit:** `522aac0793a4c5c0a6c07a6cf57a25b55ea517d5`  
**Reviewed plan SHA-256:** `24a99946f4bab284c11089ea4b1f0b32eeb4fc7e13734e096a1d986e3973784a`  
**Round cap:** 1 of 2 admitted rounds consumed  
**Verdict:** `REJECT`, closed enumerable population

## WRONG findings and disposition

1. **WRONG — authenticated envelope context was prose-only.** A seal could use caller-order recipients while the
   stored seal canonicalized them, making every artifact undecryptable at restore. **Folded:** the plan now requires
   `CustodyEnvelopeContextV1`, canonical recipient-set encoding identical at seal/open time, and a discriminating
   equality regression.
2. **WRONG — the index manifest digest was written but not checked.** A foreign-generation index could bind when
   manifest and seal agreed. **Folded:** post-effect binding now requires manifest/index/seal three-way digest
   equality, re-derives the layout, and mutates each foreign digest independently.
3. **WRONG — `object_database=captured` plus an empty object inventory was both admitted and incompatible with the
   total-mapping outcome.** **Folded:** the state/inventory relationship is bidirectional and the exact empty case
   is a required refusal regression.
4. **WRONG / DEFER — the duplicate-ownership mutation could be refused by a different validator and therefore fail
   to exercise its claimed branch.** **Folded before implementation:** the mutation must use distinct names and is
   admissible only when disabling the exact branch changes the focused test result.

## SMELL findings and disposition

- The sealed-trait/test-adapter boundary is now explicit: the later fixture adapter lives inside `bridge-core` and
  is tested in-crate rather than implemented by an external integration-test crate.
- Post-effect binding now re-derives and compares the manifest-derived layout instead of trusting an internally
  consistent index/seal pair.
- The full-workspace gate now runs both `--all-targets` and the bare workspace test command so doctests are counted.
- Exterior path/type claims are scoped to the capsule exterior; interior coverage framing remains a reviewed 2B2
  decision.
- Envelope format identity now names capsule format, sealing tool, and sealing tool version.
- The 3/17 artifact bounds are specified as derived invariants, not an unreachable caller-selected cap error.
- The unmapped-artifact mutation is correctly assigned to post-effect binding.
- The later Linux identity-drift gate requires a native filesystem such as ext4; overlayfs is not a substitute.

## Convergence classification

The population is closed and shrinking: three blocking contract omissions and one evidence-control correction,
with no open-class design finding. The repaired artifact remains the same plan; one admitted review round remains.
Implementation must not start until that round approves.
