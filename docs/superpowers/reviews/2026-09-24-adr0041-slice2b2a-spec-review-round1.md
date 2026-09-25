# ADR-0041 Slice 2B2a spec review — round 1

**Date:** 2026-09-24
**Round cap:** 1 of 2 admitted rounds consumed
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-654c0b668b8b043df3c7922928b3bb1c` / `attempt-980854095df6ee823dcfb6a00b7ba1ed`
**Reviewed commit:** `054c889bf68f9942d541f170434aa2485dfd5946` (tree `174b305f76cbc209c530699d79e9e349859fcf75`)
**Reviewed task SHA-256:** `40ab120edea347aa6a5a121a2ba7fb74581080d6518a46fe82b3d398caca3b2b` (revision 1)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review1-20260924/result.md` (SHA-256
`8bdedf420395dbc0eac4bc89e79c6e13e70583211d6338c7738467f634a24566`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`. WRONG 5 / SMELL 5; BLOCKER 5 / DEFER 5. The population is closed and enumerable.

The reviewer confirmed these source claims:

- `PinnedDirectoryV1.file` is private;
- `create_new_regular_child_at`, `open_child_no_follow`, `ChildOpenOptionsV1`, and the identity and error types exist;
- `libc` and `ring` are existing dependencies;
- the `CatFileAllObjects` format survives as one argv element;
- relative environment paths resolve after `fchdir`;
- the trusted-owner test constructor need not widen production admission.

It noted that "exact predecessor" meant the implementation base rather than the document's parent commit.

## WRONG findings (all folded in revision 2)

1. **Bootstrap executed an unadmitted target.** Running the `/usr/bin/git` trampoline's `--exec-path` executes
   whatever Git the developer directory selects, before admission. **Fold:** locate with `/usr/bin/xcrun --find git`,
   which does not execute Git and is itself admitted; admit before the first execution; check the fixed point
   afterwards. Control A5a now uses a marker.
2. **`VerifyPack { index }` had no usable path from the `work/` root.** **Fold:** `VerifyPack { git_dir, pack_hash }`
   with a runner-built `objects/pack/pack-<hash>.idx` path; control A16.
3. **The unsafe boundary could not be satisfied.** The required `geteuid` and `faccessat` calls were outside the three
   authorized boundaries. **Fold:** a per-file authorized unsafe list; control A14 inventories it.
4. **A10 accepted a timeout, so a serial deadlocking implementation passed.** **Fold:** A10 requires success before
   the deadline.
5. **A11d had no guard inside 2B2a.** **Fold:** A11d becomes an `InitBare` tree-shape and no-config control. The
   source-config isolation control returns to 2B2 as control 30, supported by a `#[cfg(test)]` guard-bypass seam.

## SMELL findings (all folded)

1. `InitBare` with both `GIT_DIR` and a positional directory. **Fold:** `GIT_DIR` is omitted for `InitBare`; probe
   P4 showed the precedence trap.
2. Stderr cap semantics. **Fold:** symmetric with stdout; control A8b.
3. Alternates encoding. **Fold:** refuse `:`, `"`, `\`, newline, and NUL; control A15.
4. Evidence recorded keys but not values. **Fold:** keys and values, with path digests.
5. No minimum-version control. **Fold:** parser rules; control A13.

## Probes added (macOS Command Line Tools Git 2.54)

P3 and P4 are recorded in task §13. Two of the P4 sub-probes were first run with an unsplit zsh variable, so they
never ran and are inadmissible. They were rerun correctly with a shell function before any belief update.

## Convergence

This round is the first on the new child, and its findings are closed, so they are repaired on the same artifact as
revision 2. Round 2 is the final admitted round.
