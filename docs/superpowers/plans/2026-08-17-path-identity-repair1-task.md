---
task-type: implement
---

# Path-identity primitive — repair 1: no new dependency, and stop over-refusing

## Description

Targeted repair on a FROZEN artifact. Base: `fed0f992` on branch
`salvage/r2f1b-path-identity-first`.

**Nothing in that artifact has been verified.** It added `icu_normalizer = "2.2"`
to `crates/bridge-core/Cargo.toml` and a single `"icu_normalizer",` line to
`Cargo.lock`, which is not a valid lock update — a real one needs the
`[[package]]` entry plus the whole transitive tree. `--locked` therefore failed
at lock-sync and **clippy, build and test never ran**. Treat every part of the
artifact as unchecked, including the parts this repair does not name.

## R1 — remove the dependency; this lane cannot add one

**You cannot add a crate — but the operator can.** The implement lane's egress
permits model APIs only (ADR-0013), so *you* have no crates.io access and no
compile loop: you cannot resolve a new dependency or regenerate `Cargo.lock`,
and hand-editing the lock produces exactly the lock-sync failure above.

CORRECTION to an earlier overstatement in this lane's record: that does NOT make
a new dependency structurally impossible. The operator can provision one on the
host (`cargo add` + a real lock update + `cargo deny check`) and dispatch on a
base where it is already resolved; `egress = "open"` on the impl sandbox also
exists (`examples/a2a-bridge.m4-slice3a-impl-openegress.toml`) though it breaks
the deliberate creds-XOR-registries split. For THIS task the owner decided
against the crate on its merits — `icu_normalizer` pulls a large tree through a
`cargo deny` gate for a question whose safe answer is free — so do not add one.

Remove `icu_normalizer` from `crates/bridge-core/Cargo.toml` and revert the
`Cargo.lock` edit. `git diff --numstat <base>..HEAD` must show **no change** to
either file.

**The primitive does not need it.** The operator's spec over-specified by
implying normalization-aware comparison; the conservative rule below is sound,
dependency-free, and strictly fail-closed:

- Byte-equal component ⇒ `Same`.
- Ancestor filesystem is **case-sensitive** ⇒ any byte difference is
  `Different`. (On a case-sensitive, non-normalizing filesystem, NFC and NFD
  byte sequences genuinely are different filenames.)
- Ancestor filesystem is **case-insensitive** ⇒ names equal under ASCII case
  folding are `CannotProve`; names differing in any non-ASCII byte are also
  `CannotProve`, because such a filesystem may also normalize and this code
  cannot tell; anything else is `Different`.
- Case sensitivity undeterminable ⇒ `CannotProve`.

That keeps every ambiguous case refusing without needing to know a single
Unicode rule.

## R2 — WRONG: over-refusal under a case-sensitive ancestor

The internal review found the primitive classifies byte-different,
normalization-equivalent missing tails as `CannotProve` even when the shared
ancestor is on a **case-sensitive** filesystem, where the spec requires
`Different`.

This is the same failure mode that killed T3a's repair 3: a comparator that
refuses too much is fail-closed but functionally inert, because the
exact-absence proof can then never authorize whenever the repo holds any other
registration. `/managed/wt` versus `/managed/other` **must** classify
`Different`. Both directions are load-bearing.

## Verify this yourself before claiming anything

`cargo` will fail with HTTP 403 on any uncached dependency, so you have no local
compile loop — but the lock-sync failure above was avoidable and must not
recur. After removing the dependency, the manifest and lockfile must be
byte-identical to the base. Say so explicitly in your handoff, per file.

Do not present compile errors as red-first evidence, and do not claim a test
passes because verify was green. State honestly, per test, whether you ran it.

**Falsification license.** If you believe the conservative rule in R1 is wrong —
for instance if it makes a required `Different` case unreachable — say so with
the concrete case rather than reintroducing a dependency.

## Acceptance Criteria

1. `crates/bridge-core/Cargo.toml` and `Cargo.lock` are byte-identical to the
   base commit; no crate is added.
2. Under a case-sensitive ancestor, any byte-different absent sibling names
   classify `Different` — including normalization-equivalent ones.
3. Under a case-insensitive ancestor, ASCII-case-equal names and any non-ASCII
   difference classify `CannotProve`.
4. `/managed/wt` vs `/managed/other` classifies `Different` (anti-over-refusal).
5. Undeterminable case sensitivity classifies `CannotProve`.
6. The pre-existing
   `porcelain_registration_check_is_exact_and_handles_locked_records` passes
   unchanged.
7. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --
   -D warnings` clean; workspace suite green; **verify must reach the test
   stage** — a lock-sync failure is an automatic reject.
8. `git diff --numstat fed0f992..HEAD` at most 250 changed lines.

## Files

- `crates/bridge-core/src/fs_custody.rs` — the primitive.
- `crates/bridge-core/Cargo.toml`, `Cargo.lock` — must return to base.

## Spec Refs

- `docs/superpowers/plans/2026-08-17-r2f1b-path-identity-primitive-task.md` —
  the primitive's contract, including the five instances it exists to close.

## Commit Message

fix(fs-custody): drop the added crate and stop over-refusing on case-sensitive ancestors

The primitive pulled in icu_normalizer, which this lane cannot resolve — there is
no crates.io access and no compile loop, so the hand-written lock line failed
--locked at lock-sync and nothing was ever compiled or tested.

It does not need one. A case-sensitive ancestor lets bytes decide; a
case-insensitive ancestor refuses on ASCII-case-equality or any non-ASCII
difference, since such a filesystem may also normalize and this code cannot tell.
Every ambiguous case still refuses, without encoding a single Unicode rule.

Also fixes over-refusal: byte-different names under a case-sensitive ancestor are
provably Different, and classifying them CannotProve would leave the
exact-absence proof unable to authorize whenever any other registration exists.
