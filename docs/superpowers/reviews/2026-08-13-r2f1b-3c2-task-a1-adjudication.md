# R2f1b 3c2 Task A1 implementation adjudication

Date: 2026-08-13

Landed base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

Frozen A1 input: `517703cbd2e469bf208f20a36248169536bca8b3`

Rejected A1 candidate: `bc262ad466b45470cd44fceda8224a36b2ba77b2`

Retained clone:
`/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5`

## Verdict

**A1 IS PARKED FOR TARGETED SALVAGE REDESIGN. PRESERVE THE CANDIDATE; DO NOT
SCRAP, INTEGRATE, OR DISPATCH A2.**

The A1 candidate is dormant and non-integrable. It contains useful required
identity, bounded child-name, reserved-name, intent, and no-replace capture test
work, but its capture contract can mutate without a required identity, can move
an unrelated pre-existing custody object into the authoritative target, and has
no callable typed-unsupported surface on non-Unix platforms.

The last approved program count remains ten implementation tasks (A1-A4 plus
B-G), but that count is not advanced by this rejected A1. A targeted design
correction must decide whether the bounded A1 contract can be repaired within
one compile-correct cut or must be split before any new implementation dispatch.
A2's frozen input does not exist yet.

## Frozen method and execution record

The round reused the slice-3 method: freeze the rejected Task A artifact, define
one compile-correct cut with an exact line ceiling, validate a Tier 3 config and
typed brief, run one write-capable implementation, source-adjudicate the review,
allow one targeted repair only for a closed enumerable rejection, run the full
host gate, then send one hard-read-only closure review.

Declared cap: one implementation review, at most one targeted repair on the same
artifact, and one closure review. A converging gate-only tail could receive one
disclosed operator correction; an open or growing review population parks the
cut. The A1 stop was 200 production or 450 total changed lines relative to
`517703cb`.

- Initial implementation run clone: `impl-77617-f18mbkc5`.
- Initial candidate: `c0b5993a6c2ce5884ffcbb004f26442f2ba52b64`.
- Initial terminal artifact SHA-256:
  `2db5149837e16033e2be2f093012a60a30ff7f418d8ec191a966b4958508c548`.
- Initial Sol/xhigh review: `REJECT`; one BLOCKER WRONG because the candidate
  stopped at declarations and omitted the required capture/restoration contract,
  plus one test-coverage SMELL.
- Targeted repair execution: `exec-6335436d74a73c4a7d802a37aa87ebe3` /
  `attempt-911f18c5d70d6345be0dbc733ddbfe68`; terminal output SHA-256
  `bf9bb0365de031c1eae9d121ce6a7c9b748146d3e625e823ec51ebcf37531c02`.
- One earlier repair dispatch refused before prompt because mixed-language LSP
  delivery lacked the required provider-effect key. It changed no repository
  bytes and is inadmissible to implementation quality.

The repaired focused gate first returned 6 passed / 1 failed. On macOS, raw
`ENOTSUP` maps to `ErrorKind::Uncategorized`, so the implementation's
`ErrorKind::Unsupported` test misclassified a proved-no-effect runtime refusal.
The source and a same-host raw-errno probe discriminated this from lost target
identity. Because this was one smaller non-repeating gate defect, the operator
used the declared blind-tail extension to classify raw `ENOTSUP`,
`EOPNOTSUPP`, and `ENOSYS` directly. No second agent repair or review was added.

The amended exact candidate is `bc262ad4`, parent `517703cb`, with exactly two
authorized paths and 450 insertions: 200 production, 224 colocated tests, and 26
handoff lines. Its worktree is clean.

## Exact-head gate evidence

At `bc262ad4`:

- focused `custody_v2`: 7 passed, 0 failed;
- full `fs_custody`: 73 passed, 0 failed;
- locked all-feature workspace suite: **3,995 passed / 0 failed / 13 ignored
  across 90 harnesses**;
- `git diff --check`, `cargo fmt --all -- --check`, locked all-target/all-feature
  workspace check, and warnings-denied Clippy: exit 0;
- locked release `a2a-bridge` build: exit 0;
- `cargo deny check`: exit 0, with advisories, bans, licenses, and sources all
  okay and policy-allowed duplicate-version warnings retained;
- repository hygiene: exit 0, 40 tracked artifacts and 8 example configs.

The first sandboxed full-suite command was refused before compilation because
Cargo could not open `target/debug/.cargo-lock`; it is inadmissible. The exact
approved-host rerun above is the gate.

The installed `x86_64-pc-windows-msvc` target did not make a Mac cross-check
admissible: `ring` failed first in its C build because MSVC headers such as
`assert.h` are absent, before `bridge-core` compiled. This exclusion is carried
into the verdict and cannot green the typed non-Unix contract.

## Closure review and source adjudication

The single closure review ran with Codex `gpt-5.6-sol` / xhigh / hard read-only:

- execution `exec-53d9a7fc0e4f71b44ce7e2f599b83905`;
- attempt `attempt-69c2548519fd015f8fcf801250edf835`;
- terminal artifact: 9,981 bytes, SHA-256
  `bcd32c4cf8400300a4069d4a94c10635496cc8434c31744856ddf110f687ac0f`;
- terminal: `VERDICT: REJECT`, three BLOCKER WRONGs and two deferred SMELLs.

The findings/verdict mirror is
[`2026-08-13-r2f1b-3c2-task-a1-sol-closure.md`](2026-08-13-r2f1b-3c2-task-a1-sol-closure.md):
8,513 bytes, SHA-256
`fd99293fe79c851f8805c737049958d4c2f8db3d14394174f08a738b60dc7467`.

A prior strict-brief dispatch refused before the workflow node because supplied
test totals lacked an explicit falsification license. No provider prompt ran;
that refusal did not consume the closure review.

Operator source adjudication confirms all three WRONGs:

1. **WRONG - incomplete identity does not refuse before mutation.**
   `required_identity_at_v2` converts open, type, device/inode, and birthtime
   failures to `None`. `capture_target_no_replace_v2_with` records that value but
   still invokes the target-to-custody rename. A regular target on a filesystem
   without usable birthtime can therefore be displaced and reported `Unknown`,
   violating the binding pre-mutation `Unsupported` contract.
2. **WRONG - failed capture can restore an unrelated custody object.** When the
   target cannot be proved unchanged after a failed rename, the code treats any
   object now at the custody name as the captured target. With target absent and
   stale custody object C present, it moves C into the authoritative target and
   reports `UnexpectedRestored`, even though capture had no effect.
3. **WRONG - typed compile-unsupported is unreachable on non-Unix.** The child
   constructors, reserved-name codec, intent constructor, and public capture API
   are `#[cfg(unix)]`. A Windows consumer receives an absent symbol, not
   `CustodyCaptureOutcomeV2::CompileUnsupported` before mutation.

Repository-wide search finds no production caller for the A1 symbols; only the
new colocated tests reference them. The failures are therefore dormant on this
head, but mandatory for the next cuts and not deferrable. The reviewer also
retains test-evidence SMELLs for missing cross-namespace/exact-bound cases and
behavioral fail-first evidence. They do not collapse any WRONG.

## Convergence decision and next design question

The cap is exhausted. The initial review had one broad missing-contract WRONG;
after the sole repair and one smaller gate correction, closure found three
distinct mechanism-level WRONGs. The population grew and crossed pre-mutation
proof, failed-syscall attribution, and cross-platform API shape. Another local
edit/review would silently extend the cap and would start A2 from a rejected
foundation.

Preserve `bc262ad4` as evidence and salvage input. The targeted A1 redesign must
bind, before another implementation turn:

- a typed identity probe that positively refuses before the capture boundary on
  missing birthtime, non-regular type, or inspection failure;
- positive evidence that a post-error custody object came from the exact
  pre-rename target, with stale/foreign custody left untouched and protective;
- a portable child-name/intent surface and a callable non-Unix capture stub that
  returns `CompileUnsupported` without inspection or mutation;
- behavioral red seams for the three WRONGs plus the missing parser/boundary
  negatives; and
- a new honest line/slice budget rather than compression at the old exact cap.

No A2, Task B, fold, push, CI, provider retry, smoke, compatibility run,
deployment, production V3 arming, or running-operator mutation follows. The
two-field cleanup carry-forward remains binding in the first later slice that
arms production V3 or wraps `ContainerRw`; 3d remains blocked.
