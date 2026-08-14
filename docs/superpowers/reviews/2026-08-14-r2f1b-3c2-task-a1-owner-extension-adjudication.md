# R2f1b 3c2 Task A1 owner-extension adjudication

Date: 2026-08-14

Landed base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

Superseded candidate: `6616753bf479d8775381eb9ef1d7237f5660514c`

Closure-approved candidate: `5cbeea1ed882afe448d3825984af9a3ed74bcb58`

Retained clone:
`/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5`

## Verdict

**THE OWNER-AUTHORIZED A1 CONTINUATION IS APPROVED. BOTH INHERITED WRONGS ARE
FIXED, WITH NO NEW WRONG OR SMELL. THE CANDIDATE IS RETAINED AND NOT INTEGRATED;
A2 REQUIRES A SEPARATE OWNER PROGRAM CHOICE AND DISPATCH AUTHORIZATION.**

This was an explicit disclosed extension after the earlier cap, not a restart.
It continued exact commit `6616753b` for one implementation turn, at most 60
changed production lines / 180 total, one closure review, and zero further
repair loops. The exact result changes one file by 30 production and 139 total
lines.

## Bounded repair and regressions

The continuation closes exactly two constructible failures:

1. unexpected successful capture no longer performs a name-based restoration;
   it issues one target-to-custody rename, leaves protective debt, and returns
   `Unknown`; and
2. every post-attempt capture `Io` returns `Unknown` without a target or custody
   identity probe, so hard-link-back cannot falsely establish no effect.

The new deterministic regressions substitute custody at the old restoration
boundary and inject error-after-effect plus hard-link-back. They assert one
rename, no C-to-target move, one pre-capture probe only, both captured names
where applicable, and `Unknown`. Inspection of `6616753b` proves both are
parent-breaking because that parent performs the second rename and post-error
identity probe.

## Exact execution and gates

Implementation workflow: `exec-7407e63d87b09366e203d1b3c5d73ebe` /
`attempt-5dc1b6f65aa336e35b2907207d0ed67d`.

The implementation container's Cargo probes were registry-blocked and are not
used as acceptance evidence. Operator host gates on exact committed candidate
`5cbeea1e` passed:

- `custody_v2`: 11 passed / 0 failed;
- `fs_custody`: 77 passed / 0 failed;
- workspace all-feature suite: **3,999 passed / 0 failed / 13 ignored**;
- locked all-target/all-feature check and warnings-denied Clippy: exit 0;
- locked release `a2a-bridge` build: exit 0;
- `cargo deny check`: advisories, bans, licenses, and sources okay;
- repository hygiene: 40 tracked artifacts / 8 example configs;
- formatter, staged diff, unstaged diff, and whitespace checks: exit 0.

Actual Windows compilation/execution was not rerun and remains an explicit
verification exclusion. The change is inside the Unix implementation/tests;
the preserved non-Unix callable `CompileUnsupported` stub is source-unchanged.

## Closure and program boundary

The one hard-read-only Sol/xhigh closure was
`exec-b0c32fba22f61b89edbdb4a880e5f22a` /
`attempt-304c87ee714e2c212160a84785a39b3b`. It explicitly marked both inherited
WRONGs **FIXED**, found **0 WRONG / 0 SMELL**, and returned `VERDICT: APPROVE` at
97/100 confidence. Its durable mirror is the
[owner-extension closure](2026-08-14-r2f1b-3c2-task-a1-owner-extension-sol-closure.md).

Approval establishes an acceptable inactive A1 candidate only. It does not
integrate the candidate, fold the rejected 3c2 feature, approve the proposed
program split, dispatch A2, arm production V3, advance 3d, push, run CI, invoke
a provider beyond the bounded bridge turns, release, deploy, or mutate the
running operator. Until the owner selects the program direction, the formal
ten-task plan remains recorded; if continued, A2 freezes exact input
`5cbeea1e`. If the split is approved, A1 remains dormant retained salvage and
Shield S1 starts independently from the then-current green main.

The two-field cleanup carry-forward remains binding in whichever later slice
first arms production V3 or wraps `ContainerRw`.
